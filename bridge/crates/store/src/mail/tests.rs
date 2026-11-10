// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Tests of the mail data: queries and the change records of every write.

use std::collections::BTreeSet;

use nebula_imap_proto::types::{MailboxName, ModSeq, Uid, UidValidity};
use nebula_jmap_proto::Keyword;
use nebula_jmap_proto::object::email::EmailAddress;

use super::*;
use crate::account::tests::alice;
use crate::db::test_support::store;
use crate::{
    AccountId, ChangeKind, Changes, ChangesSince, DataType, EmailId, MailboxId, Seq, Store,
    ThreadId, UnixTime,
};

const NOW: UnixTime = UnixTime::from_secs(2_000_000_000);

fn name(text: &str) -> MailboxName {
    MailboxName::new(text).unwrap()
}

fn listed(text: &str, role: Option<Role>) -> ListedMailbox {
    let (parent, display) = match text.rsplit_once('/') {
        Some((parent, leaf)) => (Some(name(parent)), leaf.to_owned()),
        None => (None, text.to_owned()),
    };
    ListedMailbox {
        imap_name: name(text),
        name: display,
        delimiter: Some('/'),
        parent,
        role,
        subscribed: true,
        selectable: true,
        no_inferiors: false,
        sort_order: 0,
    }
}

fn uid(n: u32) -> Uid {
    Uid::new(n).unwrap()
}

fn keywords(list: &[&str]) -> BTreeSet<Keyword> {
    list.iter().map(|k| k.parse().unwrap()).collect()
}

fn meta(message_id: &str, subject: &str, received: i64) -> EmailMeta {
    EmailMeta {
        message_id: Some(message_id.to_owned()),
        in_reply_to: Vec::new(),
        references: Vec::new(),
        subject: Some(subject.to_owned()),
        from: Some(vec![EmailAddress {
            name: Some(format!("Sender {subject}")),
            email: "sender@example.com".to_owned(),
        }]),
        to: None,
        cc: None,
        bcc: None,
        reply_to: None,
        sender: None,
        sent_at: Some(SentAt {
            utc: UnixTime::from_secs(received - 60),
            offset_minutes: 120,
        }),
        received_at: UnixTime::from_secs(received),
        size: 1000,
        has_attachment: false,
        preview: None,
        headers: b"Subject: x\r\n".to_vec(),
    }
}

fn message(n: u32, meta: EmailMeta) -> NewMessage {
    NewMessage {
        uid: uid(n),
        mod_seq: None,
        keywords: BTreeSet::new(),
        server: ServerIds::default(),
        meta,
    }
}

struct Fixture {
    store: Store,
    account: AccountId,
    inbox: MailboxId,
    archive: MailboxId,
    trash: MailboxId,
    _dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let (store, dir) = store().await;
    let account = alice(&store).await.id;
    store
        .sync_mailboxes(
            account,
            vec![
                listed("INBOX", Some(Role::Inbox)),
                listed("Archive", Some(Role::Archive)),
                listed("Trash", Some(Role::Trash)),
            ],
            NOW,
        )
        .await
        .unwrap();
    let id = |n: &str| {
        let store = store.clone();
        let n = name(n);
        async move {
            store
                .mailbox_by_name(account, &n)
                .await
                .unwrap()
                .unwrap()
                .id
        }
    };
    let inbox = id("INBOX").await;
    let archive = id("Archive").await;
    let trash = id("Trash").await;
    for mailbox in [inbox, archive, trash] {
        store
            .reset_mailbox(account, mailbox, UidValidity::new(7).unwrap(), NOW)
            .await
            .unwrap();
    }
    Fixture {
        store,
        account,
        inbox,
        archive,
        trash,
        _dir: dir,
    }
}

impl Fixture {
    async fn add(&self, mailbox: MailboxId, messages: Vec<NewMessage>) -> Recorded {
        self.store
            .add_messages(self.account, mailbox, messages, NOW)
            .await
            .unwrap()
    }

    async fn email_at(&self, mailbox: MailboxId, n: u32) -> EmailId {
        self.store
            .email_at(self.account, mailbox, uid(n))
            .await
            .unwrap()
            .unwrap()
    }

    async fn email(&self, id: EmailId) -> Email {
        self.store
            .emails(self.account, vec![id])
            .await
            .unwrap()
            .pop()
            .unwrap()
    }

    async fn state(&self, data_type: DataType) -> Seq {
        self.store.state(self.account, data_type).await.unwrap()
    }

    async fn changes(&self, data_type: DataType, since: Seq) -> Changes {
        match self
            .store
            .changes_since(self.account, data_type, since, 1000)
            .await
            .unwrap()
        {
            ChangesSince::Changes(changes) => changes,
            ChangesSince::CannotCalculate | ChangesSince::TooMany => panic!("cannot calculate"),
        }
    }

    async fn counts(&self, mailbox: MailboxId) -> MailboxCounts {
        self.store
            .mailbox_counts(self.account)
            .await
            .unwrap()
            .get(&mailbox)
            .copied()
            .unwrap_or_default()
    }

    async fn query(&self, query: EmailQuery) -> EmailQueryResult {
        self.store.query_emails(self.account, query).await.unwrap()
    }
}

fn all(filter: Option<EmailFilter>) -> EmailQuery {
    EmailQuery {
        filter,
        sort: Vec::new(),
        collapse_threads: false,
        start: QueryStart::Position(0),
        limit: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn mailbox_list_sync_records_changes() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    let first = store
        .sync_mailboxes(
            account,
            vec![
                listed("INBOX", Some(Role::Inbox)),
                listed("Work", None),
                listed("Work/2026", None),
                // A second "Sent": the role stays with the first.
                listed("Sent", Some(Role::Sent)),
                listed("Sent Items", Some(Role::Sent)),
            ],
            NOW,
        )
        .await
        .unwrap();
    let s1 = first.state(DataType::Mailbox).unwrap();
    let boxes = store.mailboxes(account).await.unwrap();
    assert_eq!(boxes.len(), 5);
    let by = |n: &str| boxes.iter().find(|m| m.imap_name.as_str() == n).unwrap();
    assert_eq!(by("Work/2026").parent, Some(by("Work").id));
    assert_eq!(by("Work/2026").name, "2026");
    assert_eq!(by("Sent").role, Some(Role::Sent));
    assert_eq!(by("Sent Items").role, None);

    // Same list again: nothing changes.
    let again = store
        .sync_mailboxes(
            account,
            vec![
                listed("INBOX", Some(Role::Inbox)),
                listed("Work", None),
                listed("Work/2026", None),
                listed("Sent", Some(Role::Sent)),
                listed("Sent Items", Some(Role::Sent)),
            ],
            NOW,
        )
        .await
        .unwrap();
    assert!(again.is_empty());

    // "Sent" disappears: "Sent Items" takes the role, "Work" goes away and
    // its child moves to the top.
    let mut work_child = listed("Work/2026", None);
    work_child.parent = None;
    work_child.name = "Work/2026".to_owned();
    store
        .sync_mailboxes(
            account,
            vec![
                listed("INBOX", Some(Role::Inbox)),
                work_child,
                listed("Sent Items", Some(Role::Sent)),
            ],
            NOW,
        )
        .await
        .unwrap();
    let after = store.mailboxes(account).await.unwrap();
    assert_eq!(after.len(), 3);
    let changes = match store
        .changes_since(account, DataType::Mailbox, s1, 100)
        .await
        .unwrap()
    {
        ChangesSince::Changes(changes) => changes,
        ChangesSince::CannotCalculate | ChangesSince::TooMany => panic!(),
    };
    let mut destroyed = changes.destroyed.clone();
    destroyed.sort_unstable();
    let mut expected = vec![by("Work").id.get(), by("Sent").id.get()];
    expected.sort_unstable();
    assert_eq!(destroyed, expected);
    let mut updated = changes.updated.clone();
    updated.sort_unstable();
    let mut expected = vec![by("Work/2026").id.get(), by("Sent Items").id.get()];
    expected.sort_unstable();
    assert_eq!(updated, expected);
    let sent_items = store
        .mailbox_by_role(account, Role::Sent)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sent_items.imap_name.as_str(), "Sent Items");
}

#[tokio::test(flavor = "multi_thread")]
async fn new_message_creates_email_thread_and_updates_counts() {
    let f = fixture().await;
    let (m0, e0, t0) = (
        f.state(DataType::Mailbox).await,
        f.state(DataType::Email).await,
        f.state(DataType::Thread).await,
    );
    let recorded = f
        .add(f.inbox, vec![message(1, meta("a@x", "Hola", 100))])
        .await;
    // Mailbox, Email, Thread and EmailDelivery (a new email).
    assert_eq!(recorded.data_types().count(), 4, "{recorded:?}");
    assert!(recorded.state(DataType::EmailDelivery).is_some());
    let id = f.email_at(f.inbox, 1).await;
    // The new email, for EmailPush; gone when it is not a delivery.
    assert_eq!(recorded.delivered(), [id]);
    let mut forgotten = recorded.clone();
    forgotten.forget(DataType::EmailDelivery);
    assert!(forgotten.delivered().is_empty());
    let mut own = recorded.clone();
    own.not_delivered();
    assert!(own.delivered().is_empty() && own.state(DataType::EmailDelivery).is_some());
    let email = f.email(id).await;
    assert_eq!(email.mailboxes, BTreeSet::from([f.inbox]));
    assert_eq!(email.meta.subject.as_deref(), Some("Hola"));
    assert_eq!(f.changes(DataType::Email, e0).await.created, vec![id.get()]);
    assert_eq!(
        f.changes(DataType::Thread, t0).await.created,
        vec![email.thread.get()]
    );
    assert_eq!(
        f.changes(DataType::Mailbox, m0).await.updated,
        vec![f.inbox.get()]
    );
    assert_eq!(
        f.counts(f.inbox).await,
        MailboxCounts {
            total_emails: 1,
            unread_emails: 1,
            total_threads: 1,
            unread_threads: 1
        }
    );

    // The same batch again (retry): no change at all.
    let again = f
        .add(f.inbox, vec![message(1, meta("a@x", "Hola", 100))])
        .await;
    assert!(again.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn flags_update_keywords_and_counts() {
    let f = fixture().await;
    f.add(f.inbox, vec![message(1, meta("a@x", "Hola", 100))])
        .await;
    let id = f.email_at(f.inbox, 1).await;
    let (m0, e0, t0) = (
        f.state(DataType::Mailbox).await,
        f.state(DataType::Email).await,
        f.state(DataType::Thread).await,
    );
    let applied = f
        .store
        .update_flags(
            f.account,
            f.inbox,
            vec![
                FlagUpdate {
                    uid: uid(1),
                    mod_seq: ModSeq::new(10),
                    keywords: keywords(&["$seen", "$flagged"]),
                },
                FlagUpdate {
                    uid: uid(99),
                    mod_seq: None,
                    keywords: BTreeSet::new(),
                },
            ],
            NOW,
        )
        .await
        .unwrap();
    assert_eq!(applied.unknown, vec![uid(99)]);
    // Flags are not a delivery.
    assert!(applied.recorded.state(DataType::EmailDelivery).is_none());
    assert!(applied.recorded.delivered().is_empty());
    assert_eq!(f.changes(DataType::Email, e0).await.updated, vec![id.get()]);
    assert_eq!(
        f.changes(DataType::Mailbox, m0).await.updated,
        vec![f.inbox.get()]
    );
    // Keywords don't change the thread.
    assert_eq!(f.state(DataType::Thread).await, t0);
    assert_eq!(f.counts(f.inbox).await.unread_emails, 0);
    assert_eq!(f.counts(f.inbox).await.unread_threads, 0);

    // Only $flagged changes: the email changes, the counts don't.
    let m1 = f.state(DataType::Mailbox).await;
    f.store
        .update_flags(
            f.account,
            f.inbox,
            vec![FlagUpdate {
                uid: uid(1),
                mod_seq: ModSeq::new(11),
                keywords: keywords(&["$seen"]),
            }],
            NOW,
        )
        .await
        .unwrap();
    assert_eq!(f.state(DataType::Mailbox).await, m1);

    assert_eq!(f.email(id).await.keywords, keywords(&["$seen"]));
    let stored = f.store.mailbox_uids(f.account, f.inbox).await.unwrap();
    assert_eq!(stored[0].mod_seq, ModSeq::new(11));
}

#[tokio::test(flavor = "multi_thread")]
#[expect(
    clippy::many_single_char_names,
    reason = "a, b, c, d are the four emails"
)]
async fn local_threads_join_and_merge() {
    let f = fixture().await;
    let mut reply = meta("b@x", "Re: Hola", 200);
    reply.in_reply_to = vec!["a@x".to_owned()];
    let other = meta("c@x", "Hola", 300);
    f.add(
        f.inbox,
        vec![
            message(1, meta("a@x", "Hola", 100)),
            message(2, reply),
            message(3, other),
        ],
    )
    .await;
    let a = f.email(f.email_at(f.inbox, 1).await).await;
    let b = f.email(f.email_at(f.inbox, 2).await).await;
    let c = f.email(f.email_at(f.inbox, 3).await).await;
    assert_eq!(a.thread, b.thread);
    // Same subject but no reference: another thread.
    assert_ne!(a.thread, c.thread);
    assert_eq!(f.counts(f.inbox).await.total_threads, 2);

    // A message that refers to both merges them; the newer thread goes.
    let (e0, t0) = (
        f.state(DataType::Email).await,
        f.state(DataType::Thread).await,
    );
    let mut both = meta("d@x", "Re: Hola", 400);
    both.references = vec!["a@x".to_owned(), "c@x".to_owned()];
    f.add(f.inbox, vec![message(4, both)]).await;
    let d = f.email(f.email_at(f.inbox, 4).await).await;
    let survivor = a.thread.min(c.thread);
    let gone = a.thread.max(c.thread);
    assert_eq!(d.thread, survivor);
    for email in [&a, &b, &c] {
        assert_eq!(f.email(email.id).await.thread, survivor);
    }
    let threads = f.changes(DataType::Thread, t0).await;
    assert_eq!(threads.destroyed, vec![gone.get()]);
    assert_eq!(threads.updated, vec![survivor.get()]);
    let emails = f.changes(DataType::Email, e0).await;
    assert_eq!(emails.created, vec![d.id.get()]);
    let moved: Vec<i64> = [&a, &b, &c]
        .iter()
        .filter(|e| e.thread == gone)
        .map(|e| e.id.get())
        .collect();
    assert_eq!(emails.updated, moved);

    // Thread/get: oldest first.
    let list = f
        .store
        .thread_emails(f.account, vec![survivor, gone])
        .await
        .unwrap();
    assert_eq!(list, vec![(survivor, vec![a.id, b.id, c.id, d.id])]);
}

/// Headers only join a thread with the same base subject: citing the ids
/// of other conversations, or reusing one's Message-ID, fuses nothing.
#[tokio::test(flavor = "multi_thread")]
async fn local_threads_need_the_same_base_subject() {
    let f = fixture().await;
    f.add(
        f.inbox,
        vec![
            message(1, meta("a@x", "Asunto", 100)),
            message(2, meta("c@x", "Otra cosa", 110)),
        ],
    )
    .await;
    let a = f.email(f.email_at(f.inbox, 1).await).await;
    let c = f.email(f.email_at(f.inbox, 2).await).await;
    assert_ne!(a.thread, c.thread);

    // Another subject, References to both threads: a thread of its own, and
    // nothing moves.
    let (e0, t0) = (
        f.state(DataType::Email).await,
        f.state(DataType::Thread).await,
    );
    let mut cites = meta("d@x", "Factura urgente", 120);
    cites.references = vec!["a@x".to_owned(), "c@x".to_owned()];
    f.add(f.inbox, vec![message(3, cites)]).await;
    let d = f.email(f.email_at(f.inbox, 3).await).await;
    assert!(d.thread != a.thread && d.thread != c.thread);
    assert_eq!(f.email(a.id).await.thread, a.thread);
    assert_eq!(f.email(c.id).await.thread, c.thread);
    let emails = f.changes(DataType::Email, e0).await;
    assert_eq!(emails.created, vec![d.id.get()]);
    assert!(emails.updated.is_empty());
    assert!(f.changes(DataType::Thread, t0).await.destroyed.is_empty());

    // Its own Message-ID is the first email's, with another subject: apart.
    f.add(f.inbox, vec![message(4, meta("a@x", "Hola", 130))])
        .await;
    let same_id = f.email(f.email_at(f.inbox, 4).await).await;
    assert_ne!(same_id.thread, a.thread);

    // Prefixes and list tags are not part of the base subject.
    for (n, subject) in [
        (5, "Re: Asunto"),
        (6, "AW: Asunto"),
        (7, "[lista] Re: Asunto"),
    ] {
        let mut reply = meta(&format!("r{n}@x"), subject, 140);
        reply.in_reply_to = vec!["a@x".to_owned()];
        f.add(f.inbox, vec![message(n, reply)]).await;
        let reply = f.email(f.email_at(f.inbox, n).await).await;
        assert_eq!(reply.thread, a.thread, "{subject}");
    }
    // A reply with its subject changed starts a new thread.
    let mut renamed = meta("r8@x", "Re: Cambio de tema", 150);
    renamed.in_reply_to = vec!["a@x".to_owned()];
    f.add(f.inbox, vec![message(8, renamed)]).await;
    let renamed = f.email(f.email_at(f.inbox, 8).await).await;
    assert_ne!(renamed.thread, a.thread);

    // Empty subjects match each other.
    let mut empty = meta("e@x", "", 160);
    empty.subject = None;
    f.add(f.inbox, vec![message(9, empty)]).await;
    let mut empty_reply = meta("e2@x", "", 170);
    empty_reply.in_reply_to = vec!["e@x".to_owned()];
    f.add(f.inbox, vec![message(10, empty_reply)]).await;
    let first = f.email(f.email_at(f.inbox, 9).await).await;
    let second = f.email(f.email_at(f.inbox, 10).await).await;
    assert_eq!(first.thread, second.thread);
}

/// A thread whose only email left every mailbox (but is still kept, with
/// its server id) still takes replies.
#[tokio::test(flavor = "multi_thread")]
async fn orphaned_emails_still_join_replies() {
    let f = fixture().await;
    let original = NewMessage {
        server: ServerIds {
            email: Some(ServerId::new("M1").unwrap()),
            thread: None,
        },
        ..message(1, meta("a@x", "Asunto", 100))
    };
    f.add(f.inbox, vec![original]).await;
    let a = f.email(f.email_at(f.inbox, 1).await).await;
    f.store
        .remove_messages(f.account, f.inbox, vec![uid(1)], NOW)
        .await
        .unwrap();
    let mut reply = meta("b@x", "Re: Asunto", 200);
    reply.in_reply_to = vec!["a@x".to_owned()];
    f.add(f.inbox, vec![message(2, reply)]).await;
    let b = f.email(f.email_at(f.inbox, 2).await).await;
    assert_eq!(b.thread, a.thread);
}

#[tokio::test(flavor = "multi_thread")]
async fn server_ids_merge_copies_and_survive_resets() {
    let f = fixture().await;
    let with_ids = |n: u32, mailbox_uid_meta: EmailMeta| NewMessage {
        server: ServerIds {
            email: Some(ServerId::new("M1").unwrap()),
            thread: Some(ServerId::new("T1").unwrap()),
        },
        ..message(n, mailbox_uid_meta)
    };
    f.add(f.inbox, vec![with_ids(1, meta("a@x", "Hola", 100))])
        .await;
    f.add(f.archive, vec![with_ids(5, meta("a@x", "Hola", 100))])
        .await;
    let id = f.email_at(f.inbox, 1).await;
    assert_eq!(f.email_at(f.archive, 5).await, id);
    let email = f.email(id).await;
    assert_eq!(email.mailboxes, BTreeSet::from([f.inbox, f.archive]));
    assert_eq!(email.server_id.as_ref().map(ServerId::as_str), Some("M1"));
    assert_eq!(f.counts(f.archive).await.total_emails, 1);

    // UIDVALIDITY of both mailboxes changes: the email disappears…
    let e0 = f.state(DataType::Email).await;
    for mailbox in [f.inbox, f.archive] {
        f.store
            .reset_mailbox(f.account, mailbox, UidValidity::new(8).unwrap(), NOW)
            .await
            .unwrap();
    }
    assert!(
        f.store
            .emails(f.account, vec![id])
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.changes(DataType::Email, e0).await.destroyed,
        vec![id.get()]
    );
    let after_reset = f.state(DataType::Email).await;
    // …and comes back with the same id when the resync finds it.
    f.add(f.inbox, vec![with_ids(70, meta("a@x", "Hola", 100))])
        .await;
    assert_eq!(f.email_at(f.inbox, 70).await, id);
    assert_eq!(
        f.changes(DataType::Email, after_reset).await.created,
        vec![id.get()]
    );
    // From before the reset, a client just sees an update.
    assert_eq!(f.changes(DataType::Email, e0).await.updated, vec![id.get()]);

    // An orphan is purged after a while.
    f.store
        .remove_messages(f.account, f.inbox, vec![uid(70)], NOW)
        .await
        .unwrap();
    let purged = f
        .store
        .purge_orphans(f.account, UnixTime::from_secs(NOW.secs() + 1))
        .await
        .unwrap();
    assert_eq!(purged, 1);
    f.add(f.inbox, vec![with_ids(71, meta("a@x", "Hola", 100))])
        .await;
    assert_ne!(f.email_at(f.inbox, 71).await, id);
}

#[tokio::test(flavor = "multi_thread")]
async fn reset_destroys_emails_without_server_ids() {
    let f = fixture().await;
    f.add(f.inbox, vec![message(1, meta("a@x", "Hola", 100))])
        .await;
    let id = f.email_at(f.inbox, 1).await;
    let thread = f.email(id).await.thread;
    let (e0, t0) = (
        f.state(DataType::Email).await,
        f.state(DataType::Thread).await,
    );
    f.store
        .reset_mailbox(f.account, f.inbox, UidValidity::new(9).unwrap(), NOW)
        .await
        .unwrap();
    assert_eq!(
        f.changes(DataType::Email, e0).await.destroyed,
        vec![id.get()]
    );
    assert_eq!(
        f.changes(DataType::Thread, t0).await.destroyed,
        vec![thread.get()]
    );
    let mailbox = f.store.mailbox(f.account, f.inbox).await.unwrap();
    assert_eq!(mailbox.sync.uid_validity, UidValidity::new(9));
    assert_eq!(mailbox.sync.highest_mod_seq, None);
    // Same message again: a new email (no server id to match).
    f.add(f.inbox, vec![message(1, meta("a@x", "Hola", 100))])
        .await;
    assert_ne!(f.email_at(f.inbox, 1).await, id);
}

#[tokio::test(flavor = "multi_thread")]
async fn moves_keep_the_id_and_update_both_counts() {
    let f = fixture().await;
    f.add(f.inbox, vec![message(1, meta("a@x", "Hola", 100))])
        .await;
    let id = f.email_at(f.inbox, 1).await;
    let (m0, e0, t0) = (
        f.state(DataType::Mailbox).await,
        f.state(DataType::Email).await,
        f.state(DataType::Thread).await,
    );
    f.store
        .move_message(f.account, (f.inbox, uid(1)), (f.trash, uid(40)), NOW)
        .await
        .unwrap();
    assert_eq!(f.email_at(f.trash, 40).await, id);
    assert_eq!(f.email(id).await.mailboxes, BTreeSet::from([f.trash]));
    assert_eq!(f.changes(DataType::Email, e0).await.updated, vec![id.get()]);
    let mut updated = f.changes(DataType::Mailbox, m0).await.updated;
    updated.sort_unstable();
    assert_eq!(updated, vec![f.inbox.get(), f.trash.get()]);
    // The thread still has the same email: no thread change.
    assert_eq!(f.state(DataType::Thread).await, t0);
    let locations = f.store.locations(f.account, id).await.unwrap();
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].imap_name.as_str(), "Trash");
    assert_eq!(locations[0].uid, uid(40));
    assert_eq!(locations[0].uid_validity, UidValidity::new(7));

    // A copy adds a mailbox.
    f.store
        .add_location(f.account, id, (f.archive, uid(3)), NOW)
        .await
        .unwrap();
    assert_eq!(
        f.email(id).await.mailboxes,
        BTreeSet::from([f.archive, f.trash])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn trash_counts_apart_for_unread_threads() {
    let f = fixture().await;
    let mut reply = meta("b@x", "Re: Hola", 200);
    reply.in_reply_to = vec!["a@x".to_owned()];
    f.add(f.inbox, vec![message(1, meta("a@x", "Hola", 100))])
        .await;
    f.add(f.trash, vec![message(1, reply)]).await;
    // Inbox email read; the unread reply is in Trash.
    f.store
        .update_flags(
            f.account,
            f.inbox,
            vec![FlagUpdate {
                uid: uid(1),
                mod_seq: None,
                keywords: keywords(&["$seen"]),
            }],
            NOW,
        )
        .await
        .unwrap();
    assert_eq!(f.counts(f.inbox).await.unread_threads, 0);
    assert_eq!(f.counts(f.trash).await.unread_threads, 1);
    assert_eq!(f.counts(f.inbox).await.total_threads, 1);

    // Marking the Trash copy read changes the counts of the Inbox's thread
    // too, so the Inbox gets an update.
    let m0 = f.state(DataType::Mailbox).await;
    f.store
        .update_flags(
            f.account,
            f.trash,
            vec![FlagUpdate {
                uid: uid(1),
                mod_seq: None,
                keywords: keywords(&["$seen"]),
            }],
            NOW,
        )
        .await
        .unwrap();
    let mut updated = f.changes(DataType::Mailbox, m0).await.updated;
    updated.sort_unstable();
    assert_eq!(updated, vec![f.inbox.get(), f.trash.get()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_the_last_copy_destroys_email_and_thread() {
    let f = fixture().await;
    f.add(f.inbox, vec![message(1, meta("a@x", "Hola", 100))])
        .await;
    let id = f.email_at(f.inbox, 1).await;
    let thread = f.email(id).await.thread;
    let t0 = f.state(DataType::Thread).await;
    f.store
        .remove_messages(f.account, f.inbox, vec![uid(1), uid(2)], NOW)
        .await
        .unwrap();
    assert!(
        f.store
            .emails(f.account, vec![id])
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.changes(DataType::Thread, t0).await.destroyed,
        vec![thread.get()]
    );
    assert!(
        f.store
            .thread_emails(f.account, vec![thread])
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(f.counts(f.inbox).await, MailboxCounts::default());
}

#[tokio::test(flavor = "multi_thread")]
async fn deleting_a_mailbox_destroys_its_emails() {
    let f = fixture().await;
    f.add(f.archive, vec![message(1, meta("a@x", "Hola", 100))])
        .await;
    let id = f.email_at(f.archive, 1).await;
    let e0 = f.state(DataType::Email).await;
    f.store
        .sync_mailboxes(
            f.account,
            vec![
                listed("INBOX", Some(Role::Inbox)),
                listed("Trash", Some(Role::Trash)),
            ],
            NOW,
        )
        .await
        .unwrap();
    assert_eq!(
        f.changes(DataType::Email, e0).await.destroyed,
        vec![id.get()]
    );
    assert!(f.store.mailbox(f.account, f.archive).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn rename_keeps_ids_and_moves_children() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    store
        .sync_mailboxes(
            account,
            vec![listed("Work", None), listed("Work/2026", None)],
            NOW,
        )
        .await
        .unwrap();
    let work = store
        .mailbox_by_name(account, &name("Work"))
        .await
        .unwrap()
        .unwrap();
    let child = store
        .mailbox_by_name(account, &name("Work/2026"))
        .await
        .unwrap()
        .unwrap();
    store
        .rename_mailbox(account, work.id, name("Jobs"), "Jobs".to_owned(), NOW)
        .await
        .unwrap();
    let renamed = store.mailbox(account, work.id).await.unwrap();
    assert_eq!(renamed.imap_name.as_str(), "Jobs");
    assert_eq!(renamed.name, "Jobs");
    let moved = store.mailbox(account, child.id).await.unwrap();
    assert_eq!(moved.imap_name.as_str(), "Jobs/2026");
    assert_eq!(moved.parent, Some(work.id));
}

#[tokio::test(flavor = "multi_thread")]
async fn sync_state_round_trips_and_guards_uidvalidity() {
    let f = fixture().await;
    let state = MailboxSyncState {
        uid_validity: UidValidity::new(7),
        uid_next: Uid::new(42),
        highest_mod_seq: ModSeq::new(1234),
        messages: Some(41),
        synced_at: Some(NOW),
    };
    f.store
        .set_mailbox_sync_state(f.account, f.inbox, state)
        .await
        .unwrap();
    assert_eq!(
        f.store.mailbox(f.account, f.inbox).await.unwrap().sync,
        state
    );
    let wrong = MailboxSyncState {
        uid_validity: UidValidity::new(8),
        ..state
    };
    assert!(
        f.store
            .set_mailbox_sync_state(f.account, f.inbox, wrong)
            .await
            .is_err()
    );
    // A reset starts the sync over: the next copy counts as the first.
    f.store
        .reset_mailbox(f.account, f.inbox, UidValidity::new(8).unwrap(), NOW)
        .await
        .unwrap();
    let reset = f.store.mailbox(f.account, f.inbox).await.unwrap().sync;
    assert_eq!(reset.synced_at, None);
    assert_eq!(reset.uid_validity, UidValidity::new(8));
}

#[tokio::test(flavor = "multi_thread")]
async fn other_accounts_see_nothing() {
    let f = fixture().await;
    f.add(f.inbox, vec![message(1, meta("a@x", "Hola", 100))])
        .await;
    let id = f.email_at(f.inbox, 1).await;
    let bob = f
        .store
        .get_or_create_account(
            crate::NewAccount {
                login: "bob",
                email: "bob@example.com",
                wrapped_dek: b"dek",
            },
            NOW,
        )
        .await
        .unwrap()
        .id;
    assert!(f.store.emails(bob, vec![id]).await.unwrap().is_empty());
    assert!(f.store.locations(bob, id).await.unwrap().is_empty());
    assert!(f.store.mailbox(bob, f.inbox).await.is_err());
    assert!(
        f.store
            .add_messages(bob, f.inbox, Vec::new(), NOW)
            .await
            .is_err()
    );
    assert_eq!(f.store.query_emails(bob, all(None)).await.unwrap().total, 0);
}

async fn query_fixture() -> (Fixture, Vec<EmailId>) {
    let f = fixture().await;
    let mut big = meta("big@x", "Zeta", 400);
    big.size = 50_000;
    big.has_attachment = true;
    let mut reply = meta("r@x", "Re: Alpha", 500);
    reply.in_reply_to = vec!["a@x".to_owned()];
    f.add(
        f.inbox,
        vec![
            message(1, meta("a@x", "Alpha", 100)),
            message(2, meta("b@x", "Beta", 200)),
            message(3, big),
            message(4, reply),
        ],
    )
    .await;
    f.add(f.archive, vec![message(1, meta("c@x", "Gamma", 300))])
        .await;
    f.store
        .update_flags(
            f.account,
            f.inbox,
            vec![FlagUpdate {
                uid: uid(2),
                mod_seq: None,
                keywords: keywords(&["$seen", "$flagged"]),
            }],
            NOW,
        )
        .await
        .unwrap();
    let mut ids = Vec::new();
    for n in 1..=4 {
        ids.push(f.email_at(f.inbox, n).await);
    }
    ids.push(f.email_at(f.archive, 1).await);
    // ids: a(100) b(200) big(400) reply(500) | gamma(300, archive)
    (f, ids)
}

#[tokio::test(flavor = "multi_thread")]
async fn query_filters() {
    let (f, ids) = query_fixture().await;
    let [a, b, big, reply, gamma] = ids[..] else {
        panic!()
    };
    let run = |filter| {
        let f = &f;
        async move { f.query(all(Some(filter))).await.ids }
    };
    // Default sort: newest first.
    assert_eq!(f.query(all(None)).await.ids, vec![reply, big, gamma, b, a]);
    assert_eq!(run(EmailFilter::InMailbox(f.archive)).await, vec![gamma]);
    assert_eq!(
        run(EmailFilter::InMailboxOtherThan(vec![f.inbox])).await,
        vec![gamma]
    );
    assert_eq!(
        run(EmailFilter::HasKeyword(Keyword::flagged())).await,
        vec![b]
    );
    assert_eq!(
        run(EmailFilter::NotKeyword(Keyword::seen())).await,
        vec![reply, big, gamma, a]
    );
    assert_eq!(
        run(EmailFilter::Before(UnixTime::from_secs(300))).await,
        vec![b, a]
    );
    assert_eq!(
        run(EmailFilter::After(UnixTime::from_secs(300))).await,
        vec![reply, big, gamma]
    );
    assert_eq!(run(EmailFilter::MinSize(2000)).await, vec![big]);
    assert_eq!(
        run(EmailFilter::MaxSize(2000)).await,
        vec![reply, gamma, b, a]
    );
    assert_eq!(run(EmailFilter::HasAttachment(true)).await, vec![big]);
    assert_eq!(run(EmailFilter::Ids(vec![a, gamma])).await, vec![gamma, a]);
    assert_eq!(
        run(EmailFilter::And(vec![
            EmailFilter::InMailbox(f.inbox),
            EmailFilter::Not(vec![EmailFilter::HasAttachment(true)]),
        ]))
        .await,
        vec![reply, b, a]
    );
    assert_eq!(
        run(EmailFilter::Or(vec![
            EmailFilter::Ids(vec![a]),
            EmailFilter::Ids(vec![b])
        ]))
        .await,
        vec![b, a]
    );
    assert!(run(EmailFilter::Or(Vec::new())).await.is_empty());
    // a and reply are one thread; only b has $seen.
    assert_eq!(
        run(EmailFilter::SomeInThreadHaveKeyword(Keyword::seen())).await,
        vec![b]
    );
    assert_eq!(
        run(EmailFilter::NoneInThreadHaveKeyword(Keyword::seen())).await,
        vec![reply, big, gamma, a]
    );
    assert_eq!(
        run(EmailFilter::AllInThreadHaveKeyword(Keyword::flagged())).await,
        vec![b]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn query_sorts_collapses_and_pages() {
    let (f, ids) = query_fixture().await;
    let [a, b, big, reply, gamma] = ids[..] else {
        panic!()
    };
    let sorted = |key, direction| EmailQuery {
        sort: vec![EmailSort { key, direction }],
        ..all(None)
    };
    assert_eq!(
        f.query(sorted(SortKey::Subject, Direction::Ascending))
            .await
            .ids,
        // "Re: Alpha" sorts as "alpha", after a (same key, higher id first).
        vec![reply, a, b, gamma, big]
    );
    assert_eq!(
        f.query(sorted(SortKey::Size, Direction::Descending))
            .await
            .ids[0],
        big
    );
    assert_eq!(
        f.query(sorted(SortKey::ReceivedAt, Direction::Ascending))
            .await
            .ids,
        vec![a, b, gamma, big, reply]
    );
    assert_eq!(
        f.query(sorted(SortKey::From, Direction::Ascending))
            .await
            .ids,
        vec![a, b, gamma, reply, big]
    );

    // Collapsed: the newest of a/reply stands for their thread.
    let collapsed = f
        .query(EmailQuery {
            collapse_threads: true,
            ..all(None)
        })
        .await;
    assert_eq!(collapsed.ids, vec![reply, big, gamma, b]);
    assert_eq!(collapsed.total, 4);

    // Pages.
    let page = f
        .query(EmailQuery {
            start: QueryStart::Position(1),
            limit: Some(2),
            ..all(None)
        })
        .await;
    assert_eq!(
        (page.ids, page.position, page.total),
        (vec![big, gamma], 1, 5)
    );
    let tail = f
        .query(EmailQuery {
            start: QueryStart::Position(-2),
            ..all(None)
        })
        .await;
    assert_eq!((tail.ids, tail.position), (vec![b, a], 3));
    let anchored = f
        .query(EmailQuery {
            start: QueryStart::Anchor {
                id: gamma,
                offset: -1,
            },
            limit: Some(2),
            ..all(None)
        })
        .await;
    assert_eq!((anchored.ids, anchored.position), (vec![big, gamma], 1));
    let missing = f
        .store
        .query_emails(
            f.account,
            EmailQuery {
                start: QueryStart::Anchor {
                    id: EmailId::from_raw(9999).unwrap(),
                    offset: 0,
                },
                ..all(None)
            },
        )
        .await;
    assert!(matches!(
        missing,
        Err(crate::StoreError::NotFound("anchor"))
    ));
    let beyond = f
        .query(EmailQuery {
            start: QueryStart::Position(50),
            ..all(None)
        })
        .await;
    assert!(beyond.ids.is_empty());
    assert_eq!(beyond.total, 5);
}

#[tokio::test(flavor = "multi_thread")]
async fn too_deep_filters_are_refused() {
    let f = fixture().await;
    let mut filter = EmailFilter::HasAttachment(true);
    for _ in 0..20 {
        filter = EmailFilter::Not(vec![filter]);
    }
    assert!(
        f.store
            .query_emails(f.account, all(Some(filter)))
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn search_results_map_to_emails() {
    let (f, ids) = query_fixture().await;
    let found = f
        .store
        .emails_at(f.account, f.inbox, vec![uid(2), uid(4), uid(77)])
        .await
        .unwrap();
    let mut expected = vec![ids[1], ids[3]];
    expected.sort_unstable();
    assert_eq!(found, expected);
    let by_message_id = f
        .store
        .emails_by_message_id(f.account, f.inbox, "b@x".to_owned())
        .await
        .unwrap();
    assert_eq!(by_message_id, vec![ids[1]]);
}

#[tokio::test(flavor = "multi_thread")]
async fn previews_are_filled_once() {
    let f = fixture().await;
    f.add(f.inbox, vec![message(1, meta("a@x", "Hola", 100))])
        .await;
    let id = f.email_at(f.inbox, 1).await;
    let e0 = f.state(DataType::Email).await;
    f.store
        .set_preview(f.account, id, "Hola mundo".to_owned())
        .await
        .unwrap();
    f.store
        .set_preview(f.account, id, "Otra".to_owned())
        .await
        .unwrap();
    assert_eq!(
        f.email(id).await.meta.preview.as_deref(),
        Some("Hola mundo")
    );
    assert_eq!(f.state(DataType::Email).await, e0);
}

#[test]
fn recorded_merges_states() {
    let mut first = Recorded::default();
    first.push(DataType::Email, Seq::new(3));
    let mut later = Recorded::default();
    later.push(DataType::Email, Seq::new(4));
    later.push(DataType::Mailbox, Seq::new(1));
    first.merge(later);
    assert_eq!(first.state(DataType::Email), Some(Seq::new(4)));
    assert_eq!(first.state(DataType::Mailbox), Some(Seq::new(1)));
    assert_eq!(first.state(DataType::Thread), None);
    let _ = (ChangeKind::Created, ThreadId::from_raw(1));
}

/// The store's backstop on filter width sits far above jmap-proto's 256
/// nodes: the widest filter jmap-mail can make from them passes.
#[tokio::test(flavor = "multi_thread")]
async fn wide_filters_have_a_backstop() {
    let (f, ids) = query_fixture().await;
    let every_property = || {
        EmailFilter::And(vec![
            EmailFilter::InMailbox(f.inbox),
            EmailFilter::InMailboxOtherThan(vec![f.trash]),
            EmailFilter::Before(UnixTime::from_secs(10_000)),
            EmailFilter::After(UnixTime::from_secs(0)),
            EmailFilter::MinSize(0),
            EmailFilter::MaxSize(1_000_000),
            EmailFilter::HasKeyword(Keyword::flagged()),
            EmailFilter::NotKeyword(Keyword::seen()),
            EmailFilter::AllInThreadHaveKeyword(Keyword::flagged()),
            EmailFilter::SomeInThreadHaveKeyword(Keyword::flagged()),
            EmailFilter::NoneInThreadHaveKeyword(Keyword::seen()),
            EmailFilter::HasAttachment(false),
            EmailFilter::Ids(ids.clone()),
        ])
    };
    let widest = EmailFilter::Or((0..256).map(|_| every_property()).collect());
    f.store
        .query_emails(f.account, all(Some(widest)))
        .await
        .unwrap();

    let leaves = |n: usize| -> Vec<EmailFilter> {
        (0..n).map(|_| EmailFilter::HasAttachment(true)).collect()
    };
    let at_limit = EmailFilter::Or(leaves(MAX_FILTER_NODES - 1));
    f.store
        .query_emails(f.account, all(Some(at_limit)))
        .await
        .unwrap();
    let over = EmailFilter::Or(leaves(MAX_FILTER_NODES));
    assert!(matches!(
        f.store.query_emails(f.account, all(Some(over))).await,
        Err(crate::StoreError::Invalid("filter too large"))
    ));
    // Spread over subgroups, it counts the same.
    let spread = EmailFilter::And(
        (0..64)
            .map(|_| EmailFilter::Or(leaves(MAX_FILTER_NODES / 64)))
            .collect(),
    );
    assert!(matches!(
        f.store.query_emails(f.account, all(Some(spread))).await,
        Err(crate::StoreError::Invalid("filter too large"))
    ));
    let mailboxes = vec![f.trash; MAX_OTHER_THAN_MAILBOXES + 1];
    assert!(matches!(
        f.store
            .query_emails(
                f.account,
                all(Some(EmailFilter::InMailboxOtherThan(mailboxes)))
            )
            .await,
        Err(crate::StoreError::Invalid("filter mailbox list too long"))
    ));
}
