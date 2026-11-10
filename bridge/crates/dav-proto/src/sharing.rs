// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Sharing calendars and address books, the
//! two ways servers offer it:
//!
//! - **WebDAV ACL** (RFC 3744): the collection's `DAV:acl` (§5.5) is read
//!   in a PROPFIND and replaced whole with the `ACL` method (§8.1). The
//!   body holds only the ACEs that are neither protected nor inherited.
//!   Stalwart does it this way (grant-only, no invert).
//! - **Calendar Server sharing** (Apple's `caldav-sharing`, also in
//!   sabre/dav (Baïkal), DAV header `calendarserver-sharing`): `CS:invite`
//!   says who it is shared with, a `POST` of `CS:share` adds (`set`) or
//!   removes (`remove`) sharees with `read` or `read-write` access.
//! - **Nextcloud sharing** (DAV header `oc-resource-sharing`, calendars
//!   and address books): the same idea in the ownCloud namespace.
//!   `O:invite` lists the sharees, a `POST` of `O:share` adds (`set`, with
//!   `O:read-write` or not) or removes them. Sharees are only named as
//!   `principal:` + the principal's path below the DAV root
//!   (`principal:principals/users/bob`, [`OcPrincipal`]); Nextcloud
//!   ignores anything else (a `mailto:`, a full path, an unknown user)
//!   and still answers 200.
//!
//! Everything is parsed leniently inside a multistatus (a value that does
//! not parse is absent, never fails the listing) and strictly when it is
//! written back: an ACL is [`Acl::is_rewritable`] only if every editable
//! ACE can be written again exactly as the server sent it and the `ACL`
//! request would be accepted (a known principal, of the collection's
//! origin if it is an href, 1 to [`MAX_PRIVILEGES`] privileges, at most
//! [`MAX_ACES`] ACEs). In the same way, an invitation with sharees that
//! have access but cannot be represented (an href that does not parse,
//! more than [`MAX_SHAREES`]) says so ([`Invite::omitted`],
//! [`OcInvite::omitted`]): a replacement computed from it would not
//! remove them. A `CS:share` body holds at most [`MAX_SHARE_REQUEST`]
//! entries and an `O:share` at most [`MAX_OC_SHARE_REQUEST`]; a bigger
//! change goes in several `POST`s.

use roxmltree::Node;

use crate::xml::{Writer, child, element_children, is, text_of};
use crate::{CalendarAddress, EncodeError, Href, Namespace, Privilege, PrivilegeSet, ResourceName};

const DAV: Namespace = Namespace::Dav;
const CS: Namespace = Namespace::CalendarServer;
const OC: Namespace = Namespace::OwnCloud;

/// Most sharees kept from one `CS:invite` or `O:invite`; the others are
/// counted in `omitted`. As many as a principal directory the bridge
/// lists (10 000): nobody can pick more people than it shows. That is
/// some 2–3 MB and 100 000 XML nodes, within one response's limits.
pub const MAX_SHAREES: usize = 10_000;
/// Most ACEs read from one `DAV:acl`, and written back in one `ACL`
/// request (it replaces the list whole, so it can't be split); more and
/// the ACL is ignored. [`MAX_SHAREES`] grants and room for the ACEs that
/// are not shares (`self`, `authenticated`, denials…).
pub const MAX_ACES: usize = MAX_SHAREES + 256;
/// Most privileges of one ACE.
pub const MAX_PRIVILEGES: usize = 32;
/// Most entries (`set` and `remove`) of one `CS:share` `POST`: sabre/dav
/// has no limit of its own, and this is some 50 KB, far from a request
/// timeout on a slow server. A bigger change is sent in several of them
/// (the caller splits it).
pub const MAX_SHARE_REQUEST: usize = 256;
/// Most entries (`set` and `remove` together) of one Nextcloud `O:share`
/// `POST`: Nextcloud refuses more with 400 "share is limited to 10 set or
/// remove elements" (`apps/dav/lib/DAV/Sharing/Plugin.php`,
/// `validateShareRequest`; seen on Nextcloud 34, 2026-11-04). A bigger
/// change is sent in several of them (the caller splits it).
pub const MAX_OC_SHARE_REQUEST: usize = 10;
/// Longest namespace URI of a privilege kept.
const MAX_NAMESPACE_LEN: usize = 256;
/// Longest local name of a privilege kept.
const MAX_LOCAL_LEN: usize = 64;

/// An XML element name (namespace URI and local name), for privileges the
/// bridge does not know but must write back unchanged.
///
/// The local name is an ASCII `NCName` (`[A-Za-z_][A-Za-z0-9._-]*`, at most
/// 64 bytes); the namespace is printable text without quotes or `<>&`
/// (at most 256 bytes).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct QualifiedName {
    namespace: String,
    local: String,
}

impl QualifiedName {
    /// A checked name.
    ///
    /// ```
    /// use nebula_dav_proto::QualifiedName;
    ///
    /// assert!(QualifiedName::new("DAV:", "read").is_some());
    /// assert!(QualifiedName::new("urn:x", "a b").is_none());
    /// assert!(QualifiedName::new("urn:\"x", "a").is_none());
    /// ```
    #[must_use]
    pub fn new(namespace: &str, local: &str) -> Option<Self> {
        let local_ok = !local.is_empty()
            && local.len() <= MAX_LOCAL_LEN
            && local
                .bytes()
                .next()
                .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
            && local
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'));
        let namespace_ok = !namespace.is_empty()
            && namespace.len() <= MAX_NAMESPACE_LEN
            && namespace
                .chars()
                .all(|c| !c.is_control() && !matches!(c, '"' | '\'' | '<' | '>' | '&'));
        (local_ok && namespace_ok).then(|| Self {
            namespace: namespace.to_owned(),
            local: local.to_owned(),
        })
    }

    /// The name of a privilege the bridge knows.
    #[must_use]
    pub fn of(privilege: Privilege) -> Self {
        let (namespace, local) = privilege.qualified_name();
        Self {
            namespace: namespace.uri().to_owned(),
            local: local.to_owned(),
        }
    }

    /// The namespace URI.
    #[must_use]
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// The local name.
    #[must_use]
    pub fn local(&self) -> &str {
        &self.local
    }

    /// The privilege it names, if the bridge knows it.
    #[must_use]
    pub fn privilege(&self) -> Option<Privilege> {
        Privilege::from_name(Namespace::from_uri(&self.namespace)?, &self.local)
    }

    fn from_node(node: Node<'_, '_>) -> Option<Self> {
        Self::new(node.tag_name().namespace()?, node.tag_name().name())
    }
}

/// Who an ACE is about (RFC 3744 §5.5.1).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AcePrincipal {
    /// `DAV:href`: one principal.
    Href(Href),
    /// `DAV:all`.
    All,
    /// `DAV:authenticated`.
    Authenticated,
    /// `DAV:unauthenticated`.
    Unauthenticated,
    /// `DAV:self`.
    Myself,
    /// `DAV:property` (the principal named by a property of the resource).
    Property(QualifiedName),
    /// Anything else (or an href that does not resolve safely): kept so
    /// the ACL is known not to be rewritable.
    Unknown,
}

/// An access control entry.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent flags of one XML element, like the RFC"
)]
pub struct Ace {
    /// Who.
    pub principal: AcePrincipal,
    /// `DAV:invert`: everyone but the principal.
    pub invert: bool,
    /// `DAV:deny` instead of `DAV:grant`.
    pub deny: bool,
    /// The privileges, as named (known or not).
    pub privileges: Vec<QualifiedName>,
    /// `DAV:protected`: the server keeps it; never sent back.
    pub protected: bool,
    /// `DAV:inherited`: comes from another resource; never sent back.
    pub inherited: bool,
}

impl Ace {
    /// A grant of `privileges` to the principal at `href`.
    #[must_use]
    pub fn grant(href: Href, privileges: &[Privilege]) -> Self {
        Self {
            principal: AcePrincipal::Href(href),
            invert: false,
            deny: false,
            privileges: privileges.iter().copied().map(QualifiedName::of).collect(),
            protected: false,
            inherited: false,
        }
    }

    /// The privileges the bridge knows, with aggregates expanded when
    /// asked ([`PrivilegeSet::allows`]).
    #[must_use]
    pub fn privilege_set(&self) -> PrivilegeSet {
        let known: Vec<Privilege> = self
            .privileges
            .iter()
            .filter_map(QualifiedName::privilege)
            .collect();
        PrivilegeSet::new(&known)
    }

    /// Part of what the `ACL` method replaces (neither protected nor
    /// inherited).
    #[must_use]
    pub const fn is_editable(&self) -> bool {
        !self.protected && !self.inherited
    }

    /// A plain grant to one principal: what a share is.
    #[must_use]
    pub fn granted_href(&self) -> Option<&Href> {
        match &self.principal {
            AcePrincipal::Href(href) if !self.invert && !self.deny && self.is_editable() => {
                Some(href)
            }
            _ => None,
        }
    }
}

/// A `DAV:acl` value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Acl {
    /// The ACEs, in order (order matters with deny ACEs).
    pub aces: Vec<Ace>,
}

impl Acl {
    /// Every ACE that would go back in an `ACL` request on `collection`
    /// can be written exactly as the server sent it, and the request would
    /// be accepted: what `request::acl` and the client's `set_acl` check
    /// (a known principal, an href principal of the collection's origin,
    /// 1 to [`MAX_PRIVILEGES`] privileges, at most [`MAX_ACES`] editable
    /// ACEs).
    #[must_use]
    pub fn is_rewritable(&self, collection: &Href) -> bool {
        self.editable().count() <= MAX_ACES
            && self.editable().all(|ace| {
                let principal = match &ace.principal {
                    AcePrincipal::Unknown => false,
                    AcePrincipal::Href(href) => href.same_origin(collection),
                    _ => true,
                };
                principal && (1..=MAX_PRIVILEGES).contains(&ace.privileges.len())
            })
    }

    /// The editable ACEs, for an `ACL` request.
    pub fn editable(&self) -> impl Iterator<Item = &Ace> {
        self.aces.iter().filter(|ace| ace.is_editable())
    }
}

fn ace_principal(node: Node<'_, '_>, base: &Href) -> AcePrincipal {
    let Some(inner) = element_children(node).next() else {
        return AcePrincipal::Unknown;
    };
    if is(inner, DAV, "href") {
        return text_of(inner, Href::MAX_LEN + 64)
            .ok()
            .flatten()
            .and_then(|text| Href::resolve(text.trim(), base).ok())
            .map_or(AcePrincipal::Unknown, AcePrincipal::Href);
    }
    if is(inner, DAV, "all") {
        AcePrincipal::All
    } else if is(inner, DAV, "authenticated") {
        AcePrincipal::Authenticated
    } else if is(inner, DAV, "unauthenticated") {
        AcePrincipal::Unauthenticated
    } else if is(inner, DAV, "self") {
        AcePrincipal::Myself
    } else if is(inner, DAV, "property") {
        element_children(inner)
            .next()
            .and_then(QualifiedName::from_node)
            .map_or(AcePrincipal::Unknown, AcePrincipal::Property)
    } else {
        AcePrincipal::Unknown
    }
}

/// Every privilege of a `grant` or `deny`: all the elements of each
/// `DAV:privilege` (RFC 3744 allows several in one; writing each in its
/// own `DAV:privilege` means the same). A `DAV:privilege` without any
/// makes the ACE unreadable.
fn ace_privileges(node: Node<'_, '_>) -> Option<Vec<QualifiedName>> {
    let mut out = Vec::new();
    for privilege in element_children(node).filter(|c| is(*c, DAV, "privilege")) {
        let mut names = element_children(privilege).peekable();
        names.peek()?;
        for name in names {
            if out.len() >= MAX_PRIVILEGES {
                return None;
            }
            out.push(QualifiedName::from_node(name)?);
        }
    }
    Some(out)
}

fn parse_ace(node: Node<'_, '_>, base: &Href) -> Option<Ace> {
    let mut ace = Ace {
        principal: AcePrincipal::Unknown,
        invert: false,
        deny: false,
        privileges: Vec::new(),
        protected: false,
        inherited: false,
    };
    let mut grant_or_deny = false;
    for item in element_children(node) {
        if is(item, DAV, "principal") {
            ace.principal = ace_principal(item, base);
        } else if is(item, DAV, "invert") {
            ace.invert = true;
            ace.principal = child(item, DAV, "principal")
                .map_or(AcePrincipal::Unknown, |p| ace_principal(p, base));
        } else if is(item, DAV, "grant") || is(item, DAV, "deny") {
            if grant_or_deny {
                return None;
            }
            grant_or_deny = true;
            ace.deny = is(item, DAV, "deny");
            ace.privileges = ace_privileges(item)?;
        } else if is(item, DAV, "protected") {
            ace.protected = true;
        } else if is(item, DAV, "inherited") {
            ace.inherited = true;
        }
    }
    grant_or_deny.then_some(ace)
}

/// `DAV:acl`, or `None` if it does not follow RFC 3744 or is over the
/// limits.
pub(crate) fn parse_acl(node: Node<'_, '_>, base: &Href) -> Option<Acl> {
    let mut aces = Vec::new();
    for item in element_children(node).filter(|c| is(*c, DAV, "ace")) {
        if aces.len() >= MAX_ACES {
            return None;
        }
        aces.push(parse_ace(item, base)?);
    }
    Some(Acl { aces })
}

/// The access of a Calendar Server share.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShareAccess {
    /// `CS:read`.
    Read,
    /// `CS:read-write`.
    ReadWrite,
}

impl ShareAccess {
    const fn local(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::ReadWrite => "read-write",
        }
    }
}

/// Where a sharee stands with an invitation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum InviteStatus {
    /// `CS:invite-accepted`.
    Accepted,
    /// `CS:invite-noresponse` (or nothing said).
    NoResponse,
    /// `CS:invite-declined`.
    Declined,
    /// `CS:invite-invalid`: the server does not know who it is.
    Invalid,
}

/// Who a calendar is shared with: an address or a principal's location.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Sharee {
    /// `mailto:…`.
    Address(CalendarAddress),
    /// A principal.
    Principal(Href),
}

impl Sharee {
    fn parse(text: &str, base: &Href) -> Option<Self> {
        let text = text.trim();
        let is_mailto = text
            .get(..7)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("mailto:"));
        if is_mailto {
            return CalendarAddress::parse(text).map(Self::Address);
        }
        Href::resolve(text, base).ok().map(Self::Principal)
    }

    fn write(&self, w: &mut Writer) -> Result<(), EncodeError> {
        let text = match self {
            Self::Address(address) => address.as_str(),
            Self::Principal(href) => href.path(),
        };
        w.text_element(DAV, "href", text).map(|_| ())
    }
}

/// One `CS:user` of `CS:invite`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteUser {
    /// Who.
    pub sharee: Sharee,
    /// `CS:access` (`None`: empty, as sabre/dav writes the owner).
    pub access: Option<ShareAccess>,
    /// The invitation's status.
    pub status: InviteStatus,
}

/// `CS:invite`: who a calendar is shared with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Invite {
    /// `CS:organizer` (Apple): the owner.
    pub organizer: Option<Sharee>,
    /// `CS:user`, one per sharee (sabre/dav lists the owner too, with an
    /// empty `CS:access`).
    pub users: Vec<InviteUser>,
    /// Users with access (accepted or not answered yet) left out of
    /// `users`: an href that is not a principal or address the bridge
    /// reads (`urn:x-uid:…`), or more than [`MAX_SHAREES`] users.
    pub omitted: usize,
}

impl Invite {
    /// Every user with access is in `users`.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.omitted == 0
    }

    /// The owner: the organizer, else the only user without an access
    /// (sabre/dav's way).
    #[must_use]
    pub fn owner(&self) -> Option<&Sharee> {
        if let Some(organizer) = &self.organizer {
            return Some(organizer);
        }
        let mut owners = self.users.iter().filter(|user| user.access.is_none());
        match (owners.next(), owners.next()) {
            (Some(only), None) => Some(&only.sharee),
            _ => None,
        }
    }

    /// The sharees with an access (the owner left out).
    pub fn sharees(&self) -> impl Iterator<Item = (&Sharee, ShareAccess, InviteStatus)> {
        self.users
            .iter()
            .filter_map(|user| Some((&user.sharee, user.access?, user.status)))
    }
}

fn href_text(node: Node<'_, '_>) -> Option<String> {
    child(node, DAV, "href").and_then(|href| text_of(href, Href::MAX_LEN + 64).ok().flatten())
}

/// The invitation status of a `CS:user` or `O:user`.
fn invite_status(item: Node<'_, '_>, namespace: Namespace) -> InviteStatus {
    if child(item, namespace, "invite-accepted").is_some() {
        InviteStatus::Accepted
    } else if child(item, namespace, "invite-declined").is_some() {
        InviteStatus::Declined
    } else if child(item, namespace, "invite-invalid").is_some() {
        InviteStatus::Invalid
    } else {
        InviteStatus::NoResponse
    }
}

/// A user that has access to the collection (and so would have to be
/// removed by a replacement).
const fn has_access(status: InviteStatus) -> bool {
    matches!(status, InviteStatus::Accepted | InviteStatus::NoResponse)
}

/// `CS:invite`. Users whose href does not parse, and users past
/// [`MAX_SHAREES`], are left out; those with access are counted in
/// [`Invite::omitted`].
pub(crate) fn parse_invite(node: Node<'_, '_>, base: &Href) -> Invite {
    let mut invite = Invite::default();
    for item in element_children(node) {
        if is(item, CS, "organizer") {
            invite.organizer = href_text(item).and_then(|text| Sharee::parse(&text, base));
        } else if is(item, CS, "user") {
            let access = child(item, CS, "access").and_then(|access| {
                if child(access, CS, "read-write").is_some() {
                    Some(ShareAccess::ReadWrite)
                } else if child(access, CS, "read").is_some() {
                    Some(ShareAccess::Read)
                } else {
                    None
                }
            });
            let status = invite_status(item, CS);
            let sharee = if invite.users.len() < MAX_SHAREES {
                href_text(item).and_then(|text| Sharee::parse(&text, base))
            } else {
                None
            };
            match sharee {
                Some(sharee) => invite.users.push(InviteUser {
                    sharee,
                    access,
                    status,
                }),
                None if access.is_some() && has_access(status) => {
                    invite.omitted = invite.omitted.saturating_add(1);
                }
                None => {}
            }
        }
    }
    invite
}

/// `CS:allowed-sharing-modes`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct SharingModes {
    /// `CS:can-be-shared`.
    pub can_be_shared: bool,
    /// `CS:can-be-published`.
    pub can_be_published: bool,
}

pub(crate) fn parse_sharing_modes(node: Node<'_, '_>) -> SharingModes {
    SharingModes {
        can_be_shared: child(node, CS, "can-be-shared").is_some(),
        can_be_published: child(node, CS, "can-be-published").is_some(),
    }
}

/// A Nextcloud principal, as its sharing names it: the principal's path
/// below the DAV root without slashes around it (`principals/users/bob`),
/// written `principal:principals/users/bob`.
///
/// Checked like an href path: at least two segments, the first one
/// `principals`, no empty, `.` or `..` segment, no `\`, no control
/// characters, at most [`Href::MAX_SEGMENTS`] segments and
/// [`Href::MAX_LEN`] bytes. Segments are kept decoded, as Nextcloud
/// writes them (a user id is not percent-encoded there).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OcPrincipal(Vec<String>);

impl OcPrincipal {
    /// The scheme-like prefix of the href.
    pub const PREFIX: &'static str = "principal:";
    /// The first segment of every principal path.
    const COLLECTION: &'static str = "principals";

    /// Reads `principal:principals/users/bob`.
    ///
    /// ```
    /// use nebula_dav_proto::OcPrincipal;
    ///
    /// let bob = OcPrincipal::parse("principal:principals/users/bob").unwrap();
    /// assert_eq!(bob.to_string(), "principal:principals/users/bob");
    /// assert!(OcPrincipal::parse("principal:principals/../x").is_none());
    /// assert!(OcPrincipal::parse("mailto:bob@example.com").is_none());
    /// assert!(OcPrincipal::parse("principal:calendars/bob").is_none());
    /// ```
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let path = text.trim().strip_prefix(Self::PREFIX)?;
        if path.len() > Href::MAX_LEN {
            return None;
        }
        Self::from_segments(path.split('/').map(str::to_owned).collect())
    }

    fn from_segments(segments: Vec<String>) -> Option<Self> {
        let valid = segments.len() >= 2
            && segments.len() <= Href::MAX_SEGMENTS
            && segments
                .first()
                .is_some_and(|first| first == Self::COLLECTION)
            && segments
                .iter()
                .all(|segment| ResourceName::try_from(segment.as_str()).is_ok());
        valid.then_some(Self(segments))
    }

    /// The DAV root of a principal's location: everything before its last
    /// `principals` segment (`/remote.php/dav/` for
    /// `/remote.php/dav/principals/users/alice/`).
    #[must_use]
    pub fn dav_root(principal: &Href) -> Option<Href> {
        let segments = principal.segments();
        let at = segments.iter().rposition(|s| s == Self::COLLECTION)?;
        let mut root = Href::resolve("/", principal).ok()?;
        for segment in segments.get(..at)? {
            root = root
                .member(&ResourceName::try_from(segment.as_str()).ok()?, true)
                .ok()?;
        }
        Some(root)
    }

    /// The principal at `href`, named from the DAV `root` (`None` if it is
    /// not a principal below it).
    #[must_use]
    pub fn of(href: &Href, root: &Href) -> Option<Self> {
        if !href.is_inside(root) {
            return None;
        }
        let below = href.segments().get(root.segments().len()..)?;
        Self::from_segments(below.to_vec())
    }

    /// Where it is, below the DAV `root` (same origin, as a collection).
    #[must_use]
    pub fn href(&self, root: &Href) -> Option<Href> {
        let mut at = root.as_collection();
        for segment in &self.0 {
            at = at
                .member(&ResourceName::try_from(segment.as_str()).ok()?, true)
                .ok()?;
        }
        Some(at)
    }
}

impl std::fmt::Display for OcPrincipal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(Self::PREFIX)?;
        f.write_str(&self.0.join("/"))
    }
}

/// One `O:user` of `O:invite`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcShare {
    /// Who (a user, a group…).
    pub principal: OcPrincipal,
    /// `O:read-write`, else read.
    pub access: ShareAccess,
    /// The invitation's status (Nextcloud accepts at once).
    pub status: InviteStatus,
}

/// `O:invite`: who a Nextcloud calendar or address book is shared with
/// (empty for the sharee's view of it).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OcInvite {
    /// The sharees.
    pub users: Vec<OcShare>,
    /// Users with access (accepted or not answered yet) left out of
    /// `users`: an href that is not a `principal:`, or more than
    /// [`MAX_SHAREES`] users.
    pub omitted: usize,
}

impl OcInvite {
    /// Every user with access is in `users`.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.omitted == 0
    }
}

/// `O:invite`. Users whose href is not a `principal:`, and users past
/// [`MAX_SHAREES`], are left out; those with access are counted in
/// [`OcInvite::omitted`].
pub(crate) fn parse_oc_invite(node: Node<'_, '_>) -> OcInvite {
    let mut invite = OcInvite::default();
    for item in element_children(node).filter(|c| is(*c, OC, "user")) {
        let status = invite_status(item, OC);
        let principal = if invite.users.len() < MAX_SHAREES {
            href_text(item).and_then(|text| OcPrincipal::parse(&text))
        } else {
            None
        };
        let Some(principal) = principal else {
            if has_access(status) {
                invite.omitted = invite.omitted.saturating_add(1);
            }
            continue;
        };
        let access =
            if child(item, OC, "access").is_some_and(|a| child(a, OC, "read-write").is_some()) {
                ShareAccess::ReadWrite
            } else {
                ShareAccess::Read
            };
        invite.users.push(OcShare {
            principal,
            access,
            status,
        });
    }
    invite
}

/// The body of a Nextcloud sharing `POST` (`O:share`).
///
/// # Errors
///
/// Nothing to do, or more than [`MAX_OC_SHARE_REQUEST`] sharees.
pub(crate) fn oc_share_body(
    set: &[(OcPrincipal, ShareAccess)],
    remove: &[OcPrincipal],
) -> Result<String, EncodeError> {
    if set.is_empty() && remove.is_empty() || set.len() + remove.len() > MAX_OC_SHARE_REQUEST {
        return Err(EncodeError::Range);
    }
    let mut w = Writer::new(OC, "share");
    for (principal, access) in set {
        w.start(OC, "set");
        w.text_element(DAV, "href", &principal.to_string())?;
        w.empty(OC, access.local());
        w.end();
    }
    for principal in remove {
        w.start(OC, "remove");
        w.text_element(DAV, "href", &principal.to_string())?;
        w.end();
    }
    Ok(w.finish())
}

fn write_principal(w: &mut Writer, principal: &AcePrincipal) -> Result<(), EncodeError> {
    w.start(DAV, "principal");
    match principal {
        AcePrincipal::Href(href) => {
            w.text_element(DAV, "href", href.path())?;
        }
        AcePrincipal::All => {
            w.empty(DAV, "all");
        }
        AcePrincipal::Authenticated => {
            w.empty(DAV, "authenticated");
        }
        AcePrincipal::Unauthenticated => {
            w.empty(DAV, "unauthenticated");
        }
        AcePrincipal::Myself => {
            w.empty(DAV, "self");
        }
        AcePrincipal::Property(name) => {
            w.start(DAV, "property");
            w.empty_qualified(name)?;
            w.end();
        }
        AcePrincipal::Unknown => return Err(EncodeError::Range),
    }
    w.end();
    Ok(())
}

/// The body of an `ACL` request (RFC 3744 §8.1), see [`crate::request::acl`].
///
/// # Errors
///
/// An ACE with an [`AcePrincipal::Unknown`] principal, more than
/// [`MAX_ACES`] ACEs or [`MAX_PRIVILEGES`] privileges, or an ACE without
/// privileges.
pub(crate) fn acl_body<'a>(aces: impl IntoIterator<Item = &'a Ace>) -> Result<String, EncodeError> {
    let mut w = Writer::new(DAV, "acl");
    let mut count = 0_usize;
    for ace in aces.into_iter().filter(|ace| ace.is_editable()) {
        count += 1;
        if count > MAX_ACES || ace.privileges.is_empty() || ace.privileges.len() > MAX_PRIVILEGES {
            return Err(EncodeError::Range);
        }
        w.start(DAV, "ace");
        if ace.invert {
            w.start(DAV, "invert");
            write_principal(&mut w, &ace.principal)?;
            w.end();
        } else {
            write_principal(&mut w, &ace.principal)?;
        }
        w.start(DAV, if ace.deny { "deny" } else { "grant" });
        for privilege in &ace.privileges {
            w.start(DAV, "privilege");
            w.empty_qualified(privilege)?;
            w.end();
        }
        w.end().end();
    }
    Ok(w.finish())
}

/// The body of a Calendar Server `POST` (`CS:share`): add or change
/// sharees (`set`, with their access) and remove others.
///
/// # Errors
///
/// Nothing to do, or more than [`MAX_SHARE_REQUEST`] sharees.
pub(crate) fn share_body(
    set: &[(Sharee, ShareAccess)],
    remove: &[Sharee],
) -> Result<String, EncodeError> {
    if set.is_empty() && remove.is_empty() || set.len() + remove.len() > MAX_SHARE_REQUEST {
        return Err(EncodeError::Range);
    }
    let mut w = Writer::new(CS, "share");
    for (sharee, access) in set {
        w.start(CS, "set");
        sharee.write(&mut w)?;
        w.empty(CS, access.local());
        w.end();
    }
    for sharee in remove {
        w.start(CS, "remove");
        sharee.write(&mut w)?;
        w.end();
    }
    Ok(w.finish())
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::*;
    use crate::{Property, XmlLimits, parse_multistatus};

    fn base() -> Href {
        Href::from_url(&Url::parse("http://localhost:8080/dav/cal/alice/").unwrap()).unwrap()
    }

    fn props(xml: &str) -> crate::Properties {
        let url = Url::parse("http://localhost:8080/dav/cal/alice/").unwrap();
        let mut multistatus =
            parse_multistatus(xml.as_bytes(), &url, &XmlLimits::default()).unwrap();
        multistatus.responses.remove(0).properties
    }

    /// What Stalwart 0.16.27 answers for a calendar alice shares with bob
    /// (read and write), as seen live.
    const STALWART: &str = r#"<?xml version="1.0" encoding="UTF-8"?><D:multistatus xmlns:D="DAV:" xmlns:A="urn:ietf:params:xml:ns:caldav"><D:response><D:href>/dav/cal/alice%40nebulamail.test/probe-share/</D:href><D:propstat><D:prop><D:displayname>Probe share</D:displayname><D:resourcetype><D:collection/><A:calendar/></D:resourcetype><D:owner><D:href>/dav/pal/alice%40nebulamail.test/</D:href></D:owner><D:acl><D:ace><D:principal><D:href>/dav/pal/bob%40nebulamail.test/</D:href></D:principal><D:grant><D:privilege><D:write-content/></D:privilege><D:privilege><A:read-free-busy/></D:privilege><D:privilege><D:write/></D:privilege><D:privilege><D:read-current-user-privilege-set/></D:privilege><D:privilege><D:read/></D:privilege><D:privilege><D:write-properties/></D:privilege></D:grant></D:ace></D:acl><D:acl-restrictions><D:grant-only/><D:no-invert/></D:acl-restrictions></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>"#;

    /// sabre/dav 4.7 (Baïkal 0.10.1): bob's copy of a calendar alice shares
    /// with him, as seen live.
    const SABRE_SHAREE: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:s="http://sabredav.org/ns" xmlns:cal="urn:ietf:params:xml:ns:caldav" xmlns:cs="http://calendarserver.org/ns/"><d:response><d:href>/dav.php/calendars/bob/6febbec2/</d:href><d:propstat><d:prop><d:displayname>Probe share</d:displayname><d:resourcetype><d:collection/><cal:calendar/><cs:shared/></d:resourcetype><d:current-user-privilege-set><d:privilege><cal:read-free-busy/></d:privilege><d:privilege><d:read/></d:privilege><d:privilege><d:read-acl/></d:privilege><d:privilege><d:read-current-user-privilege-set/></d:privilege><d:privilege><d:write-properties/></d:privilege></d:current-user-privilege-set><d:owner><d:href>/dav.php/principals/bob/</d:href></d:owner><cs:invite><cs:user><cs:invite-accepted/><cs:access/><d:href>/dav.php/principals/alice</d:href></cs:user><cs:user><cs:invite-accepted/><cs:access><cs:read/></cs:access><d:href>mailto:bob@nebulamail.test</d:href></cs:user></cs:invite><cs:allowed-sharing-modes><cs:can-be-shared/></cs:allowed-sharing-modes><d:acl><d:ace><d:principal><d:href>/dav.php/principals/bob/</d:href></d:principal><d:grant><d:privilege><d:write-properties/></d:privilege></d:grant><d:protected/></d:ace><d:ace><d:principal><d:authenticated/></d:principal><d:grant><d:privilege><cal:read-free-busy/></d:privilege></d:grant><d:protected/></d:ace></d:acl></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;

    #[test]
    fn stalwart_acl() {
        let p = props(STALWART);
        assert_eq!(p.owner.unwrap().path(), "/dav/pal/alice@nebulamail.test/");
        let acl = p.acl.unwrap();
        let collection =
            Href::resolve("/dav/cal/alice%40nebulamail.test/probe-share/", &base()).unwrap();
        assert!(acl.is_rewritable(&collection));
        let elsewhere =
            Href::from_url(&Url::parse("http://other.example/dav/cal/x/").unwrap()).unwrap();
        assert!(!acl.is_rewritable(&elsewhere));
        let [ace] = acl.aces.as_slice() else {
            panic!("one ACE")
        };
        assert_eq!(
            ace.granted_href().unwrap().path(),
            "/dav/pal/bob@nebulamail.test/"
        );
        let set = ace.privilege_set();
        assert!(set.can_read() && set.can_write_content() && set.can_delete());
        assert!(set.allows(Privilege::ReadFreeBusy) && !set.allows(Privilege::WriteAcl));
        assert_eq!(ace.privileges.len(), 6);
    }

    #[test]
    fn sabre_sharee() {
        let p = props(SABRE_SHAREE);
        assert!(p.has(Property::Invite));
        let kind = p.resource_type.unwrap();
        assert!(kind.calendar && kind.shared && !kind.shared_owner);
        let invite = p.invite.unwrap();
        assert_eq!(
            invite.owner(),
            Some(&Sharee::Principal(
                Href::resolve("/dav.php/principals/alice", &base()).unwrap()
            ))
        );
        let sharees: Vec<_> = invite.sharees().collect();
        assert_eq!(sharees.len(), 1);
        assert_eq!(
            sharees[0],
            (
                &Sharee::Address(CalendarAddress::parse("mailto:bob@nebulamail.test").unwrap()),
                ShareAccess::Read,
                InviteStatus::Accepted
            )
        );
        assert!(p.sharing_modes.unwrap().can_be_shared);
        // Protected ACEs only: nothing to write back.
        let acl = p.acl.unwrap();
        assert_eq!(acl.editable().count(), 0);
        assert!(acl.aces.iter().all(|ace| ace.granted_href().is_none()));
    }

    #[test]
    fn apple_organizer() {
        let xml = r#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:" xmlns:CS="http://calendarserver.org/ns/"><D:response><D:href>/calendars/users/bob/shared/</D:href><D:propstat><D:prop><CS:invite><CS:organizer><D:href>/principals/users/alice/</D:href><CS:common-name>Alice</CS:common-name></CS:organizer><CS:user><D:href>urn:x-uid:bob</D:href><CS:invite-noresponse/><CS:access><CS:read-write/></CS:access></CS:user><CS:user><D:href>mailto:carol@example.com</D:href><CS:invite-declined/><CS:access><CS:read/></CS:access></CS:user></CS:invite></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>"#;
        let invite = props(xml).invite.unwrap();
        assert_eq!(
            invite.owner(),
            Some(&Sharee::Principal(
                Href::resolve("/principals/users/alice/", &base()).unwrap()
            ))
        );
        // `urn:x-uid:` is not an href we follow: left out, and counted,
        // as bob has access.
        let sharees: Vec<_> = invite.sharees().collect();
        assert_eq!(sharees.len(), 1);
        assert_eq!(sharees[0].1, ShareAccess::Read);
        assert_eq!(sharees[0].2, InviteStatus::Declined);
        assert_eq!(invite.omitted, 1);
        assert!(!invite.is_complete());
        // Declined, invalid or without access: nothing to remove, the
        // invitation is complete.
        for user in [
            "<CS:invite-declined/><CS:access><CS:read-write/></CS:access>",
            "<CS:invite-invalid/><CS:access><CS:read/></CS:access>",
            "<CS:invite-accepted/><CS:access/>",
        ] {
            let other = xml.replace(
                "<CS:invite-noresponse/><CS:access><CS:read-write/></CS:access>",
                user,
            );
            let invite = props(&other).invite.unwrap();
            assert!(invite.is_complete(), "{user}");
        }
    }

    #[test]
    fn too_many_sharees_are_counted() {
        let users = "<CS:user><D:href>mailto:carol@example.com</D:href><CS:invite-accepted/><CS:access><CS:read/></CS:access></CS:user>"
            .repeat(MAX_SHAREES + 1);
        let xml = format!(
            r#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:" xmlns:CS="http://calendarserver.org/ns/"><D:response><D:href>/c/</D:href><D:propstat><D:prop><CS:invite>{users}</CS:invite></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>"#
        );
        let invite = props(&xml).invite.unwrap();
        assert_eq!(invite.users.len(), MAX_SHAREES);
        assert_eq!(invite.omitted, 1);
    }

    /// `count` pieces of XML, numbered.
    fn numbered(count: usize, piece: impl Fn(usize) -> String) -> String {
        (0..count).map(piece).collect::<Vec<_>>().concat()
    }

    #[test]
    fn invite_keeps_more_than_256_users() {
        let users = numbered(300, |n| {
            format!(
                "<CS:user><D:href>mailto:u{n}@example.com</D:href><CS:invite-accepted/><CS:access><CS:read/></CS:access></CS:user>"
            )
        });
        let xml = format!(
            r#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:" xmlns:CS="http://calendarserver.org/ns/"><D:response><D:href>/c/</D:href><D:propstat><D:prop><CS:invite>{users}</CS:invite></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>"#
        );
        let invite = props(&xml).invite.unwrap();
        assert_eq!((invite.users.len(), invite.omitted), (300, 0));
    }

    #[test]
    fn oc_invite_keeps_more_than_256_users() {
        let users = numbered(300, |n| {
            format!(
                "<O:user><D:href>principal:principals/users/u{n}</D:href><O:invite-accepted/></O:user>"
            )
        });
        let xml = format!(
            r#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:" xmlns:O="http://owncloud.org/ns"><D:response><D:href>/c/</D:href><D:propstat><D:prop><O:invite>{users}</O:invite></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>"#
        );
        let invite = props(&xml).oc_invite.unwrap();
        assert_eq!((invite.users.len(), invite.omitted), (300, 0));
    }

    #[test]
    fn acl_with_300_aces_is_read_and_rewritable() {
        let aces = numbered(300, |n| {
            format!(
                "<D:ace><D:principal><D:href>/dav/pal/u{n}/</D:href></D:principal><D:grant><D:privilege><D:read/></D:privilege></D:grant></D:ace>"
            )
        });
        let xml = format!(
            r#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:"><D:response><D:href>/c/</D:href><D:propstat><D:prop><D:acl>{aces}</D:acl></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>"#
        );
        let acl = props(&xml).acl.unwrap();
        assert_eq!(acl.aces.len(), 300);
        assert!(acl.is_rewritable(&base()));
        assert!(acl_body(acl.editable()).is_ok());
    }

    #[test]
    fn rewritable_means_writable() {
        let acl_of = |inner: &str| {
            props(&format!(
                r#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:"><D:response><D:href>/c/</D:href><D:propstat><D:prop><D:acl>{inner}</D:acl></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>"#
            ))
            .acl
            .unwrap()
        };
        let collection = base();
        // An empty grant cannot be written back; protected, it is not sent.
        let empty = "<D:ace><D:principal><D:all/></D:principal><D:grant/></D:ace>";
        assert!(!acl_of(empty).is_rewritable(&collection));
        let protected =
            "<D:ace><D:principal><D:all/></D:principal><D:grant/><D:protected/></D:ace>";
        assert!(acl_of(protected).is_rewritable(&collection));
        // Several privileges in one `DAV:privilege`: all kept, a deny is
        // never lost.
        let deny = "<D:ace><D:principal><D:all/></D:principal><D:deny><D:privilege><D:read/><D:write/></D:privilege></D:deny></D:ace>";
        let acl = acl_of(deny);
        assert_eq!(acl.aces[0].privileges.len(), 2);
        assert!(acl.is_rewritable(&collection));
        let body = acl_body(acl.editable()).unwrap();
        assert!(body.contains("<D:deny><D:privilege><D:read/></D:privilege><D:privilege><D:write/></D:privilege></D:deny>"));
        // A principal on another origin would name someone else here.
        let foreign = "<D:ace><D:principal><D:href>https://other.example/p/bob/</D:href></D:principal><D:grant><D:privilege><D:read/></D:privilege></D:grant></D:ace>";
        assert!(!acl_of(foreign).is_rewritable(&collection));
        let local = foreign.replace("https://other.example", "http://localhost:8080");
        assert!(acl_of(&local).is_rewritable(&collection));
    }

    #[test]
    fn hostile_acls_never_fail_the_listing() {
        let ace = |inner: &str| {
            format!(
                r#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:"><D:response><D:href>/c/</D:href><D:propstat><D:prop><D:acl>{inner}</D:acl></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>"#
            )
        };
        // An href to another host is resolved (and checked by the caller);
        // one that does not resolve is an unknown principal.
        let p = props(&ace(
            "<D:ace><D:principal><D:href>/a/../b/</D:href></D:principal><D:grant><D:privilege><D:read/></D:privilege></D:grant></D:ace>",
        ));
        let acl = p.acl.unwrap();
        assert_eq!(acl.aces[0].principal, AcePrincipal::Unknown);
        assert!(!acl.is_rewritable(&base()));
        // Grant and deny in one ACE, a privilege with a bad name, too many
        // ACEs: no ACL at all.
        for inner in [
            "<D:ace><D:principal><D:all/></D:principal><D:grant><D:privilege><D:read/></D:privilege></D:grant><D:deny><D:privilege><D:read/></D:privilege></D:deny></D:ace>".to_owned(),
            "<D:ace><D:principal><D:all/></D:principal></D:ace>".to_owned(),
            "<D:ace><D:principal><D:all/></D:principal><D:grant><D:privilege/></D:grant></D:ace>".to_owned(),
            "<D:ace><D:principal><D:all/></D:principal><D:grant>".to_owned()
                + &"<D:privilege><D:read/></D:privilege>".repeat(MAX_PRIVILEGES + 1)
                + "</D:grant></D:ace>",
            "<D:ace><D:principal><D:all/></D:principal><D:grant><D:privilege><D:read/></D:privilege></D:grant></D:ace>"
                .repeat(MAX_ACES + 1),
        ] {
            assert_eq!(props(&ace(&inner)).acl, None, "{inner}");
        }
    }

    #[test]
    fn acl_round_trip_keeps_unknown_privileges() {
        let xml = r#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:" xmlns:X="urn:example:priv"><D:response><D:href>/c/</D:href><D:propstat><D:prop><D:acl><D:ace><D:invert><D:principal><D:self/></D:principal></D:invert><D:deny><D:privilege><X:special-thing/></D:privilege></D:deny></D:ace><D:ace><D:principal><D:property><D:owner/></D:property></D:principal><D:grant><D:privilege><D:all/></D:privilege></D:grant><D:protected/></D:ace><D:ace><D:principal><D:authenticated/></D:principal><D:grant><D:privilege><D:read/></D:privilege></D:grant></D:ace></D:acl></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>"#;
        let acl = props(xml).acl.unwrap();
        assert!(acl.is_rewritable(&base()));
        let body = acl_body(acl.editable()).unwrap();
        assert!(body.contains(r#"<D:ace><D:invert><D:principal><D:self/></D:principal></D:invert><D:deny><D:privilege><X:special-thing xmlns:X="urn:example:priv"/></D:privilege></D:deny></D:ace>"#));
        assert!(body.contains("<D:ace><D:principal><D:authenticated/></D:principal><D:grant><D:privilege><D:read/></D:privilege></D:grant></D:ace>"));
        assert!(!body.contains("D:owner"));
        // It parses back to the same editable ACEs.
        let again = format!(
            r#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:"><D:response><D:href>/c/</D:href><D:propstat><D:prop>{}</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>"#,
            body.split_once("?>").unwrap().1
        );
        let parsed = props(&again).acl.unwrap();
        assert_eq!(parsed.aces, acl.editable().cloned().collect::<Vec<_>>());
        // An empty ACL is a valid body: unshare everything.
        assert!(acl_body(&[]).unwrap().ends_with("<D:acl xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\" xmlns:CR=\"urn:ietf:params:xml:ns:carddav\" xmlns:CS=\"http://calendarserver.org/ns/\" xmlns:I=\"http://apple.com/ns/ical/\" xmlns:O=\"http://owncloud.org/ns\" xmlns:S=\"http://sabredav.org/ns\" xmlns:NC=\"http://nextcloud.com/ns\"></D:acl>"));
        let unknown = Ace {
            principal: AcePrincipal::Unknown,
            ..Ace::grant(base(), &[Privilege::Read])
        };
        assert_eq!(acl_body([&unknown]), Err(EncodeError::Range));
    }

    #[test]
    fn share_bodies() {
        let bob = Sharee::Address(CalendarAddress::parse("bob@example.com").unwrap());
        let carol = Sharee::Principal(Href::resolve("/principals/carol/", &base()).unwrap());
        let body = share_body(&[(bob.clone(), ShareAccess::ReadWrite)], &[carol]).unwrap();
        assert!(body.contains(concat!("<CS:share", r#" xmlns:D="DAV:""#)));
        assert!(body.contains("<CS:set><D:href>mailto:bob@example.com</D:href><CS:read-write/></CS:set><CS:remove><D:href>/principals/carol/</D:href></CS:remove></CS:share>"));
        assert_eq!(share_body(&[], &[]), Err(EncodeError::Range));
        let many = vec![(bob.clone(), ShareAccess::Read); MAX_SHARE_REQUEST + 1];
        assert_eq!(share_body(&many, &[]), Err(EncodeError::Range));
        assert!(share_body(&many[1..], &[]).is_ok());
    }

    /// Nextcloud 34: alice's address book shared with bob (read-write)
    /// and a group (read), as seen live (the ACL left out).
    const NEXTCLOUD_OWNER: &str = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:s="http://sabredav.org/ns" xmlns:card="urn:ietf:params:xml:ns:carddav" xmlns:oc="http://owncloud.org/ns" xmlns:nc="http://nextcloud.org/ns"><d:response><d:href>/remote.php/dav/addressbooks/users/alice/probe-book/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><card:addressbook/></d:resourcetype><d:displayname>Probe book</d:displayname><d:current-user-privilege-set><d:privilege><d:write-properties/></d:privilege><d:privilege><d:write-acl/></d:privilege><d:privilege><d:write/></d:privilege><d:privilege><d:write-content/></d:privilege><d:privilege><d:unlock/></d:privilege><d:privilege><d:bind/></d:privilege><d:privilege><d:unbind/></d:privilege><d:privilege><d:read/></d:privilege><d:privilege><d:read-acl/></d:privilege><d:privilege><d:read-current-user-privilege-set/></d:privilege></d:current-user-privilege-set><d:owner><d:href>/remote.php/dav/principals/users/alice/</d:href></d:owner><oc:invite><oc:user><d:href>principal:principals/groups/admin</d:href><oc:common-name>admin</oc:common-name><oc:invite-accepted/><oc:access><oc:read/></oc:access></oc:user><oc:user><d:href>principal:principals/users/bob</d:href><oc:common-name>bob</oc:common-name><oc:invite-accepted/><oc:access><oc:read-write/></oc:access></oc:user></oc:invite></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat><d:propstat><d:prop><card:addressbook-description/><x1:invite xmlns:x1="http://calendarserver.org/ns/"/><x1:allowed-sharing-modes xmlns:x1="http://calendarserver.org/ns/"/></d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat></d:response></d:multistatus>"#;

    /// Nextcloud 34: bob's view of a calendar alice shares with him
    /// (read-only), as seen live (the ACL left out).
    const NEXTCLOUD_SHAREE: &str = r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:" xmlns:s="http://sabredav.org/ns" xmlns:cal="urn:ietf:params:xml:ns:caldav" xmlns:cs="http://calendarserver.org/ns/" xmlns:oc="http://owncloud.org/ns" xmlns:nc="http://nextcloud.org/ns"><d:response><d:href>/remote.php/dav/calendars/bob/probe-share_shared_by_alice/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><cal:calendar/></d:resourcetype><d:displayname>Probe share (alice)</d:displayname><d:owner><d:href>/remote.php/dav/principals/users/alice/</d:href></d:owner><d:current-user-privilege-set><d:privilege><d:write-properties/></d:privilege><d:privilege><d:read/></d:privilege><d:privilege><d:read-acl/></d:privilege><d:privilege><d:read-current-user-privilege-set/></d:privilege><d:privilege><cal:read-free-busy/></d:privilege></d:current-user-privilege-set><cs:allowed-sharing-modes/><oc:invite/></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat><d:propstat><d:prop><cs:invite/></d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat></d:response></d:multistatus>"#;

    fn nextcloud_root() -> Href {
        let base = Href::from_url(&Url::parse("http://127.0.0.1:5234/").unwrap()).unwrap();
        Href::resolve("/remote.php/dav/", &base).unwrap()
    }

    #[test]
    fn nextcloud_owner() {
        let p = props(NEXTCLOUD_OWNER);
        assert!(p.has(Property::OcInvite) && !p.has(Property::Invite));
        let invite = p.oc_invite.unwrap();
        let shares: Vec<(String, ShareAccess, InviteStatus)> = invite
            .users
            .iter()
            .map(|u| (u.principal.to_string(), u.access, u.status))
            .collect();
        assert_eq!(
            shares,
            [
                (
                    "principal:principals/groups/admin".to_owned(),
                    ShareAccess::Read,
                    InviteStatus::Accepted
                ),
                (
                    "principal:principals/users/bob".to_owned(),
                    ShareAccess::ReadWrite,
                    InviteStatus::Accepted
                ),
            ]
        );
        assert!(p.privileges.unwrap().allows(Privilege::WriteAcl));
    }

    #[test]
    fn nextcloud_sharee() {
        let p = props(NEXTCLOUD_SHAREE);
        assert_eq!(
            p.owner.unwrap().path(),
            "/remote.php/dav/principals/users/alice/"
        );
        // An empty invite: the sharee does not see the others.
        assert_eq!(p.oc_invite, Some(OcInvite::default()));
        assert_eq!(p.sharing_modes, Some(SharingModes::default()));
        let set = p.privileges.unwrap();
        assert!(set.can_read() && !set.can_write_content());
    }

    #[test]
    fn nextcloud_principals() {
        let root = nextcloud_root();
        let alice = Href::resolve("/remote.php/dav/principals/users/alice/", &root).unwrap();
        assert_eq!(OcPrincipal::dav_root(&alice), Some(root.clone()));
        let named = OcPrincipal::of(&alice, &root).unwrap();
        assert_eq!(named.to_string(), "principal:principals/users/alice");
        assert_eq!(named.href(&root), Some(alice.clone()));
        // A user id with characters a path encodes: decoded in the name,
        // encoded in the href, and back.
        let odd = OcPrincipal::parse("principal:principals/users/Bob Smith@x").unwrap();
        let at = odd.href(&root).unwrap();
        assert_eq!(at.path(), "/remote.php/dav/principals/users/Bob%20Smith@x/");
        assert_eq!(OcPrincipal::of(&at, &root), Some(odd));
        // Hostile or foreign names.
        for text in [
            "principal:",
            "principal:principals",
            "principal:principals/",
            "principal:principals//bob",
            "principal:principals/users/..",
            "principal:principals/users/.",
            "principal:principals/a\\b",
            "principal:principals/users/b\nob",
            "principal:/principals/users/bob",
            "principal:calendars/alice/x",
            "/remote.php/dav/principals/users/bob/",
            "mailto:bob@nebulamail.test",
        ] {
            assert_eq!(OcPrincipal::parse(text), None, "{text:?}");
        }
        let long = format!("principal:principals/{}", "a/".repeat(Href::MAX_SEGMENTS));
        assert_eq!(OcPrincipal::parse(&long), None);
        // Not below the root, or not a principal: no name.
        let other = Href::resolve("/remote.php/dav/calendars/alice/", &root).unwrap();
        assert_eq!(OcPrincipal::of(&other, &root), None);
        assert_eq!(OcPrincipal::of(&root, &root), None);
        let elsewhere = Href::from_url(
            &Url::parse("http://127.0.0.1:9999/remote.php/dav/principals/users/bob/").unwrap(),
        )
        .unwrap();
        assert_eq!(OcPrincipal::of(&elsewhere, &root), None);
        assert_eq!(OcPrincipal::dav_root(&other), None, "no principals segment");
    }

    #[test]
    fn hostile_oc_invites() {
        let invite = |inner: &str| {
            props(&format!(
                r#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:" xmlns:O="http://owncloud.org/ns"><D:response><D:href>/c/</D:href><D:propstat><D:prop><O:invite>{inner}</O:invite></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>"#
            ))
            .oc_invite
        };
        // Bad names are left out and counted; no access means read.
        let found = invite(
            "<O:user><D:href>principal:principals/../x</D:href></O:user><O:user><D:href>mailto:x@y.z</D:href></O:user><O:user><D:href>principal:principals/users/bob</D:href></O:user><O:user><D:href>mailto:z@y.z</D:href><O:invite-declined/></O:user>",
        )
        .unwrap();
        assert_eq!(found.users.len(), 1);
        assert_eq!(found.users[0].access, ShareAccess::Read);
        assert_eq!(found.users[0].status, InviteStatus::NoResponse);
        assert_eq!(found.omitted, 2, "the declined one has no access");
        assert!(!found.is_complete());
        // Too many: the first ones kept, the rest counted.
        let many = "<O:user><D:href>principal:principals/users/bob</D:href></O:user>"
            .repeat(MAX_SHAREES + 1);
        let found = invite(&many).unwrap();
        assert_eq!((found.users.len(), found.omitted), (MAX_SHAREES, 1));
    }

    #[test]
    fn oc_share_bodies() {
        let bob = OcPrincipal::parse("principal:principals/users/bob").unwrap();
        let group = OcPrincipal::parse("principal:principals/groups/a&b").unwrap();
        let body = oc_share_body(
            &[(bob.clone(), ShareAccess::Read)],
            std::slice::from_ref(&group),
        )
        .unwrap();
        assert!(body.contains("<O:share xmlns:D=\"DAV:\""));
        assert!(body.contains("<O:set><D:href>principal:principals/users/bob</D:href><O:read/></O:set><O:remove><D:href>principal:principals/groups/a&amp;b</D:href></O:remove></O:share>"));
        assert_eq!(oc_share_body(&[], &[]), Err(EncodeError::Range));
        let many = vec![(bob.clone(), ShareAccess::Read); MAX_OC_SHARE_REQUEST + 1];
        assert_eq!(oc_share_body(&many, &[]), Err(EncodeError::Range));
        assert!(oc_share_body(&many[1..], &[]).is_ok());
    }

    #[test]
    fn names() {
        assert!(QualifiedName::new("DAV:", "1read").is_none());
        assert!(QualifiedName::new("", "read").is_none());
        assert!(QualifiedName::new("DAV:", &"a".repeat(65)).is_none());
        assert_eq!(
            QualifiedName::of(Privilege::Share).privilege(),
            Some(Privilege::Share)
        );
        assert_eq!(
            QualifiedName::new("urn:x", "read").unwrap().privilege(),
            None
        );
    }
}
