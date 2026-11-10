// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Row ids. SQLite ids are positive `i64`; each table gets its own type so
//! an account id can't be passed where a grant id is expected.

macro_rules! row_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(i64);

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }

        impl $name {
            /// The id from its number, if it is positive (the only ids SQLite
            /// gives out). Finding the row is up to the query.
            #[must_use]
            pub fn from_raw(raw: i64) -> Option<Self> {
                (raw > 0).then_some(Self(raw))
            }

            /// The number stored in the database.
            #[must_use]
            pub fn get(self) -> i64 {
                self.0
            }
        }
    };
}

row_id!(
    /// An account (`accounts.id`).
    AccountId
);
row_id!(
    /// A Web Push subscription (`push_subscriptions.id`). Never reused.
    PushSubscriptionId
);
row_id!(
    /// An OAuth grant, one per login (`oauth_grants.id`).
    GrantId
);
row_id!(
    /// A mailbox (`mailboxes.id`): one IMAP folder. Stable across renames
    /// done through the bridge.
    MailboxId
);
row_id!(
    /// An email (`emails.id`): one IMAP message, or several copies with the
    /// same server object id. Never reused.
    EmailId
);
row_id!(
    /// A thread (`threads.id`). Never reused.
    ThreadId
);
row_id!(
    /// An uploaded blob (`uploads.id`). Never reused.
    UploadId
);
row_id!(
    /// A sending identity (`identities.id`). Never reused.
    IdentityId
);
row_id!(
    /// A sent message (`email_submissions.id`). Never reused.
    SubmissionId
);
row_id!(
    /// A calendar or address book (`dav_collections.id`). Never reused.
    CollectionId
);
row_id!(
    /// An iCalendar object or vCard (`dav_resources.id`). Never reused.
    ResourceId
);
row_id!(
    /// A Sieve script (`sieve_scripts.id`). Never reused.
    SieveScriptId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_positive_ids() {
        assert_eq!(AccountId::from_raw(7).map(AccountId::get), Some(7));
        assert_eq!(AccountId::from_raw(0), None);
        assert_eq!(AccountId::from_raw(-1), None);
    }
}
