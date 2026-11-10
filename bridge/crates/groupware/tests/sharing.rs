// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Sharing calendars and address books
//! against a fake CalDAV/CardDAV server (hyper, in memory) shaped like
//! the real ones: Stalwart (WebDAV ACL; what others share is listed in
//! the user's home by "assisted discovery"), sabre/dav as in Baïkal
//! (Calendar Server sharing of calendars, `CS:shared` copies in the
//! user's home; address books not shareable), Nextcloud (its own
//! `O:share` for calendars and address books, principals found only by
//! searching) and Radicale (nothing).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::format_push_string,
    clippy::string_slice,
    reason = "integration test helpers"
)]

use std::convert::Infallible;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http::Response;
use http_body_util::{BodyExt as _, Full};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use nebula_crypto::{AccountId as KeyId, AccountKey, DataKey};
use nebula_dav::DavServer;
use nebula_groupware::sharing::{ShareError, ShareUpdate};
use nebula_groupware::{DavStatus, Groupware, GroupwareAccount, GroupwareConfig, GroupwareError};
use nebula_store::{
    AccountId, Collection, CollectionKind, NewAccount, Rights, ShareMode, Sharee, Store, UnixTime,
};
use net_guard::{DnsError, DomainName, LookupFuture, Resolver};
use secrecy::SecretString;
use tokio::net::TcpListener;

type Body = Full<Bytes>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Stalwart,
    Sabre,
    Radicale,
    Nextcloud,
}

struct State {
    kind: Kind,
    /// The editable ACEs of alice's "work" calendar (Stalwart), as the
    /// last ACL request wrote them.
    acl: String,
    /// The sharees of "work" (sabre/dav): address and access.
    sharees: Vec<(String, &'static str)>,
    /// Nextcloud's sharees: collection path, `principal:` href, read-write.
    oc_sharees: Vec<(String, String, bool)>,
    /// A server that answers yes to a removal and keeps the sharee.
    ignore_removes: bool,
    /// More `CS:user` of "work" (sabre/dav), as they are.
    extra_users: String,
    /// How many more calendars bob shares with alice (Stalwart), or dave
    /// (sabre/dav: copies in alice's home, `CS:shared`, organizer
    /// `urn:x-uid:dave`, which the bridge cannot resolve).
    shared: usize,
    /// A pause before answering the read of "work"'s ACL (Stalwart).
    acl_delay: Option<Duration>,
    /// A pause before answering a sharing `POST`.
    post_delay: Option<Duration>,
    /// More principals in the directory: `/dav/principals/u{n}/`, with
    /// `mailto:u{n}@example.com`.
    people: usize,
    /// The sharing `POST` with this number (from 0) fails with a 500.
    fail_post_at: Option<usize>,
    /// After the first sharing `POST`, this sharee (sabre/dav) becomes
    /// read-write, as another client would do.
    upgrade_after_post: Option<String>,
    /// Sharing `POST`s answered.
    posts: usize,
    /// The editable ACEs of alice's "people" address book (Stalwart).
    book_acl: String,
    /// More `CS:user` of carol's copy in alice's home (sabre/dav).
    copy_users: String,
    /// The ACEs of bob's `shared` calendars (Stalwart shows them to a
    /// sharee with `write-acl`).
    shared_acl: String,
    requests: Vec<String>,
}

impl State {
    fn last(&self, method: &str) -> Option<&String> {
        self.requests.iter().rev().find(|r| r.starts_with(method))
    }
}

fn reply(kind: Kind, status: u16, body: &str) -> Response<Body> {
    let dav = match kind {
        Kind::Stalwart => {
            "1, 2, 3, access-control, extended-mkcol, calendar-access, calendar-auto-schedule, addressbook"
        }
        Kind::Sabre => {
            "1, 3, extended-mkcol, access-control, calendar-access, calendar-auto-schedule, resource-sharing, calendarserver-sharing, addressbook"
        }
        Kind::Radicale => "1, 2, 3, calendar-access, addressbook, extended-mkcol",
        Kind::Nextcloud => {
            "1, 3, extended-mkcol, access-control, calendarserver-principal-property-search, oc-resource-sharing, calendar-access, calendar-proxy, calendar-auto-schedule, calendarserver-sharing, addressbook, nc-paginate"
        }
    };
    Response::builder()
        .status(status)
        .header("content-type", "application/xml; charset=utf-8")
        .header("dav", dav)
        .body(Full::new(Bytes::from(body.to_owned())))
        .unwrap()
}

fn multistatus(inner: &str) -> String {
    // The prefixes our own request bodies use, so an ACL written by the
    // bridge can be sent back as it is.
    format!(
        r#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:CR="urn:ietf:params:xml:ns:carddav" xmlns:CS="http://calendarserver.org/ns/" xmlns:O="http://owncloud.org/ns">{inner}</D:multistatus>"#
    )
}

fn response(href: &str, props: &str) -> String {
    format!(
        "<D:response><D:href>{href}</D:href><D:propstat><D:prop>{props}</D:prop>\
         <D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>"
    )
}

fn principal(path: &str, name: &str, address: &str) -> String {
    response(
        path,
        &format!(
            "<D:resourcetype><D:principal/><D:collection/></D:resourcetype>\
             <D:displayname>{name}</D:displayname>\
             <C:calendar-user-address-set><D:href>mailto:{address}</D:href></C:calendar-user-address-set>"
        ),
    )
}

const ALL: &str = "<D:current-user-privilege-set><D:privilege><D:all/></D:privilege></D:current-user-privilege-set>";
const ALL_AND_SHARE: &str = "<D:current-user-privilege-set><D:privilege><D:all/></D:privilege><D:privilege><D:share/></D:privilege></D:current-user-privilege-set>";
const READ: &str = "<D:current-user-privilege-set><D:privilege><D:read/></D:privilege></D:current-user-privilege-set>";

/// What is between the root tags of a body the bridge wrote.
fn inner(body: &str, root: &str) -> String {
    let start = body.find('>').map_or(0, |i| i + 1);
    let start = body[start..].find('>').map_or(start, |i| start + i + 1);
    let end = body.rfind(&format!("</{root}>")).unwrap_or(body.len());
    body[start..end].to_owned()
}

/// The `CS:set` and `CS:remove` of a `CS:share` body (the bridge's own
/// shape: href, then the access).
fn apply_share(state: &mut State, body: &str) {
    for set in body.split("<CS:set>").skip(1) {
        let href = set
            .split("<D:href>")
            .nth(1)
            .and_then(|s| s.split("</D:href>").next())
            .unwrap()
            .to_owned();
        let access = if set.starts_with(&format!("<D:href>{href}</D:href><CS:read-write/>")) {
            "read-write"
        } else {
            "read"
        };
        state.sharees.retain(|(who, _)| *who != href);
        state.sharees.push((href, access));
    }
    for remove in body
        .split("<CS:remove>")
        .skip(1)
        .filter(|_| !state.ignore_removes)
    {
        let href = remove
            .split("<D:href>")
            .nth(1)
            .and_then(|s| s.split("</D:href>").next())
            .unwrap();
        state.sharees.retain(|(who, _)| who != href);
    }
}

/// The `O:set` and `O:remove` of an `O:share` body, as Nextcloud does it:
/// only `principal:` hrefs of users it knows (bob and carol); anything
/// else is ignored without an error.
fn apply_oc_share(state: &mut State, collection: &str, body: &str) {
    let known = [
        "principal:principals/bob",
        "principal:principals/carol",
        "principal:principals/groups/staff",
    ];
    let href_of = |part: &str| {
        part.split("<D:href>")
            .nth(1)
            .and_then(|s| s.split("</D:href>").next())
            .unwrap()
            .to_owned()
    };
    for set in body.split("<O:set>").skip(1) {
        let set = set.split("</O:set>").next().unwrap();
        let href = href_of(set);
        if !known.contains(&href.as_str()) && !href.starts_with("principal:principals/u") {
            continue;
        }
        let write = set.contains("<O:read-write/>");
        state
            .oc_sharees
            .retain(|(c, who, _)| !(c == collection && *who == href));
        state.oc_sharees.push((collection.to_owned(), href, write));
    }
    for remove in body
        .split("<O:remove>")
        .skip(1)
        .filter(|_| !state.ignore_removes)
    {
        let href = href_of(remove);
        state
            .oc_sharees
            .retain(|(c, who, _)| !(c == collection && *who == href));
    }
}

fn oc_invite(state: &State, collection: &str) -> String {
    let mut out = String::from("<O:invite>");
    for (_, who, write) in state.oc_sharees.iter().filter(|(c, _, _)| c == collection) {
        let access = if *write { "read-write" } else { "read" };
        out.push_str(&format!(
            "<O:user><D:href>{who}</D:href><O:common-name>x</O:common-name><O:invite-accepted/><O:access><O:{access}/></O:access></O:user>"
        ));
    }
    out.push_str("</O:invite>");
    out
}

fn home(state: &State) -> String {
    let kind = state.kind;
    let mut out = response(
        "/dav/cal/alice/",
        "<D:resourcetype><D:collection/></D:resourcetype>",
    );
    match kind {
        Kind::Stalwart => {
            out.push_str(&response(
                "/dav/cal/alice/work/",
                &format!(
                    "<D:resourcetype><D:collection/><C:calendar/></D:resourcetype>\
                     <D:displayname>Work</D:displayname><D:sync-token>t1</D:sync-token>\
                     <D:owner><D:href>/dav/principals/alice/</D:href></D:owner>{ALL}\
                     <D:acl>{}</D:acl>",
                    state.acl
                ),
            ));
            for i in 0..state.shared {
                out.push_str(&response(
                    &format!("/dav/cal/bob/s{i:03}/"),
                    &format!(
                        "<D:resourcetype><D:collection/><C:calendar/></D:resourcetype>\
                         <D:displayname>Shared {i}</D:displayname><D:sync-token>t1</D:sync-token>\
                         <D:owner><D:href>/dav/principals/bob/</D:href></D:owner>{READ}<D:acl>{}</D:acl>",
                        state.shared_acl
                    ),
                ));
            }
            // Bob's calendar, shared with alice, listed in her home
            // ("assisted discovery"), and one outside the home that is
            // nobody else's (left out).
            out.push_str(&response(
                "/dav/cal/bob/team/",
                &format!(
                    "<D:resourcetype><D:collection/><C:calendar/></D:resourcetype>\
                     <D:displayname>Team</D:displayname><D:sync-token>t1</D:sync-token>\
                     <D:owner><D:href>/dav/principals/bob/</D:href></D:owner>{READ}"
                ),
            ));
            out.push_str(&response(
                "/dav/cal/other/",
                &format!(
                    "<D:resourcetype><D:collection/><C:calendar/></D:resourcetype>\
                     <D:sync-token>t1</D:sync-token>\
                     <D:owner><D:href>/dav/principals/alice/</D:href></D:owner>{ALL}"
                ),
            ));
        }
        Kind::Sabre => {
            let mut invite = String::from(
                "<CS:user><CS:invite-accepted/><CS:access/><D:href>/dav/principals/alice</D:href></CS:user>",
            );
            for (who, access) in &state.sharees {
                invite.push_str(&format!(
                    "<CS:user><CS:invite-accepted/><CS:access><CS:{access}/></CS:access><D:href>{who}</D:href></CS:user>"
                ));
            }
            invite.push_str(&state.extra_users);
            out.push_str(&response(
                "/dav/cal/alice/work/",
                &format!(
                    "<D:resourcetype><D:collection/><C:calendar/></D:resourcetype>\
                     <D:displayname>Work</D:displayname><D:sync-token>t1</D:sync-token>\
                     <D:owner><D:href>/dav/principals/alice/</D:href></D:owner>{ALL_AND_SHARE}\
                     <CS:invite>{invite}</CS:invite>\
                     <CS:allowed-sharing-modes><CS:can-be-shared/></CS:allowed-sharing-modes>"
                ),
            ));
            for i in 0..state.shared {
                out.push_str(&response(
                    &format!("/dav/cal/alice/d{i:03}/"),
                    &format!(
                        "<D:resourcetype><D:collection/><C:calendar/><CS:shared/></D:resourcetype>\
                         <D:displayname>Dave {i}</D:displayname><D:sync-token>t1</D:sync-token>\
                         <D:owner><D:href>/dav/principals/alice/</D:href></D:owner>{READ}\
                         <CS:invite><CS:organizer><D:href>urn:x-uid:dave</D:href></CS:organizer>\
                         <CS:user><CS:invite-accepted/><CS:access><CS:read/></CS:access><D:href>/dav/principals/alice</D:href></CS:user></CS:invite>"
                    ),
                ));
            }
            // Carol's calendar, shared with alice: a copy in alice's home,
            // owned by alice as sabre/dav says; carol is in the invite.
            out.push_str(&response(
                "/dav/cal/alice/6febbec2/",
                &format!(
                    "<D:resourcetype><D:collection/><C:calendar/><CS:shared/></D:resourcetype>\
                     <D:displayname>Carol's</D:displayname><D:sync-token>t1</D:sync-token>\
                     <D:owner><D:href>/dav/principals/alice/</D:href></D:owner>{READ}\
                     <CS:invite><CS:user><CS:invite-accepted/><CS:access/><D:href>/dav/principals/carol</D:href></CS:user>\
                     {copy_users}<CS:user><CS:invite-accepted/><CS:access><CS:read/></CS:access><D:href>mailto:alice@example.com</D:href></CS:user></CS:invite>\
                     <CS:allowed-sharing-modes><CS:can-be-shared/></CS:allowed-sharing-modes>",
                    copy_users = state.copy_users
                ),
            ));
        }
        Kind::Nextcloud => {
            out.push_str(&response(
                "/dav/cal/alice/work/",
                &format!(
                    "<D:resourcetype><D:collection/><C:calendar/></D:resourcetype>\
                     <D:displayname>Work</D:displayname><D:sync-token>t1</D:sync-token>\
                     <D:owner><D:href>/dav/principals/alice/</D:href></D:owner>{ALL}\
                     <CS:allowed-sharing-modes><CS:can-be-shared/><CS:can-be-published/></CS:allowed-sharing-modes>{}",
                    oc_invite(state, "/dav/cal/alice/work/")
                ),
            ));
            // Carol's calendar shared with alice: in alice's home, owned
            // by carol, renamed, an empty invite.
            out.push_str(&response(
                "/dav/cal/alice/team_shared_by_carol/",
                &format!(
                    "<D:resourcetype><D:collection/><C:calendar/></D:resourcetype>\
                     <D:displayname>Team (carol)</D:displayname><D:sync-token>t1</D:sync-token>\
                     <D:owner><D:href>/dav/principals/carol/</D:href></D:owner>{READ}\
                     <CS:allowed-sharing-modes/><O:invite/>"
                ),
            ));
        }
        Kind::Radicale => {
            out.push_str(&response(
                "/dav/cal/alice/work/",
                &format!(
                    "<D:resourcetype><D:collection/><C:calendar/></D:resourcetype>\
                     <D:displayname>Work</D:displayname><D:sync-token>t1</D:sync-token>\
                     <D:owner><D:href>/dav/principals/alice/</D:href></D:owner>{ALL}"
                ),
            ));
        }
    }
    out
}

fn books(state: &State) -> String {
    let mut out = response(
        "/dav/card/alice/",
        "<D:resourcetype><D:collection/></D:resourcetype>",
    );
    let acl = match state.kind {
        Kind::Radicale => String::new(),
        Kind::Nextcloud => oc_invite(state, "/dav/card/alice/people/"),
        Kind::Stalwart => format!("<D:acl>{}</D:acl>", state.book_acl),
        Kind::Sabre => "<D:acl/>".to_owned(),
    };
    out.push_str(&response(
        "/dav/card/alice/people/",
        &format!(
            "<D:resourcetype><D:collection/><CR:addressbook/></D:resourcetype>\
             <D:displayname>People</D:displayname><D:sync-token>t1</D:sync-token>\
             <D:owner><D:href>/dav/principals/alice/</D:href></D:owner>{ALL}{acl}"
        ),
    ));
    out
}

/// A listing without sharing (`D:acl`, `CS:invite`, `O:invite`), as a
/// server answers a PROPFIND that does not ask for them.
fn without_sharing(listing: &str) -> String {
    let mut out = listing.to_owned();
    for tag in ["D:acl", "CS:invite", "O:invite"] {
        loop {
            let open = format!("<{tag}>");
            let close = format!("</{tag}>");
            let Some(start) = out.find(&open) else {
                break;
            };
            let Some(end) = out[start..].find(&close) else {
                break;
            };
            out.replace_range(start..start + end + close.len(), "");
        }
        out = out.replace(&format!("<{tag}/>"), "");
    }
    out
}

/// The `D:response` about `href` in a listing (a Depth 0 answer).
fn response_in(listing: &str, href: &str) -> Option<String> {
    let start = listing.find(&format!("<D:response><D:href>{href}</D:href>"))?;
    let end = listing[start..].find("</D:response>")? + "</D:response>".len();
    Some(listing[start..start + end].to_owned())
}

fn directory(state: &State) -> String {
    (0..state.people)
        .map(|n| {
            principal(
                &format!("/dav/principals/u{n}/"),
                &format!("U{n}"),
                &format!("u{n}@example.com"),
            )
        })
        .collect()
}

fn handle(state: &mut State, method: &str, path: &str, body: &str) -> Response<Body> {
    state.requests.push(format!("{method} {path} {body}"));
    let kind = state.kind;
    let ok = |body: &str| reply(kind, 207, &multistatus(body));
    match (method, path) {
        ("OPTIONS", _) => reply(kind, 200, ""),
        ("PROPFIND", "/dav/") => ok(&response(
            "/dav/",
            "<D:current-user-principal><D:href>/dav/principals/alice/</D:href></D:current-user-principal>",
        )),
        ("PROPFIND", "/dav/principals/alice/") => ok(&response(
            "/dav/principals/alice/",
            "<C:calendar-home-set><D:href>/dav/cal/alice/</D:href></C:calendar-home-set>\
             <CR:addressbook-home-set><D:href>/dav/card/alice/</D:href></CR:addressbook-home-set>\
             <C:calendar-user-address-set><D:href>mailto:alice@example.com</D:href></C:calendar-user-address-set>\
             <D:principal-collection-set><D:href>/dav/principals/</D:href></D:principal-collection-set>\
             <D:displayname>Alice</D:displayname>",
        )),
        // Nextcloud hides its users: no listing, only searches (by name,
        // which also matches addresses, or by address).
        ("PROPFIND", "/dav/principals/") if kind == Kind::Nextcloud => reply(kind, 405, ""),
        ("REPORT", "/dav/principals/")
            if kind == Kind::Nextcloud && body.contains("principal-search-property-set") =>
        {
            reply(
                kind,
                200,
                r#"<?xml version="1.0"?><d:principal-search-property-set xmlns:d="DAV:" xmlns:s="http://sabredav.org/ns" xmlns:cal="urn:ietf:params:xml:ns:caldav"><d:principal-search-property><d:prop><d:displayname/></d:prop></d:principal-search-property><d:principal-search-property><d:prop><s:email-address/></d:prop></d:principal-search-property><d:principal-search-property><d:prop><cal:calendar-user-address-set/></d:prop></d:principal-search-property></d:principal-search-property-set>"#,
            )
        }
        ("REPORT", "/dav/principals/") if kind == Kind::Nextcloud => {
            let people = [
                ("alice", "Alice", "alice@example.com"),
                ("bob", "Bob", "bob@example.com"),
                ("carol", "Carol", "carol@example.com"),
                // Found, but Nextcloud's sharing ignores him.
                ("dave", "Dave", "dave@example.com"),
            ];
            let term = body
                .split("<D:match>")
                .nth(1)
                .and_then(|s| s.split("</D:match>").next())
                .unwrap_or_default()
                .to_owned();
            let mut found: String = people
                .iter()
                .filter(|(_, name, address)| name.contains(&term) || address.contains(&term))
                .map(|(user, name, address)| {
                    principal(&format!("/dav/principals/{user}/"), name, address)
                })
                .collect();
            found.push_str(&directory(state));
            // A group: no address, so only a search by its name finds it.
            if "Staff".contains(&term) {
                found.push_str(&response(
                    "/dav/principals/groups/staff/",
                    "<D:resourcetype><D:principal/><D:collection/></D:resourcetype>\
                     <D:displayname>Staff</D:displayname>\
                     <C:calendar-user-type>GROUP</C:calendar-user-type>",
                ));
            }
            ok(&found)
        }
        ("PROPFIND", "/dav/principals/") => ok(&format!(
            "{}{}{}{}{}",
            response(
                "/dav/principals/",
                "<D:resourcetype><D:collection/></D:resourcetype>"
            ),
            principal("/dav/principals/alice/", "Alice", "alice@example.com"),
            // Bob has two addresses.
            response(
                "/dav/principals/bob/",
                "<D:resourcetype><D:principal/><D:collection/></D:resourcetype>\
                 <D:displayname>Bob</D:displayname>\
                 <C:calendar-user-address-set><D:href>mailto:bob@example.com</D:href>\
                 <D:href>mailto:bob.old@example.com</D:href></C:calendar-user-address-set>"
            ),
            principal("/dav/principals/carol/", "Carol", "carol@example.com"),
            directory(state),
        )),
        ("REPORT", "/dav/principals/") => reply(kind, 207, &multistatus("")),
        // Sharing only when asked for (a split listing asks apart).
        ("PROPFIND", "/dav/cal/alice/") if body.contains("acl/>") => ok(&home(state)),
        ("PROPFIND", "/dav/cal/alice/") => ok(&without_sharing(&home(state))),
        ("PROPFIND", "/dav/card/alice/") if body.contains("acl/>") => ok(&books(state)),
        ("PROPFIND", "/dav/card/alice/") => ok(&without_sharing(&books(state))),
        ("PROPFIND", "/dav/cal/alice/work/") if kind == Kind::Stalwart => ok(&response(
            "/dav/cal/alice/work/",
            &format!("<D:acl>{}</D:acl>", state.acl),
        )),
        ("ACL", "/dav/cal/alice/work/") if kind == Kind::Stalwart => {
            state.acl = inner(body, "D:acl");
            reply(kind, 200, "")
        }
        ("PROPFIND", "/dav/card/alice/people/") if kind == Kind::Stalwart => ok(&response(
            "/dav/card/alice/people/",
            &format!("<D:acl>{}</D:acl>", state.book_acl),
        )),
        ("ACL", "/dav/card/alice/people/") if kind == Kind::Stalwart => {
            state.book_acl = inner(body, "D:acl");
            reply(kind, 200, "")
        }
        // A collection's own sharing (split listing): as in the home's.
        ("PROPFIND", _) if path.starts_with("/dav/cal/alice/") && body.contains("acl/>") => {
            match response_in(&home(state), path) {
                Some(found) => ok(&found),
                None => reply(kind, 404, ""),
            }
        }
        ("POST", "/dav/cal/alice/work/") if kind == Kind::Sabre => {
            let number = state.posts;
            state.posts += 1;
            if state.fail_post_at == Some(number) {
                return reply(kind, 500, "");
            }
            apply_share(state, body);
            if number == 0
                && let Some(who) = state.upgrade_after_post.take()
            {
                for (sharee, access) in &mut state.sharees {
                    if *sharee == who {
                        *access = "read-write";
                    }
                }
            }
            reply(kind, 200, "")
        }
        // Nextcloud: its own sharing; a `CS:share` is not implemented.
        ("POST", "/dav/cal/alice/work/" | "/dav/card/alice/people/") if kind == Kind::Nextcloud => {
            if body.contains("<CS:share") {
                return reply(kind, 501, "");
            }
            // As Nextcloud 34 (`validateShareRequest`): at most 10 `set`
            // and `remove` elements together, else 400.
            if body.matches("<O:set>").count() + body.matches("<O:remove>").count() > 10 {
                return reply(kind, 400, "");
            }
            let number = state.posts;
            state.posts += 1;
            if state.fail_post_at == Some(number) {
                return reply(kind, 500, "");
            }
            apply_oc_share(state, path, body);
            reply(kind, 200, "")
        }
        ("REPORT", _) => ok("<D:sync-token>t1</D:sync-token>"),
        _ => reply(kind, 404, ""),
    }
}

struct Fake {
    port: u16,
    state: Arc<Mutex<State>>,
}

impl Fake {
    async fn start(kind: Kind) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(Mutex::new(State {
            kind,
            // Something the bridge did not write and must keep.
            acl: "<D:ace><D:principal><D:authenticated/></D:principal><D:grant><D:privilege><C:read-free-busy/></D:privilege></D:grant></D:ace>".to_owned(),
            sharees: Vec::new(),
            oc_sharees: Vec::new(),
            ignore_removes: false,
            extra_users: String::new(),
            shared: 0,
            acl_delay: None,
            post_delay: None,
            people: 0,
            fail_post_at: None,
            upgrade_after_post: None,
            posts: 0,
            book_acl: String::new(),
            copy_users: String::new(),
            shared_acl: String::new(),
            requests: Vec::new(),
        }));
        let shared = Arc::clone(&state);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let state = Arc::clone(&shared);
                tokio::spawn(async move {
                    let service =
                        service_fn(move |request: http::Request<hyper::body::Incoming>| {
                            let state = Arc::clone(&state);
                            async move {
                                let (parts, body) = request.into_parts();
                                let body = body.collect().await.unwrap().to_bytes();
                                let body = String::from_utf8_lossy(&body).into_owned();
                                let (response, delay) = {
                                    let mut state = state.lock().unwrap();
                                    // The bridge here only uses 127.0.0.1. A
                                    // `localhost` request comes from outside
                                    // the test (e.g. an IDE that probes every
                                    // new listening port): not recorded.
                                    if parts
                                        .headers
                                        .get("host")
                                        .and_then(|v| v.to_str().ok())
                                        .is_some_and(|h| h.split(':').next() == Some("localhost"))
                                    {
                                        return Ok::<_, Infallible>(reply(state.kind, 404, ""));
                                    }
                                    let delay = state
                                        .acl_delay
                                        .filter(|_| {
                                            parts.method == "PROPFIND"
                                                && parts.uri.path() == "/dav/cal/alice/work/"
                                        })
                                        .or(state.post_delay.filter(|_| parts.method == "POST"));
                                    let response = handle(
                                        &mut state,
                                        parts.method.as_str(),
                                        parts.uri.path(),
                                        &body,
                                    );
                                    (response, delay)
                                };
                                if let Some(delay) = delay {
                                    tokio::time::sleep(delay).await;
                                }
                                Ok::<_, Infallible>(response)
                            }
                        });
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        Self { port, state }
    }

    fn last(&self, method: &str) -> Option<String> {
        self.state.lock().unwrap().last(method).cloned()
    }
}

struct NoDns;

impl Resolver for NoDns {
    fn lookup<'a>(&'a self, _name: &'a DomainName) -> LookupFuture<'a> {
        Box::pin(async { Err(DnsError) })
    }
}

struct Setup {
    fake: Fake,
    store: Store,
    id: AccountId,
    account: Arc<GroupwareAccount>,
    _groupware: Groupware,
    _dir: tempfile::TempDir,
}

impl Setup {
    async fn collections(&self) -> Vec<Collection> {
        self.store
            .dav_collections(self.id)
            .await
            .unwrap()
            .into_iter()
            .filter(|c| c.orphaned_at.is_none())
            .collect()
    }

    async fn named(&self, name: &str) -> Collection {
        self.collections()
            .await
            .into_iter()
            .find(|c| c.props.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no collection {name}"))
    }

    async fn principal(&self, name: &str) -> nebula_groupware::principals::Principal {
        self.account
            .principals()
            .await
            .unwrap()
            .into_iter()
            .find(|p| p.name.as_deref() == Some(name))
            .unwrap()
    }
}

async fn setup(kind: Kind) -> Setup {
    setup_with(kind, |_| {}, |_| {}).await
}

/// A setup whose fake starts with `prepare` and whose bridge has
/// `configure` (limits, budgets).
async fn setup_with(
    kind: Kind,
    prepare: impl FnOnce(&mut State),
    configure: impl FnOnce(&mut GroupwareConfig),
) -> Setup {
    let fake = Fake::start(kind).await;
    prepare(&mut fake.state.lock().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let id: AccountId = store
        .get_or_create_account(
            NewAccount {
                login: "alice",
                email: "alice@example.com",
                wrapped_dek: b"k",
            },
            UnixTime::now().unwrap(),
        )
        .await
        .unwrap()
        .id;
    let server = DavServer::new(&format!("http://127.0.0.1:{}/dav/", fake.port))
        .unwrap()
        .with_private_network(true);
    let mut config = GroupwareConfig::new(server);
    config.poll_with_push = Duration::from_hours(1);
    config.poll_without_push = Duration::from_hours(1);
    configure(&mut config);
    let groupware = Groupware::new(store.clone(), config, Arc::new(NoDns));
    let key = AccountKey::new(
        KeyId::new(b"login:alice").unwrap(),
        DataKey::generate().unwrap(),
    );
    let account = groupware
        .open(id, "alice", SecretString::from("password"), key)
        .unwrap();
    for _ in 0..500 {
        if account.status() == DavStatus::Ready {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(account.status(), DavStatus::Ready);
    // The first round has run once Ready; wait for the store to have it.
    for _ in 0..500 {
        if !store.dav_collections(id).await.unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Setup {
        fake,
        store,
        id,
        account,
        _groupware: groupware,
        _dir: dir,
    }
}

impl Setup {
    /// Changes the fake server as another client would, then runs a sync
    /// round so the store lists it.
    async fn change_on_server(&self, change: impl FnOnce(&mut State)) {
        change(&mut self.fake.state.lock().unwrap());
        self.account.sync_round(false).await.unwrap();
    }

    fn posts(&self) -> usize {
        self.fake
            .state
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|r| r.starts_with("POST"))
            .count()
    }
}

fn url(fake: &Fake, path: &str) -> String {
    format!("http://127.0.0.1:{}{path}", fake.port)
}

const READ_RIGHTS: Rights = Rights::READ.union(Rights::READ_FREE_BUSY);

/// Stalwart: alice shares her calendar with bob by ACL, changes it to
/// read and write, and stops; an ACE the bridge did not write stays. Bob's
/// calendar listed in alice's home is his (not shareable by her), and one
/// outside her home that is nobody else's is left out.
#[tokio::test]
async fn stalwart_like_acl() {
    let s = setup(Kind::Stalwart).await;
    let work = s.named("Work").await;
    assert_eq!(work.props.sharing.owner, None);
    assert_eq!(work.props.sharing.mode, Some(ShareMode::Acl));
    assert!(work.props.sharing.shares.is_empty());
    let team = s.named("Team").await;
    assert_eq!(
        team.props.sharing.owner.as_ref().unwrap().as_str(),
        url(&s.fake, "/dav/principals/bob/")
    );
    assert_eq!(team.props.sharing.mode, None);
    assert!(!team.is_default, "someone else's is never the default");
    assert!(
        s.collections()
            .await
            .iter()
            .all(|c| !c.props.href.as_str().ends_with("/dav/cal/other/"))
    );
    // Address books are shared the same way.
    assert_eq!(
        s.named("People").await.props.sharing.mode,
        Some(ShareMode::Acl)
    );

    let bob = s.principal("Bob").await;
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                set: vec![(bob.clone(), READ_RIGHTS)],
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    let sent = s.fake.last("ACL").unwrap();
    assert!(sent.contains("<D:authenticated/>"), "kept: {sent}");
    assert!(sent.contains(
        "<D:href>/dav/principals/bob/</D:href></D:principal><D:grant><D:privilege><D:read/></D:privilege><D:privilege><C:read-free-busy/></D:privilege></D:grant>"
    ));
    let work = s.named("Work").await;
    assert_eq!(
        work.props.sharing.shares,
        [(
            Sharee::new(&url(&s.fake, "/dav/principals/bob/")).unwrap(),
            READ_RIGHTS
        )]
    );

    // Read and write: bob's ACE replaced, not added.
    let write = READ_RIGHTS
        .union(Rights::WRITE_CONTENT)
        .union(Rights::BIND)
        .union(Rights::UNBIND)
        .union(Rights::WRITE_PROPERTIES);
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                set: vec![(bob.clone(), write)],
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    let sent = s.fake.last("ACL").unwrap();
    assert_eq!(sent.matches("/dav/principals/bob/").count(), 1, "{sent}");
    assert!(sent.contains("<D:privilege><D:write/></D:privilege>"));
    let work = s.named("Work").await;
    assert!(work.props.sharing.shares[0].1.contains(write));

    // shareWith set whole to nobody: bob's grant goes, the rest stays.
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                replace: true,
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    let sent = s.fake.last("ACL").unwrap();
    assert!(!sent.contains("bob"), "{sent}");
    assert!(sent.contains("<D:authenticated/>"));
    assert!(s.named("Work").await.props.sharing.shares.is_empty());

    // Someone else's calendar can't be shared by alice.
    let refused = s
        .account
        .share_collection(
            team.id,
            &ShareUpdate {
                set: vec![(bob, READ_RIGHTS)],
                ..ShareUpdate::default()
            },
        )
        .await;
    assert!(
        matches!(
            refused,
            Err(GroupwareError::Share(ShareError::NotShareable))
        ),
        "{refused:?}"
    );
}

/// sabre/dav (Baïkal): calendars are shared with a `CS:share` POST naming
/// the sharee by address; the sharee's copy is someone else's (the owner
/// from `CS:invite`), and address books are not shareable.
#[tokio::test]
async fn sabre_like_calendar_server() {
    let s = setup(Kind::Sabre).await;
    let work = s.named("Work").await;
    assert_eq!(work.props.sharing.mode, Some(ShareMode::CalendarServer));
    let copy = s.named("Carol's").await;
    assert_eq!(
        copy.props.sharing.owner.as_ref().unwrap().as_str(),
        url(&s.fake, "/dav/principals/carol/")
    );
    assert_eq!(copy.props.sharing.mode, None);
    assert_eq!(s.named("People").await.props.sharing.mode, None);

    let bob = s.principal("Bob").await;
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                set: vec![(bob.clone(), READ_RIGHTS)],
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    let sent = s.fake.last("POST").unwrap();
    assert!(
        sent.contains("<CS:set><D:href>mailto:bob@example.com</D:href><CS:read/></CS:set>"),
        "{sent}"
    );
    let work = s.named("Work").await;
    assert_eq!(
        work.props.sharing.shares,
        [(Sharee::new("mailto:bob@example.com").unwrap(), READ_RIGHTS)]
    );
    // Unshare: removed by the name the server lists.
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                remove: vec![bob],
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    let sent = s.fake.last("POST").unwrap();
    assert!(
        sent.contains("<CS:remove><D:href>mailto:bob@example.com</D:href></CS:remove>"),
        "{sent}"
    );
    assert!(s.named("Work").await.props.sharing.shares.is_empty());
}

/// Radicale: no sharing at all.
#[tokio::test]
async fn radicale_like_nothing() {
    let s = setup(Kind::Radicale).await;
    for collection in s.collections().await {
        assert_eq!(collection.props.sharing.mode, None);
        assert_eq!(collection.props.sharing.owner, None);
    }
    let work = s.named("Work").await;
    assert_eq!(work.props.kind, CollectionKind::Calendar);
    let bob = s.principal("Bob").await;
    let refused = s
        .account
        .share_collection(
            work.id,
            &ShareUpdate {
                set: vec![(bob, READ_RIGHTS)],
                ..ShareUpdate::default()
            },
        )
        .await;
    assert!(
        matches!(
            refused,
            Err(GroupwareError::Share(ShareError::NotShareable))
        ),
        "{refused:?}"
    );
    assert!(s.fake.last("ACL").is_none() && s.fake.last("POST").is_none());
}

/// Nextcloud: calendars and address books are shared with an `O:share`
/// POST naming the principal by its path below the DAV root (not the
/// Calendar Server sharing it also announces); people are found by
/// searching (it lists nobody); a calendar shared with alice is carol's;
/// a sharee Nextcloud silently ignores is an error.
#[tokio::test]
async fn nextcloud_like_oc_share() {
    let s = setup(Kind::Nextcloud).await;
    let work = s.named("Work").await;
    assert_eq!(work.props.sharing.mode, Some(ShareMode::Nextcloud));
    assert_eq!(
        s.named("People").await.props.sharing.mode,
        Some(ShareMode::Nextcloud)
    );
    let team = s.named("Team (carol)").await;
    assert_eq!(
        team.props.sharing.owner.as_ref().unwrap().as_str(),
        url(&s.fake, "/dav/principals/carol/")
    );
    assert_eq!(team.props.sharing.mode, None);

    // The directory comes from a search (`@`), not a listing.
    let bob = s.principal("Bob").await;
    let dave = s.principal("Dave").await;
    assert!(
        s.fake
            .last("REPORT /dav/principals/")
            .is_some_and(|r| r.contains("<D:match>@</D:match>")),
    );

    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                set: vec![(bob.clone(), READ_RIGHTS)],
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    let sent = s.fake.last("POST").unwrap();
    assert!(
        sent.contains("<O:set><D:href>principal:principals/bob</D:href><O:read/></O:set>"),
        "{sent}"
    );
    assert!(!sent.contains("CS:share"), "{sent}");
    let work = s.named("Work").await;
    assert_eq!(
        work.props.sharing.shares,
        [(
            Sharee::new(&url(&s.fake, "/dav/principals/bob/")).unwrap(),
            READ_RIGHTS
        )]
    );

    // Read and write, on the address book too.
    let write = READ_RIGHTS.union(Rights::WRITE_CONTENT).union(Rights::BIND);
    let people = s.named("People").await;
    s.account
        .share_collection(
            people.id,
            &ShareUpdate {
                set: vec![(bob.clone(), write)],
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    let sent = s.fake.last("POST /dav/card/alice/people/").unwrap();
    assert!(sent.contains("<O:read-write/>"), "{sent}");
    let people = s.named("People").await;
    assert!(
        people.props.sharing.shares[0]
            .1
            .contains(Rights::WRITE_CONTENT)
    );

    // Someone Nextcloud does not share with: 200, but not shared.
    let ignored = s
        .account
        .share_collection(
            work.id,
            &ShareUpdate {
                set: vec![(dave, READ_RIGHTS)],
                ..ShareUpdate::default()
            },
        )
        .await;
    assert!(
        matches!(ignored, Err(GroupwareError::Share(ShareError::Ignored))),
        "{ignored:?}"
    );

    // shareWith set whole to nobody: bob removed by his principal path.
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                replace: true,
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    let sent = s.fake.last("POST /dav/cal/alice/work/").unwrap();
    assert!(
        sent.contains("<O:remove><D:href>principal:principals/bob</D:href></O:remove>"),
        "{sent}"
    );
    assert!(s.named("Work").await.props.sharing.shares.is_empty());

    // Carol's calendar can't be shared again by alice.
    let refused = s
        .account
        .share_collection(
            team.id,
            &ShareUpdate {
                set: vec![(bob, READ_RIGHTS)],
                ..ShareUpdate::default()
            },
        )
        .await;
    assert!(
        matches!(
            refused,
            Err(GroupwareError::Share(ShareError::NotShareable))
        ),
        "{refused:?}"
    );
}

/// sabre/dav (Baïkal) hides every other user: an address the server
/// shows no principal for gets a number of its own and is shared with by
/// address in a `CS:share`.
#[tokio::test]
async fn sabre_like_share_by_address() {
    let s = setup(Kind::Sabre).await;
    assert!(s.account.shares_by_address());
    let work = s.named("Work").await;
    let dan = nebula_dav::proto::CalendarAddress::parse("mailto:dan@outside.example").unwrap();
    let number = s.account.address_principal(&dan).unwrap();
    assert_eq!(number, nebula_groupware::principals::address_number(&dan));
    assert_eq!(s.account.known_address(number), Some(dan.clone()));
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                set_addresses: vec![(dan.clone(), READ_RIGHTS)],
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    let sent = s.fake.last("POST").unwrap();
    assert!(
        sent.contains("<CS:set><D:href>mailto:dan@outside.example</D:href><CS:read/></CS:set>"),
        "{sent}"
    );
    let work = s.named("Work").await;
    let (sharee, _) = &work.props.sharing.shares[0];
    assert_eq!(s.account.sharee_number(sharee), Some(number));
    // Replaced by nobody: removed by address.
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                replace: true,
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    let sent = s.fake.last("POST").unwrap();
    assert!(
        sent.contains("<CS:remove><D:href>mailto:dan@outside.example</D:href></CS:remove>"),
        "{sent}"
    );
    assert!(s.named("Work").await.props.sharing.shares.is_empty());
}

/// Addresses are only for Calendar Server sharing: Stalwart's ACL names
/// principals, so nothing is sent; and its addresses get no number.
#[tokio::test]
async fn stalwart_like_no_addresses() {
    let s = setup(Kind::Stalwart).await;
    assert!(!s.account.shares_by_address());
    let dan = nebula_dav::proto::CalendarAddress::parse("mailto:dan@outside.example").unwrap();
    assert_eq!(s.account.address_principal(&dan), None);
    let work = s.named("Work").await;
    let refused = s
        .account
        .share_collection(
            work.id,
            &ShareUpdate {
                set_addresses: vec![(dan, READ_RIGHTS)],
                ..ShareUpdate::default()
            },
        )
        .await;
    assert!(
        matches!(refused, Err(GroupwareError::Share(ShareError::Unnamed))),
        "{refused:?}"
    );
    assert!(s.fake.last("ACL").is_none());
    // It lists everyone: nothing to search by name.
    assert!(s.account.search_by_name("Bo").await.unwrap().is_empty());
}

/// Nextcloud's groups have no address: the directory (a search for `@`)
/// misses them, a search by name finds them, and they are shared with
/// by their principal path.
#[tokio::test]
async fn nextcloud_like_groups() {
    let s = setup(Kind::Nextcloud).await;
    assert!(!s.account.shares_by_address());
    let all = s.account.principals().await.unwrap();
    assert!(all.iter().all(|p| p.name.as_deref() != Some("Staff")));
    // Too short or with controls: nothing asked.
    assert!(s.account.search_by_name("S").await.unwrap().is_empty());
    assert!(
        s.account
            .search_by_name("St\naff")
            .await
            .unwrap()
            .is_empty()
    );
    let found = s.account.search_by_name("Staf").await.unwrap();
    let staff = found
        .into_iter()
        .find(|p| p.name.as_deref() == Some("Staff"))
        .unwrap();
    assert_eq!(
        staff.kind,
        nebula_groupware::principals::PrincipalKind::Group
    );
    assert!(
        s.fake
            .last("REPORT /dav/principals/")
            .is_some_and(|r| r.contains("<D:match>Staf</D:match>"))
    );
    // Remembered: Principal/get finds it by its number.
    assert!(s.account.known_principal(staff.number()).is_some());
    assert!(s.account.can_name(&staff, ShareMode::Nextcloud));
    let work = s.named("Work").await;
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                set: vec![(staff, READ_RIGHTS)],
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    let sent = s.fake.last("POST").unwrap();
    assert!(
        sent.contains("<O:set><D:href>principal:principals/groups/staff</D:href><O:read/></O:set>"),
        "{sent}"
    );
    assert_eq!(
        s.named("Work").await.props.sharing.shares[0].0.as_str(),
        url(&s.fake, "/dav/principals/groups/staff/")
    );
}

const FREE_BUSY: Rights = Rights::READ_FREE_BUSY;

/// "Free/busy only" exists only with WebDAV ACL: Nextcloud and Calendar
/// Server sharing would make it full read access, so nothing is sent.
#[tokio::test]
async fn free_busy_only_needs_acl() {
    for kind in [Kind::Nextcloud, Kind::Sabre] {
        let s = setup(kind).await;
        let work = s.named("Work").await;
        let bob = s.principal("Bob").await;
        let refused = s
            .account
            .share_collection(
                work.id,
                &ShareUpdate {
                    set: vec![(bob, FREE_BUSY)],
                    ..ShareUpdate::default()
                },
            )
            .await;
        assert!(
            matches!(
                refused,
                Err(GroupwareError::Share(ShareError::FreeBusyOnly))
            ),
            "{refused:?}"
        );
        assert_eq!(s.posts(), 0);
    }
    // By address (sabre/dav), the same.
    let s = setup(Kind::Sabre).await;
    let work = s.named("Work").await;
    let dan = nebula_dav::proto::CalendarAddress::parse("mailto:dan@example.com").unwrap();
    s.account.address_principal(&dan).unwrap();
    let refused = s
        .account
        .share_collection(
            work.id,
            &ShareUpdate {
                set_addresses: vec![(dan, FREE_BUSY)],
                ..ShareUpdate::default()
            },
        )
        .await;
    assert!(
        matches!(
            refused,
            Err(GroupwareError::Share(ShareError::FreeBusyOnly))
        ),
        "{refused:?}"
    );
    assert_eq!(s.posts(), 0);

    // With ACL it is `read-free-busy` alone.
    let s = setup(Kind::Stalwart).await;
    let work = s.named("Work").await;
    let bob = s.principal("Bob").await;
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                set: vec![(bob, FREE_BUSY)],
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    let sent = s.fake.last("ACL").unwrap();
    assert!(
        sent.contains(
            "<D:href>/dav/principals/bob/</D:href></D:principal><D:grant><D:privilege><C:read-free-busy/></D:privilege></D:grant>"
        ),
        "{sent}"
    );
}

/// A sharee the directory does not show (a Nextcloud group seen only in
/// the collection's invite) is changed and removed by the location it is
/// listed at; a server that answers yes and keeps it is an error.
#[tokio::test]
async fn nextcloud_like_listed_group() {
    let s = setup(Kind::Nextcloud).await;
    s.change_on_server(|state| {
        state.oc_sharees.push((
            "/dav/cal/alice/work/".to_owned(),
            "principal:principals/groups/staff".to_owned(),
            false,
        ));
    })
    .await;
    let work = s.named("Work").await;
    let staff = Sharee::new(&url(&s.fake, "/dav/principals/groups/staff/")).unwrap();
    assert_eq!(work.props.sharing.shares, [(staff.clone(), READ_RIGHTS)]);
    assert!(s.account.sharee_number(&staff).is_some());

    // New rights.
    let write = READ_RIGHTS.union(Rights::WRITE_CONTENT).union(Rights::BIND);
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                set_sharees: vec![(staff.clone(), write)],
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    let sent = s.fake.last("POST").unwrap();
    assert!(
        sent.contains(
            "<O:set><D:href>principal:principals/groups/staff</D:href><O:read-write/></O:set>"
        ),
        "{sent}"
    );

    // Removed.
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                remove_sharees: vec![staff.clone()],
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    let sent = s.fake.last("POST").unwrap();
    assert!(
        sent.contains("<O:remove><D:href>principal:principals/groups/staff</D:href></O:remove>"),
        "{sent}"
    );
    assert!(s.named("Work").await.props.sharing.shares.is_empty());

    // Answered yes, still there: not a success.
    s.change_on_server(|state| {
        state.ignore_removes = true;
        state.oc_sharees.push((
            "/dav/cal/alice/work/".to_owned(),
            "principal:principals/groups/staff".to_owned(),
            false,
        ));
    })
    .await;
    let kept = s
        .account
        .share_collection(
            work.id,
            &ShareUpdate {
                remove_sharees: vec![staff],
                ..ShareUpdate::default()
            },
        )
        .await;
    assert!(
        matches!(kept, Err(GroupwareError::Share(ShareError::NotRemoved))),
        "{kept:?}"
    );
}

/// A whole `shareWith` removes whoever the server has now, also someone
/// another client added after the last sync round.
#[tokio::test]
async fn replace_reads_the_sharees_again() {
    // Nextcloud.
    let s = setup(Kind::Nextcloud).await;
    let work = s.named("Work").await;
    let bob = s.principal("Bob").await;
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                set: vec![(bob.clone(), READ_RIGHTS)],
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    // Carol added by another client; the store does not know yet.
    s.fake.state.lock().unwrap().oc_sharees.push((
        "/dav/cal/alice/work/".to_owned(),
        "principal:principals/carol".to_owned(),
        false,
    ));
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                replace: true,
                set: vec![(bob.clone(), READ_RIGHTS)],
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    let sent = s.fake.last("POST").unwrap();
    assert!(
        sent.contains("<O:remove><D:href>principal:principals/carol</D:href></O:remove>"),
        "{sent}"
    );
    let shares = s.named("Work").await.props.sharing.shares;
    assert_eq!(
        shares,
        [(
            Sharee::new(&url(&s.fake, "/dav/principals/bob/")).unwrap(),
            READ_RIGHTS
        )]
    );

    // sabre/dav, with an address that has no principal.
    let s = setup(Kind::Sabre).await;
    let work = s.named("Work").await;
    let bob = s.principal("Bob").await;
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                set: vec![(bob.clone(), READ_RIGHTS)],
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    s.fake
        .state
        .lock()
        .unwrap()
        .sharees
        .push(("mailto:erin@example.com".to_owned(), "read"));
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                replace: true,
                set: vec![(bob.clone(), READ_RIGHTS)],
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    let sent = s.fake.last("POST").unwrap();
    assert!(
        sent.contains("<CS:remove><D:href>mailto:erin@example.com</D:href></CS:remove>"),
        "{sent}"
    );
    assert_eq!(
        s.named("Work").await.props.sharing.shares,
        [(Sharee::new("mailto:bob@example.com").unwrap(), READ_RIGHTS)]
    );

    // A server that keeps the one removed: an error, not a success.
    s.change_on_server(|state| {
        state.ignore_removes = true;
        state
            .sharees
            .push(("mailto:erin@example.com".to_owned(), "read"));
    })
    .await;
    let kept = s
        .account
        .share_collection(
            work.id,
            &ShareUpdate {
                replace: true,
                set: vec![(bob, READ_RIGHTS)],
                ..ShareUpdate::default()
            },
        )
        .await;
    assert!(
        matches!(kept, Err(GroupwareError::Share(ShareError::NotRemoved))),
        "{kept:?}"
    );
}

/// Two changes of who "work" is shared with, at once (two clicks, two
/// tabs): one at a time, so the second reads what the first wrote and
/// never gives back access the first just took away.
#[tokio::test]
async fn sharing_changes_go_one_at_a_time() {
    let s = setup(Kind::Stalwart).await;
    let work = s.named("Work").await;
    let bob = s.principal("Bob").await;
    let carol = s.principal("Carol").await;
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                set: vec![(carol.clone(), READ_RIGHTS)],
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    {
        let mut state = s.fake.state.lock().unwrap();
        state.acl_delay = Some(Duration::from_millis(200));
        state.requests.clear();
    }
    let remove_carol = ShareUpdate {
        remove: vec![carol],
        ..ShareUpdate::default()
    };
    let add_bob = ShareUpdate {
        set: vec![(bob, READ_RIGHTS)],
        ..ShareUpdate::default()
    };
    let (first, second) = tokio::join!(
        s.account.share_collection(work.id, &remove_carol),
        s.account.share_collection(work.id, &add_bob),
    );
    first.unwrap();
    second.unwrap();
    let state = s.fake.state.lock().unwrap();
    // The reads of the ACL (not the listings of `relist`) and the writes.
    let steps: Vec<&str> = state
        .requests
        .iter()
        .filter_map(|r| {
            if r.starts_with("PROPFIND /dav/cal/alice/work/ ") && r.contains("acl") {
                Some("read")
            } else if r.starts_with("ACL /dav/cal/alice/work/ ") {
                Some("write")
            } else {
                None
            }
        })
        .collect();
    assert_eq!(steps, ["read", "write", "read", "write"]);
    assert!(state.acl.contains("/dav/principals/bob/"), "{}", state.acl);
    assert!(!state.acl.contains("carol"), "{}", state.acl);
}

/// sabre/dav: an invitation with someone the bridge cannot name (Apple's
/// `urn:x-uid:`) who has access leaves "work" without a sharing mode: a
/// replacement would not remove them. A declined one does not matter.
#[tokio::test]
async fn sabre_like_incomplete_invitation() {
    let s = setup(Kind::Sabre).await;
    s.fake.state.lock().unwrap().extra_users = "<CS:user><D:href>urn:x-uid:dave</D:href><CS:invite-declined/><CS:access><CS:read-write/></CS:access></CS:user>".to_owned();
    s.account.sync_round(false).await.unwrap();
    assert_eq!(
        s.named("Work").await.props.sharing.mode,
        Some(ShareMode::CalendarServer)
    );
    s.fake.state.lock().unwrap().extra_users = "<CS:user><D:href>urn:x-uid:dave</D:href><CS:invite-noresponse/><CS:access><CS:read-write/></CS:access></CS:user>".to_owned();
    s.account.sync_round(false).await.unwrap();
    let work = s.named("Work").await;
    assert_eq!(work.props.sharing.mode, None);
    assert!(work.props.sharing.shares.is_empty());
    let refused = s
        .account
        .share_collection(
            work.id,
            &ShareUpdate {
                replace: true,
                ..ShareUpdate::default()
            },
        )
        .await;
    assert!(
        matches!(
            refused,
            Err(GroupwareError::Share(ShareError::NotShareable))
        ),
        "{refused:?}"
    );
    assert!(s.fake.last("POST").is_none());
}

/// Stalwart lists what others share in the home: however much bob shares,
/// the round works and keeps the account's own first, then what it
/// already had, then the rest by href, the same in every round.
#[tokio::test]
async fn sharing_many_collections_does_not_stop_the_sync() {
    let s = setup(Kind::Stalwart).await;
    let team = s.named("Team").await;
    s.fake.state.lock().unwrap().shared = 250;
    s.account.sync_round(false).await.unwrap();
    let max = nebula_dav::Limits::default().max_collections;
    let kept = s.collections().await;
    assert_eq!(kept.len(), max);
    let names: Vec<String> = kept.iter().filter_map(|c| c.props.name.clone()).collect();
    assert!(names.iter().any(|n| n == "Work"));
    assert!(names.iter().any(|n| n == "People"));
    // Bob's "team" was there before: not pushed out by the new ones,
    // whose hrefs come first.
    assert!(kept.iter().any(|c| c.id == team.id));
    assert!(names.iter().any(|n| n == "Shared 0"));
    assert!(!names.iter().any(|n| n == "Shared 249"));
    // The same ones in the next round.
    s.account.sync_round(false).await.unwrap();
    let mut again: Vec<_> = s.collections().await.iter().map(|c| c.id).collect();
    let mut first: Vec<_> = kept.iter().map(|c| c.id).collect();
    again.sort_unstable();
    first.sort_unstable();
    assert_eq!(again, first);
}

/// Calendar Server copies shared with alice whose organizer the bridge
/// cannot resolve (`urn:x-uid:`) are still someone else's: however many,
/// the round works and keeps alice's own.
#[tokio::test]
async fn unresolved_sharers_do_not_stop_the_sync() {
    let s = setup(Kind::Sabre).await;
    s.fake.state.lock().unwrap().shared = 250;
    s.account.sync_round(false).await.unwrap();
    let max = nebula_dav::Limits::default().max_collections;
    let kept = s.collections().await;
    assert_eq!(kept.len(), max);
    let names: Vec<String> = kept.iter().filter_map(|c| c.props.name.clone()).collect();
    assert!(names.iter().any(|n| n == "Work"));
}

fn requests(s: &Setup, method: &str) -> usize {
    s.fake
        .state
        .lock()
        .unwrap()
        .requests
        .iter()
        .filter(|r| r.starts_with(method))
        .count()
}

/// Calendar Server and Nextcloud sharing have no "free/busy only" (nor
/// "delete without writing"): refused before anything is sent, never
/// rounded up to more. With the ACL (Stalwart) free/busy is a
/// `read-free-busy` grant.
#[tokio::test]
async fn free_busy_only_is_not_rounded_up() {
    for kind in [Kind::Sabre, Kind::Nextcloud] {
        let s = setup(kind).await;
        let work = s.named("Work").await;
        let bob = s.principal("Bob").await;
        let posts = requests(&s, "POST");
        // Free/busy only has its own error (checked first).
        for rights in [Rights::UNBIND, Rights::READ.union(Rights::UNBIND)] {
            let refused = s
                .account
                .share_collection(
                    work.id,
                    &ShareUpdate {
                        set: vec![(bob.clone(), rights)],
                        ..ShareUpdate::default()
                    },
                )
                .await;
            assert!(
                matches!(
                    refused,
                    Err(GroupwareError::Share(ShareError::RightsNotSupported))
                ),
                "{refused:?}"
            );
        }
        assert_eq!(requests(&s, "POST"), posts);
        assert!(s.named("Work").await.props.sharing.shares.is_empty());
    }
    let s = setup(Kind::Stalwart).await;
    let work = s.named("Work").await;
    let bob = s.principal("Bob").await;
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                set: vec![(bob, Rights::READ_FREE_BUSY)],
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    let sent = s.fake.last("ACL").unwrap();
    assert!(
        sent.contains(
            "<D:href>/dav/principals/bob/</D:href></D:principal><D:grant><D:privilege><C:read-free-busy/></D:privilege></D:grant>"
        ),
        "{sent}"
    );
}

/// sabre/dav lists bob by his other address with read and write: making
/// it read changes that entry, and none is left with more.
#[tokio::test]
async fn sabre_like_lowering_by_another_address() {
    let s = setup(Kind::Sabre).await;
    s.fake
        .state
        .lock()
        .unwrap()
        .sharees
        .push(("mailto:bob.old@example.com".to_owned(), "read-write"));
    s.account.sync_round(false).await.unwrap();
    let work = s.named("Work").await;
    assert_eq!(work.props.sharing.shares.len(), 1);
    let bob = s.principal("Bob").await;
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                set: vec![(bob, READ_RIGHTS)],
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    let sent = s.fake.last("POST").unwrap();
    assert!(
        sent.contains("<CS:set><D:href>mailto:bob.old@example.com</D:href><CS:read/></CS:set>"),
        "{sent}"
    );
    assert_eq!(
        s.named("Work").await.props.sharing.shares,
        [(
            Sharee::new("mailto:bob.old@example.com").unwrap(),
            READ_RIGHTS
        )]
    );
}

/// A `shareWith` set whole drops bob; the server answers yes but keeps
/// him: the change is reported as not done.
#[tokio::test]
async fn sabre_like_ignored_removal_by_replace() {
    let s = setup(Kind::Sabre).await;
    let work = s.named("Work").await;
    let bob = s.principal("Bob").await;
    let carol = s.principal("Carol").await;
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                set: vec![(bob, READ_RIGHTS), (carol.clone(), READ_RIGHTS)],
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(s.named("Work").await.props.sharing.shares.len(), 2);
    s.fake.state.lock().unwrap().ignore_removes = true;
    let ignored = s
        .account
        .share_collection(
            work.id,
            &ShareUpdate {
                replace: true,
                set: vec![(carol, READ_RIGHTS)],
                ..ShareUpdate::default()
            },
        )
        .await;
    assert!(
        matches!(ignored, Err(GroupwareError::Share(ShareError::NotRemoved))),
        "{ignored:?}"
    );
}

/// The directory's `U{n}` principals, in order.
async fn many(s: &Setup, count: usize) -> Vec<nebula_groupware::principals::Principal> {
    let all = s.account.principals().await.unwrap();
    (0..count)
        .map(|n| {
            all.iter()
                .find(|p| p.name.as_deref() == Some(&format!("U{n}")))
                .cloned()
                .unwrap()
        })
        .collect()
}

fn posts_of(s: &Setup) -> Vec<String> {
    s.fake
        .state
        .lock()
        .unwrap()
        .requests
        .iter()
        .filter(|r| r.starts_with("POST"))
        .cloned()
        .collect()
}

fn entries(post: &str) -> usize {
    post.matches("<CS:set>").count()
        + post.matches("<CS:remove>").count()
        + post.matches("<O:set>").count()
        + post.matches("<O:remove>").count()
}

const RW_RIGHTS: Rights = Rights::READ
    .union(Rights::READ_FREE_BUSY)
    .union(Rights::WRITE_CONTENT)
    .union(Rights::WRITE_PROPERTIES)
    .union(Rights::BIND)
    .union(Rights::UNBIND);

/// Stalwart: an ACL with 300 grants goes in one `ACL` request, the store
/// keeps the 300 (mode still ACL), and a replacement with 150 leaves 150
/// and the ACE the bridge did not write.
#[tokio::test]
async fn stalwart_like_acl_many() {
    let s = setup_with(Kind::Stalwart, |state| state.people = 300, |_| {}).await;
    let work = s.named("Work").await;
    let people = many(&s, 300).await;
    let acls = requests(&s, "ACL");
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                replace: true,
                set: people.iter().map(|p| (p.clone(), READ_RIGHTS)).collect(),
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(requests(&s, "ACL"), acls + 1);
    let acl = s.fake.state.lock().unwrap().acl.clone();
    assert_eq!(acl.matches("<D:ace>").count(), 301, "the foreign one too");
    let work = s.named("Work").await;
    assert_eq!(work.props.sharing.mode, Some(ShareMode::Acl));
    assert_eq!(work.props.sharing.shares.len(), 300);
    assert!(
        work.props
            .sharing
            .shares
            .iter()
            .all(|(_, r)| *r == READ_RIGHTS)
    );
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                replace: true,
                set: people[..150]
                    .iter()
                    .map(|p| (p.clone(), READ_RIGHTS))
                    .collect(),
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    let work = s.named("Work").await;
    assert_eq!(work.props.sharing.shares.len(), 150);
    let acl = s.fake.state.lock().unwrap().acl.clone();
    assert!(acl.contains("<D:authenticated/>"), "{acl}");
}

/// sabre/dav: 600 sharees go in three `CS:share` (256 + 256 + 88), and
/// the store lists the 600.
#[tokio::test]
async fn sabre_like_many() {
    let s = setup_with(Kind::Sabre, |state| state.people = 600, |_| {}).await;
    let work = s.named("Work").await;
    let people = many(&s, 600).await;
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                set: people.iter().map(|p| (p.clone(), READ_RIGHTS)).collect(),
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    let sizes: Vec<usize> = posts_of(&s).iter().map(|p| entries(p)).collect();
    assert_eq!(sizes, [256, 256, 88]);
    let work = s.named("Work").await;
    assert_eq!(work.props.sharing.mode, Some(ShareMode::CalendarServer));
    assert_eq!(work.props.sharing.shares.len(), 600);
    // Removing 300 and lowering nobody: removals only, two requests.
    let before = posts_of(&s).len();
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                remove: people[..300].to_vec(),
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    let sizes: Vec<usize> = posts_of(&s)[before..].iter().map(|p| entries(p)).collect();
    assert_eq!(sizes, [256, 44]);
    assert_eq!(s.named("Work").await.props.sharing.shares.len(), 300);
}

/// Nextcloud: the same with `O:share`; every principal given access is
/// among the sharees afterwards.
#[tokio::test]
async fn nextcloud_like_many() {
    let s = setup_with(Kind::Nextcloud, |state| state.people = 600, |_| {}).await;
    let work = s.named("Work").await;
    let people = many(&s, 600).await;
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                set: people.iter().map(|p| (p.clone(), READ_RIGHTS)).collect(),
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    // Nextcloud takes at most 10 a `POST`: 60 of them.
    let sizes: Vec<usize> = posts_of(&s).iter().map(|p| entries(p)).collect();
    assert_eq!(sizes, [10; 60]);
    let work = s.named("Work").await;
    assert_eq!(work.props.sharing.mode, Some(ShareMode::Nextcloud));
    assert_eq!(work.props.sharing.shares.len(), 600);
}

fn sabre_sharees(count: usize, access: &'static str) -> Vec<(String, &'static str)> {
    (0..count)
        .map(|n| (format!("mailto:u{n}@example.com"), access))
        .collect()
}

/// A change in several requests that fails half way: the store is read
/// again and nobody has more than before or than asked (300 lowered to
/// read, 300 new with read-write; the second request fails).
#[tokio::test]
async fn chunked_failure_never_gives_more() {
    let s = setup_with(
        Kind::Sabre,
        |state| {
            state.people = 600;
            state.sharees = sabre_sharees(300, "read-write");
        },
        |_| {},
    )
    .await;
    let work = s.named("Work").await;
    let people = many(&s, 600).await;
    s.fake.state.lock().unwrap().fail_post_at = Some(1);
    let mut set: Vec<_> = people[..300]
        .iter()
        .map(|p| (p.clone(), READ_RIGHTS))
        .collect();
    set.extend(people[300..].iter().map(|p| (p.clone(), RW_RIGHTS)));
    let failed = s
        .account
        .share_collection(
            work.id,
            &ShareUpdate {
                set,
                ..ShareUpdate::default()
            },
        )
        .await;
    assert!(failed.is_err(), "{failed:?}");
    // The store shows what the server did: the first 256 lowered.
    let work = s.named("Work").await;
    let shares = &work.props.sharing.shares;
    assert_eq!(shares.len(), 300);
    let read = shares.iter().filter(|(_, r)| *r == READ_RIGHTS).count();
    assert_eq!(read, 256);
    let new: Vec<String> = (300..600)
        .map(|n| format!("mailto:u{n}@example.com"))
        .collect();
    assert!(
        !shares
            .iter()
            .any(|(sharee, _)| new.iter().any(|n| n == sharee.as_str()))
    );
}

/// Between two requests the server gives someone more than asked (another
/// client): the change is not a success.
#[tokio::test]
async fn chunked_upgrade_by_server_is_ignored() {
    let s = setup_with(Kind::Sabre, |state| state.people = 600, |_| {}).await;
    let work = s.named("Work").await;
    let people = many(&s, 600).await;
    s.fake.state.lock().unwrap().upgrade_after_post = Some("mailto:u0@example.com".to_owned());
    let result = s
        .account
        .share_collection(
            work.id,
            &ShareUpdate {
                set: people.iter().map(|p| (p.clone(), READ_RIGHTS)).collect(),
                ..ShareUpdate::default()
            },
        )
        .await;
    assert!(
        matches!(result, Err(GroupwareError::Share(ShareError::Ignored))),
        "{result:?}"
    );
}

/// "Free/busy only" for one among 400: nothing is sent at all.
#[tokio::test]
async fn free_busy_only_among_many_sends_nothing() {
    for kind in [Kind::Sabre, Kind::Nextcloud] {
        let s = setup_with(kind, |state| state.people = 401, |_| {}).await;
        let work = s.named("Work").await;
        let people = many(&s, 401).await;
        let mut set: Vec<_> = people[..400]
            .iter()
            .map(|p| (p.clone(), READ_RIGHTS))
            .collect();
        set.push((people[400].clone(), Rights::READ_FREE_BUSY));
        let refused = s
            .account
            .share_collection(
                work.id,
                &ShareUpdate {
                    set,
                    ..ShareUpdate::default()
                },
            )
            .await;
        assert!(
            matches!(
                refused,
                Err(GroupwareError::Share(ShareError::FreeBusyOnly))
            ),
            "{refused:?}"
        );
        assert_eq!(requests(&s, "POST"), 0);
    }
}

/// Too much: nothing is sent. A change naming more than twice the
/// collection limit; and (Stalwart) one that would pass the account's
/// budget: two own collections of 300 with a budget of 500.
#[tokio::test]
async fn too_many_sends_nothing() {
    let s = setup_with(
        Kind::Stalwart,
        |state| state.people = 300,
        |config| config.max_account_shares = 500,
    )
    .await;
    let work = s.named("Work").await;
    let huge = ShareUpdate {
        remove_sharees: (0..=nebula_groupware::sharing::MAX_SHARE_CHANGE)
            .map(|n| Sharee::new(&format!("mailto:x{n}@example.com")).unwrap())
            .collect(),
        ..ShareUpdate::default()
    };
    let before = s.fake.state.lock().unwrap().requests.len();
    let refused = s.account.share_collection(work.id, &huge).await;
    assert!(
        matches!(refused, Err(GroupwareError::Share(ShareError::TooMany))),
        "{refused:?}"
    );
    assert_eq!(s.fake.state.lock().unwrap().requests.len(), before);
    let people = many(&s, 300).await;
    let all: Vec<_> = people.iter().map(|p| (p.clone(), READ_RIGHTS)).collect();
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                set: all.clone(),
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    let book = s.named("People").await;
    assert_eq!(book.props.sharing.mode, Some(ShareMode::Acl));
    let acls = requests(&s, "ACL");
    let refused = s
        .account
        .share_collection(
            book.id,
            &ShareUpdate {
                set: all,
                ..ShareUpdate::default()
            },
        )
        .await;
    assert!(
        matches!(refused, Err(GroupwareError::Share(ShareError::TooMany))),
        "{refused:?}"
    );
    assert_eq!(requests(&s, "ACL"), acls);
    assert!(s.fake.state.lock().unwrap().book_acl.is_empty());
}

/// A principal of another origin in a later request: refused before the
/// first one is sent.
#[tokio::test]
async fn foreign_principal_in_a_later_batch_sends_nothing() {
    let s = setup(Kind::Sabre).await;
    let work = s.named("Work").await;
    let mut set_addresses: Vec<_> = (0..300)
        .map(|n| {
            (
                nebula_dav::proto::CalendarAddress::parse(&format!("dan{n}@outside.example"))
                    .unwrap(),
                READ_RIGHTS,
            )
        })
        .collect();
    set_addresses.truncate(300);
    let refused = s
        .account
        .share_collection(
            work.id,
            &ShareUpdate {
                set_addresses,
                set_sharees: vec![(
                    Sharee::new("https://other.example/dav/principals/eve/").unwrap(),
                    RW_RIGHTS,
                )],
                ..ShareUpdate::default()
            },
        )
        .await;
    assert!(
        matches!(
            refused,
            Err(GroupwareError::Dav(nebula_dav::DavError::OriginNotAllowed))
        ),
        "{refused:?}"
    );
    assert_eq!(requests(&s, "POST"), 0);
}

/// One sharee named twice with different access (a principal and the
/// address it is listed with): one entry, the lesser access.
#[tokio::test]
async fn one_name_twice_gets_the_lesser_access() {
    let s = setup(Kind::Sabre).await;
    let work = s.named("Work").await;
    let bob = s.principal("Bob").await;
    let address = bob.addresses[0].as_str().to_owned();
    s.account
        .share_collection(
            work.id,
            &ShareUpdate {
                set: vec![(bob, READ_RIGHTS)],
                set_sharees: vec![(Sharee::new(&address).unwrap(), RW_RIGHTS)],
                ..ShareUpdate::default()
            },
        )
        .await
        .unwrap();
    let sent = s.fake.last("POST").unwrap();
    assert_eq!(sent.matches("<CS:set>").count(), 1, "{sent}");
    let sharees = s.fake.state.lock().unwrap().sharees.clone();
    assert_eq!(sharees, [(address, "read")]);
}

/// Out of time between two requests: what was sent stays, the store is
/// read again, and the change says it is unfinished.
#[tokio::test]
async fn out_of_time_between_requests() {
    let s = setup_with(
        Kind::Sabre,
        |state| state.people = 600,
        |config| config.share_deadline = Duration::from_millis(200),
    )
    .await;
    let work = s.named("Work").await;
    let people = many(&s, 600).await;
    s.fake.state.lock().unwrap().post_delay = Some(Duration::from_millis(300));
    let result = s
        .account
        .share_collection(
            work.id,
            &ShareUpdate {
                set: people.iter().map(|p| (p.clone(), READ_RIGHTS)).collect(),
                ..ShareUpdate::default()
            },
        )
        .await;
    assert!(
        matches!(result, Err(GroupwareError::Share(ShareError::Unfinished))),
        "{result:?}"
    );
    assert_eq!(requests(&s, "POST"), 1);
    assert_eq!(s.named("Work").await.props.sharing.shares.len(), 256);
}

fn stalwart_grants(count: usize) -> String {
    (0..count)
        .map(|n| {
            format!(
                "<D:ace><D:principal><D:href>/dav/principals/u{n}/</D:href></D:principal><D:grant><D:privilege><D:read/></D:privilege></D:grant></D:ace>"
            )
        })
        .collect::<Vec<_>>()
        .concat()
}

/// A home listing too big with the ACLs: listed again without them and
/// each ACL read apart; the round works, and the next one goes straight
/// to the split listing. A collection whose own answer is too big is
/// left without a mode, and the round still works.
#[tokio::test]
async fn oversized_home_listing_is_split() {
    let s = setup_with(
        Kind::Stalwart,
        |_| {},
        |config| {
            let mut limits = nebula_dav::Limits::default();
            limits.max_multistatus = 64 * 1024;
            config.server = config.server.clone().with_limits(limits);
        },
    )
    .await;
    let whole = |s: &Setup| {
        s.fake
            .state
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|r| r.starts_with("PROPFIND /dav/cal/alice/ ") && r.contains("acl/>"))
            .count()
    };
    // Each ACL about 30 KB: three of them pass 64 KiB together.
    s.change_on_server(|state| {
        state.acl = stalwart_grants(200);
        state.shared = 2;
        state.shared_acl = stalwart_grants(200);
    })
    .await;
    let work = s.named("Work").await;
    assert_eq!(work.props.sharing.mode, Some(ShareMode::Acl));
    assert_eq!(work.props.sharing.shares.len(), 200);
    assert!(s.named("Shared 1").await.props.sharing.owner.is_some());
    let tried = whole(&s);
    s.account.sync_round(false).await.unwrap();
    assert_eq!(whole(&s), tried, "not asked whole again");
    assert_eq!(s.named("Work").await.props.sharing.shares.len(), 200);
    // Its own answer too big: no mode, nobody listed, the round works.
    s.change_on_server(|state| state.acl = stalwart_grants(600))
        .await;
    let work = s.named("Work").await;
    assert_eq!(work.props.sharing.mode, None);
    assert!(work.props.sharing.shares.is_empty());
    assert!(s.named("Team").await.props.sharing.owner.is_some());
}

/// sabre/dav: a copy in alice's home whose invitation is too big to read
/// keeps the owner it had, never becomes alice's own; one never read
/// before is left out.
#[tokio::test]
async fn copy_without_its_invitation_is_never_own() {
    let big_users = (0..400)
        .map(|n| {
            format!(
                "<CS:user><CS:invite-accepted/><CS:access><CS:read/></CS:access><D:href>mailto:u{n}@example.com</D:href></CS:user>"
            )
        })
        .collect::<Vec<_>>().concat();
    let limits = |config: &mut GroupwareConfig| {
        let mut limits = nebula_dav::Limits::default();
        limits.max_multistatus = 32 * 1024;
        config.server = config.server.clone().with_limits(limits);
    };
    // Seen before: carol's still.
    let s = setup_with(Kind::Sabre, |_| {}, limits).await;
    let copy = s.named("Carol's").await;
    let carol = copy.props.sharing.owner.clone().unwrap();
    let users = big_users.clone();
    s.change_on_server(move |state| state.copy_users = users)
        .await;
    let copy = s.named("Carol's").await;
    assert_eq!(copy.props.sharing.owner, Some(carol));
    // Never seen: left out, not alice's.
    let s = setup_with(
        Kind::Sabre,
        move |state| state.copy_users = big_users,
        limits,
    )
    .await;
    assert!(
        !s.collections()
            .await
            .iter()
            .any(|c| c.props.name.as_deref() == Some("Carol's"))
    );
    assert!(s.named("Work").await.props.sharing.owner.is_none());
}
