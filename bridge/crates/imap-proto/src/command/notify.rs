// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `NOTIFY` (RFC 5465): ask the server to report events of other mailboxes
//! too (as `STATUS` and `LIST` responses), so one IDLE connection watches
//! the whole account.

use super::{Writer, command};
use crate::EncodeError;

/// Which mailboxes a group of events is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NotifyFilter {
    /// `SELECTED`: the selected mailbox (events come as EXISTS, EXPUNGE,
    /// FETCH, like without NOTIFY).
    Selected,
    /// `PERSONAL`: every mailbox of the user's personal namespace.
    Personal,
    /// `INBOXES`: mailboxes where new mail may be delivered.
    Inboxes,
    /// `SUBSCRIBED`: the subscribed mailboxes.
    Subscribed,
}

/// An event to be told about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NotifyEvent {
    /// `MessageNew`: a message arrived.
    MessageNew,
    /// `MessageExpunge`: a message was expunged.
    MessageExpunge,
    /// `FlagChange`: flags changed (needs `MessageNew` and `MessageExpunge`
    /// in the same group).
    FlagChange,
    /// `MailboxName`: a mailbox was created, deleted or renamed (sent as
    /// `LIST`).
    MailboxName,
    /// `SubscriptionChange`: a mailbox was (un)subscribed (sent as `LIST`).
    SubscriptionChange,
}

impl NotifyEvent {
    fn as_wire(self) -> &'static [u8] {
        match self {
            Self::MessageNew => b"MessageNew",
            Self::MessageExpunge => b"MessageExpunge",
            Self::FlagChange => b"FlagChange",
            Self::MailboxName => b"MailboxName",
            Self::SubscriptionChange => b"SubscriptionChange",
        }
    }
}

/// Some events for some mailboxes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotifyGroup<'a> {
    /// Which mailboxes.
    pub filter: NotifyFilter,
    /// At least one event.
    pub events: &'a [NotifyEvent],
}

/// `NOTIFY SET (filter (events)) ...`: replaces earlier settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotifySet<'a> {
    /// At least one group.
    pub groups: &'a [NotifyGroup<'a>],
}

/// `NOTIFY NONE`: stop all notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotifyNone;

command!(NotifySet<'_>, "NOTIFY", |notify, writer| {
    if notify.groups.is_empty() {
        return Err(EncodeError::Empty("NOTIFY groups"));
    }
    writer.raw(b"NOTIFY SET");
    for group in notify.groups {
        write_group(writer, group)?;
    }
    Ok(())
});

command!(NotifyNone, "NOTIFY", |_notify, writer| {
    writer.raw(b"NOTIFY NONE");
    Ok(())
});

fn write_group(writer: &mut Writer, group: &NotifyGroup<'_>) -> Result<(), EncodeError> {
    if group.events.is_empty() {
        return Err(EncodeError::Empty("NOTIFY events"));
    }
    let has = |event| group.events.contains(&event);
    // RFC 5465 §5: FlagChange only together with MessageNew and
    // MessageExpunge (the server would answer BAD).
    if has(NotifyEvent::FlagChange)
        && !(has(NotifyEvent::MessageNew) && has(NotifyEvent::MessageExpunge))
    {
        return Err(EncodeError::Invalid(
            "FlagChange needs MessageNew and MessageExpunge",
        ));
    }
    writer.raw(match group.filter {
        NotifyFilter::Selected => b" (SELECTED (",
        NotifyFilter::Personal => b" (PERSONAL (",
        NotifyFilter::Inboxes => b" (INBOXES (",
        NotifyFilter::Subscribed => b" (SUBSCRIBED (",
    });
    for (index, event) in group.events.iter().enumerate() {
        if index > 0 {
            writer.space();
        }
        writer.raw(event.as_wire());
    }
    writer.raw(b"))");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::test_util::basic;
    use crate::command::{EncodeOptions, encode};
    use crate::types::Tag;

    #[test]
    fn encodes() {
        use NotifyEvent::{FlagChange, MailboxName, MessageExpunge, MessageNew};
        let groups = [
            NotifyGroup {
                filter: NotifyFilter::Selected,
                events: &[MessageNew, MessageExpunge, FlagChange],
            },
            NotifyGroup {
                filter: NotifyFilter::Personal,
                events: &[MessageNew, MessageExpunge, MailboxName],
            },
        ];
        assert_eq!(
            basic(&NotifySet { groups: &groups }),
            "A1 NOTIFY SET (SELECTED (MessageNew MessageExpunge FlagChange)) \
             (PERSONAL (MessageNew MessageExpunge MailboxName))\r\n"
        );
        assert_eq!(basic(&NotifyNone), "A1 NOTIFY NONE\r\n");
    }

    #[test]
    fn refuses_bad_groups() {
        let encode = |notify: &NotifySet<'_>| {
            encode(notify, Tag::new("A1").unwrap(), EncodeOptions::basic()).err()
        };
        assert!(encode(&NotifySet { groups: &[] }).is_some());
        let empty = [NotifyGroup {
            filter: NotifyFilter::Personal,
            events: &[],
        }];
        assert!(encode(&NotifySet { groups: &empty }).is_some());
        let flags_only = [NotifyGroup {
            filter: NotifyFilter::Personal,
            events: &[NotifyEvent::FlagChange],
        }];
        assert!(
            encode(&NotifySet {
                groups: &flags_only
            })
            .is_some()
        );
    }
}
