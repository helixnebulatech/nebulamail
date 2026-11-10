// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Server capabilities.

use std::fmt;

/// A SASL mechanism advertised as `AUTH=<name>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AuthMechanism {
    /// `PLAIN` (RFC 4616).
    Plain,
    /// `XOAUTH2` (Google, Microsoft).
    XOAuth2,
    /// `OAUTHBEARER` (RFC 7628).
    OAuthBearer,
    /// Any other mechanism, upper-cased.
    Other(String),
}

impl AuthMechanism {
    fn parse(name: &str) -> Self {
        match name.to_ascii_uppercase().as_str() {
            "PLAIN" => Self::Plain,
            "XOAUTH2" => Self::XOAuth2,
            "OAUTHBEARER" => Self::OAuthBearer,
            other => Self::Other(other.to_owned()),
        }
    }

    /// The mechanism name as sent in `AUTHENTICATE`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Plain => "PLAIN",
            Self::XOAuth2 => "XOAUTH2",
            Self::OAuthBearer => "OAUTHBEARER",
            Self::Other(name) => name,
        }
    }
}

/// A capability. The ones we use are variants; the rest are `Other`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Capability {
    /// `IMAP4rev1` (RFC 3501).
    Imap4Rev1,
    /// `IMAP4rev2` (RFC 9051).
    Imap4Rev2,
    /// `STARTTLS`.
    StartTls,
    /// `LOGINDISABLED`: the LOGIN command must not be used.
    LoginDisabled,
    /// `AUTH=<mechanism>`.
    Auth(AuthMechanism),
    /// `SASL-IR` (RFC 4959): initial response in the AUTHENTICATE line.
    SaslIr,
    /// `IDLE` (RFC 2177).
    Idle,
    /// `NAMESPACE` (RFC 2342).
    Namespace,
    /// `ID` (RFC 2971).
    Id,
    /// `ENABLE` (RFC 5161).
    Enable,
    /// `CONDSTORE` (RFC 7162).
    CondStore,
    /// `QRESYNC` (RFC 7162).
    QResync,
    /// `UIDPLUS` (RFC 4315).
    UidPlus,
    /// `MOVE` (RFC 6851).
    Move,
    /// `LITERAL+` (RFC 7888).
    LiteralPlus,
    /// `LITERAL-` (RFC 7888): non-synchronizing literals up to 4096 bytes.
    LiteralMinus,
    /// `UTF8=ACCEPT` (RFC 6855).
    Utf8Accept,
    /// `SPECIAL-USE` (RFC 6154).
    SpecialUse,
    /// `CREATE-SPECIAL-USE` (RFC 6154).
    CreateSpecialUse,
    /// `LIST-EXTENDED` (RFC 5258).
    ListExtended,
    /// `LIST-STATUS` (RFC 5819).
    ListStatus,
    /// `OBJECTID` (RFC 8474): EMAILID, THREADID, MAILBOXID.
    ObjectId,
    /// `OBJECTID+` (IETF draft, implemented by Stalwart instead of RFC
    /// 8474): after `ENABLE OBJECTID+`, `FETCH (OBJECTID)` answers
    /// `OBJECTID (EMAILID x THREADID y)`.
    ObjectIdPlus,
    /// `PREVIEW` (RFC 8970).
    Preview,
    /// `QUOTA` (RFC 9208, RFC 2087).
    Quota,
    /// `UNSELECT` (RFC 3691).
    Unselect,
    /// `CHILDREN` (RFC 3348).
    Children,
    /// `ESEARCH` (RFC 4731).
    ESearch,
    /// `BINARY` (RFC 3516): decoded parts and `literal8` in APPEND.
    Binary,
    /// `NOTIFY` (RFC 5465): events of other mailboxes on one connection.
    Notify,
    /// `MULTIAPPEND` (RFC 3502): several messages in one APPEND.
    MultiAppend,
    /// `X-GM-EXT-1`: Gmail labels and ids.
    GmailExt1,
    /// Any other capability, upper-cased.
    Other(String),
}

impl Capability {
    /// Parses one capability atom (case-insensitive).
    #[must_use]
    pub fn parse(atom: &str) -> Self {
        let upper = atom.to_ascii_uppercase();
        if let Some(mechanism) = upper.strip_prefix("AUTH=") {
            return Self::Auth(AuthMechanism::parse(mechanism));
        }
        match upper.as_str() {
            "IMAP4REV1" => Self::Imap4Rev1,
            "IMAP4REV2" => Self::Imap4Rev2,
            "STARTTLS" => Self::StartTls,
            "LOGINDISABLED" => Self::LoginDisabled,
            "SASL-IR" => Self::SaslIr,
            "IDLE" => Self::Idle,
            "NAMESPACE" => Self::Namespace,
            "ID" => Self::Id,
            "ENABLE" => Self::Enable,
            "CONDSTORE" => Self::CondStore,
            "QRESYNC" => Self::QResync,
            "UIDPLUS" => Self::UidPlus,
            "MOVE" => Self::Move,
            "LITERAL+" => Self::LiteralPlus,
            "LITERAL-" => Self::LiteralMinus,
            "UTF8=ACCEPT" => Self::Utf8Accept,
            "SPECIAL-USE" => Self::SpecialUse,
            "CREATE-SPECIAL-USE" => Self::CreateSpecialUse,
            "LIST-EXTENDED" => Self::ListExtended,
            "LIST-STATUS" => Self::ListStatus,
            "OBJECTID" => Self::ObjectId,
            "OBJECTID+" => Self::ObjectIdPlus,
            "PREVIEW" => Self::Preview,
            "QUOTA" => Self::Quota,
            "UNSELECT" => Self::Unselect,
            "CHILDREN" => Self::Children,
            "ESEARCH" => Self::ESearch,
            "BINARY" => Self::Binary,
            "NOTIFY" => Self::Notify,
            "MULTIAPPEND" => Self::MultiAppend,
            "X-GM-EXT-1" => Self::GmailExt1,
            _ => Self::Other(upper),
        }
    }

    /// The capability as sent on the wire.
    #[must_use]
    pub fn as_wire(&self) -> String {
        let text = match self {
            Self::Imap4Rev1 => "IMAP4rev1",
            Self::Imap4Rev2 => "IMAP4rev2",
            Self::StartTls => "STARTTLS",
            Self::LoginDisabled => "LOGINDISABLED",
            Self::Auth(mechanism) => return format!("AUTH={}", mechanism.as_str()),
            Self::SaslIr => "SASL-IR",
            Self::Idle => "IDLE",
            Self::Namespace => "NAMESPACE",
            Self::Id => "ID",
            Self::Enable => "ENABLE",
            Self::CondStore => "CONDSTORE",
            Self::QResync => "QRESYNC",
            Self::UidPlus => "UIDPLUS",
            Self::Move => "MOVE",
            Self::LiteralPlus => "LITERAL+",
            Self::LiteralMinus => "LITERAL-",
            Self::Utf8Accept => "UTF8=ACCEPT",
            Self::SpecialUse => "SPECIAL-USE",
            Self::CreateSpecialUse => "CREATE-SPECIAL-USE",
            Self::ListExtended => "LIST-EXTENDED",
            Self::ListStatus => "LIST-STATUS",
            Self::ObjectId => "OBJECTID",
            Self::ObjectIdPlus => "OBJECTID+",
            Self::Preview => "PREVIEW",
            Self::Quota => "QUOTA",
            Self::Unselect => "UNSELECT",
            Self::Children => "CHILDREN",
            Self::ESearch => "ESEARCH",
            Self::Binary => "BINARY",
            Self::Notify => "NOTIFY",
            Self::MultiAppend => "MULTIAPPEND",
            Self::GmailExt1 => "X-GM-EXT-1",
            Self::Other(name) => name,
        };
        text.to_owned()
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_wire())
    }
}

/// The capabilities a server advertised.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities(Vec<Capability>);

impl Capabilities {
    /// Builds the set from parsed capabilities (duplicates removed).
    #[must_use]
    pub fn new(list: impl IntoIterator<Item = Capability>) -> Self {
        let mut unique: Vec<Capability> = Vec::new();
        for capability in list {
            if !unique.contains(&capability) {
                unique.push(capability);
            }
        }
        Self(unique)
    }

    /// Parses capability atoms.
    pub fn parse_atoms<'a>(atoms: impl IntoIterator<Item = &'a str>) -> Self {
        Self::new(atoms.into_iter().map(Capability::parse))
    }

    /// Whether the server supports `capability`: it listed it, or it is
    /// part of the base `IMAP4rev2` protocol and the server speaks only
    /// `IMAP4rev2` ([`Self::is_rev2_only`]).
    ///
    /// RFC 9051 Appendix E item 2 folds these extensions into `IMAP4rev2`,
    /// so a server that only announces `IMAP4rev2` need not list them
    /// (RFC 9051 §6.1.1, §7.2.2: "Client implementations SHOULD NOT
    /// require any capability name other than `IMAP4rev2`"). A server with
    /// both `IMAP4rev1` and `IMAP4rev2` behaves as RFC 3501 until the
    /// client sends `ENABLE IMAP4rev2` (RFC 9051 §6.1.1, §6.3.1), which we
    /// don't: there only the listed ones count.
    #[must_use]
    pub fn has(&self, capability: &Capability) -> bool {
        self.has_listed(capability) || (self.is_rev2_only() && in_rev2_base(capability))
    }

    /// Whether the server listed `capability` literally.
    #[must_use]
    pub fn has_listed(&self, capability: &Capability) -> bool {
        self.0.contains(capability)
    }

    /// Whether the server announced `IMAP4rev2` without `IMAP4rev1`: the
    /// connection then follows RFC 9051 (UTF-8 mailbox names, §5.1; no
    /// LSUB, Appendix E item 19; the base extensions of Appendix E item 2).
    #[must_use]
    pub fn is_rev2_only(&self) -> bool {
        self.has_listed(&Capability::Imap4Rev2) && !self.has_listed(&Capability::Imap4Rev1)
    }

    /// Whether the server advertised `AUTH=<mechanism>`.
    #[must_use]
    pub fn has_auth(&self, mechanism: &AuthMechanism) -> bool {
        self.0
            .iter()
            .any(|capability| matches!(capability, Capability::Auth(m) if m == mechanism))
    }

    /// Whether the server speaks `IMAP4rev1` or `IMAP4rev2`.
    #[must_use]
    pub fn is_imap4(&self) -> bool {
        self.has(&Capability::Imap4Rev1) || self.has(&Capability::Imap4Rev2)
    }

    /// All capabilities.
    pub fn iter(&self) -> impl Iterator<Item = &Capability> {
        self.0.iter()
    }

    /// Whether the set is empty (nothing advertised yet).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The extensions RFC 9051 Appendix E item 2 folds into `IMAP4rev2` (that we
/// know). Not included, because only part of them was folded in:
/// `BINARY` (only its FETCH side: APPEND with `literal8` still needs the
/// capability) and `SPECIAL-USE` (only the mailbox attributes, not the
/// `RETURN (SPECIAL-USE)` option of RFC 6154 §5.1). `SEARCHRES` has no
/// variant because we don't use it.
fn in_rev2_base(capability: &Capability) -> bool {
    matches!(
        capability,
        Capability::Namespace
            | Capability::Unselect
            | Capability::UidPlus
            | Capability::ESearch
            | Capability::Enable
            | Capability::Idle
            | Capability::SaslIr
            | Capability::ListExtended
            | Capability::ListStatus
            | Capability::Move
            | Capability::LiteralMinus
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_capabilities() {
        let caps = Capabilities::parse_atoms([
            "IMAP4rev1",
            "auth=plain",
            "LITERAL+",
            "X-FOO",
            "IDLE",
            "IDLE",
        ]);
        assert!(caps.is_imap4());
        assert!(caps.has_auth(&AuthMechanism::Plain));
        assert!(caps.has(&Capability::LiteralPlus));
        assert!(caps.has(&Capability::Other("X-FOO".to_owned())));
        assert_eq!(caps.iter().count(), 5);
        assert_eq!(Capability::parse("utf8=accept"), Capability::Utf8Accept);
    }

    #[test]
    fn rev2_only_implies_the_base_extensions() {
        let rev2 = Capabilities::parse_atoms(["IMAP4rev2", "AUTH=PLAIN"]);
        assert!(rev2.is_rev2_only());
        for implied in [
            Capability::Namespace,
            Capability::Unselect,
            Capability::UidPlus,
            Capability::ESearch,
            Capability::Enable,
            Capability::Idle,
            Capability::SaslIr,
            Capability::ListExtended,
            Capability::ListStatus,
            Capability::Move,
            Capability::LiteralMinus,
        ] {
            assert!(rev2.has(&implied), "{implied}");
            assert!(!rev2.has_listed(&implied), "{implied}");
        }
        // Only partly folded in, or not at all.
        for not_implied in [
            Capability::Binary,
            Capability::SpecialUse,
            Capability::Utf8Accept,
            Capability::LiteralPlus,
            Capability::CondStore,
            Capability::Quota,
        ] {
            assert!(!rev2.has(&not_implied), "{not_implied}");
        }
    }

    #[test]
    fn rev1_and_rev2_only_count_what_is_listed() {
        let both = Capabilities::parse_atoms(["IMAP4rev1", "IMAP4rev2", "IDLE"]);
        assert!(!both.is_rev2_only());
        assert!(both.has(&Capability::Idle));
        assert!(!both.has(&Capability::Move));
        assert!(!both.has(&Capability::Unselect));
        let rev1 = Capabilities::parse_atoms(["IMAP4rev1"]);
        assert!(!rev1.is_rev2_only());
        assert!(!rev1.has(&Capability::Move));
    }
}
