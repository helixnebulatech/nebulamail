// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Unit-test helpers: a temporary store with one account and its INBOX,
//! and a [`Mail`] service pointed at a local port (no IMAP server needed).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code: a panic is a failed test"
)]

use std::collections::BTreeSet;
use std::sync::Arc;

use nebula_imap::Credentials;
use nebula_imap::proto::types::{MailboxName, Uid, UidValidity};
use nebula_store::{
    AccountId, EmailId, EmailMeta, ListedMailbox, MailboxId, NewAccount, NewMessage, Role,
    ServerIds, Store, UnixTime,
};
use net_guard::{Port, PortPolicy, SystemResolver};

use crate::core::account::MailAccount;
use crate::{ImapServer, Mail, MailConfig, TlsMode};

pub(crate) const NOW: UnixTime = UnixTime::from_secs(2_000_000_000);

pub(crate) struct Fixture {
    pub(crate) mail: Mail,
    pub(crate) account: AccountId,
    pub(crate) inbox: MailboxId,
    _dir: tempfile::TempDir,
}

/// A port nothing listens on (bound, then let go).
pub(crate) fn closed_port() -> u16 {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.local_addr().unwrap().port()
}

/// The service pointed at the loopback address and `port`, with a store holding one
/// account and its INBOX.
pub(crate) async fn fixture(port: u16) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let account = store
        .get_or_create_account(
            NewAccount {
                login: "alice",
                email: "[EMAIL]",
                wrapped_dek: b"dek",
            },
            NOW,
        )
        .await
        .unwrap()
        .id;
    let listed = ListedMailbox {
        imap_name: MailboxName::new("INBOX").unwrap(),
        name: "Inbox".to_owned(),
        delimiter: Some('/'),
        parent: None,
        role: Some(Role::Inbox),
        subscribed: true,
        selectable: true,
        no_inferiors: false,
        sort_order: 0,
    };
    store
        .sync_mailboxes(account, vec![listed], NOW)
        .await
        .unwrap();
    let inbox = store.mailboxes(account).await.unwrap()[0].id;
    let port = Port::try_from(port).unwrap();
    let server = ImapServer::new(
        std::net::Ipv4Addr::LOCALHOST.to_string().parse().unwrap(),
        port,
        TlsMode::Implicit,
    )
    .with_private_network(true)
    .with_ports(PortPolicy::only(vec![port]));
    let resolver = Arc::new(SystemResolver::new().unwrap());
    let mail = Mail::new(store, MailConfig::new(server), resolver).unwrap();
    Fixture {
        mail,
        account,
        inbox,
        _dir: dir,
    }
}

impl Fixture {
    pub(crate) fn store(&self) -> &Store {
        &self.mail.shared.store
    }

    /// Stores a message at `uid` in the INBOX (UIDVALIDITY 1) and returns
    /// its email id.
    pub(crate) async fn add_message(&self, uid: u32) -> EmailId {
        let store = self.store();
        let validity = UidValidity::new(1).unwrap();
        if store
            .mailbox(self.account, self.inbox)
            .await
            .unwrap()
            .sync
            .uid_validity
            != Some(validity)
        {
            store
                .reset_mailbox(self.account, self.inbox, validity, NOW)
                .await
                .unwrap();
        }
        let uid = Uid::new(uid).unwrap();
        let meta = EmailMeta {
            message_id: Some(format!("{uid}@x")),
            in_reply_to: Vec::new(),
            references: Vec::new(),
            subject: Some("Hola".to_owned()),
            from: None,
            to: None,
            cc: None,
            bcc: None,
            reply_to: None,
            sender: None,
            sent_at: None,
            received_at: NOW,
            size: 100,
            has_attachment: false,
            preview: None,
            headers: b"Subject: Hola\r\n\r\n".to_vec(),
        };
        let message = NewMessage {
            uid,
            mod_seq: None,
            keywords: BTreeSet::new(),
            server: ServerIds::default(),
            meta,
        };
        store
            .add_messages(self.account, self.inbox, vec![message], NOW)
            .await
            .unwrap();
        store
            .email_at(self.account, self.inbox, uid)
            .await
            .unwrap()
            .unwrap()
    }

    /// An account object that is not registered in the service.
    pub(crate) fn account(&self) -> Arc<MailAccount> {
        Arc::new(MailAccount::new(
            self.account,
            Arc::clone(&self.mail.shared),
            Credentials::new("alice", "secret".into()),
        ))
    }
}
