// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Capability URIs (RFC 8620 §2, IANA "JMAP Capabilities").

use std::fmt;
use std::str::FromStr;

use crate::macros::serde_via_str;

/// A capability URI, as used in `using`, in the session and in
/// `primaryAccounts`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Capability {
    /// `urn:ietf:params:jmap:core` (RFC 8620).
    Core,
    /// `urn:ietf:params:jmap:mail` (RFC 8621).
    Mail,
    /// `urn:ietf:params:jmap:submission` (RFC 8621).
    Submission,
    /// `urn:ietf:params:jmap:vacationresponse` (RFC 8621).
    VacationResponse,
    /// `urn:ietf:params:jmap:quota` (RFC 9425).
    Quota,
    /// `urn:ietf:params:jmap:blob` (RFC 9404).
    Blob,
    /// `urn:ietf:params:jmap:websocket` (RFC 8887).
    WebSocket,
    /// `urn:ietf:params:jmap:mdn` (RFC 9007).
    Mdn,
    /// `urn:ietf:params:jmap:sieve` (RFC 9661).
    Sieve,
    /// `urn:ietf:params:jmap:contacts` (RFC 9610).
    Contacts,
    /// `urn:ietf:params:jmap:contacts:parse`: `ContactCard/parse`.
    ContactsParse,
    /// `urn:ietf:params:jmap:calendars` (draft-ietf-jmap-calendars).
    Calendars,
    /// `urn:ietf:params:jmap:calendars:parse`: `CalendarEvent/parse`.
    CalendarsParse,
    /// `urn:ietf:params:jmap:principals` (RFC 9670).
    Principals,
    /// `urn:ietf:params:jmap:principals:availability`:
    /// `Principal/getAvailability` (draft-ietf-jmap-calendars).
    PrincipalsAvailability,
    /// `urn:ietf:params:jmap:mail:share`.
    MailShare,
    /// `urn:ietf:params:jmap:webpush-vapid` (RFC 9749): the server signs
    /// Web Push with VAPID; the session value has its
    /// `applicationServerKey`.
    WebPushVapid,
    /// `urn:ietf:params:jmap:emailpush` (Stalwart 1.0): a push
    /// subscription's `emailPush` puts the delivered emails in the push
    /// (an `EmailPush` object instead of the delivery's `StateChange`).
    /// `{}` in the session and on the user's own account.
    EmailPush,
    /// `https://helixnebula.tech/jmap/bridge`: the server is the Nebula
    /// Mail bridge (a vendor capability, RFC 8620 §2, `{}`). Lets the web
    /// app offer what the bridge does right and Stalwart doesn't yet
    /// (`canChangeOneOccurrence`), and has the bridge's own
    /// methods `CalendarEvent/counter` and `CalendarEvent/declineCounter`
    /// and properties, like a mailbox's IMAP name (§10.8).
    Bridge,
    /// Any other URI. Kept so the server can name it in an
    /// `unknownCapability` error.
    Other(OtherCapability),
}

/// The URI of [`Capability::Bridge`] (a domain of the project's).
const BRIDGE_URI: &str = "https://helixnebula.tech/jmap/bridge";

/// A capability URI this crate doesn't know (only made by parsing).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OtherCapability(String);

impl OtherCapability {
    /// The URI.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An empty string is not a capability.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("capability URI is empty")]
pub struct CapabilityError;

impl Capability {
    /// The URI.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Core => "urn:ietf:params:jmap:core",
            Self::Mail => "urn:ietf:params:jmap:mail",
            Self::Submission => "urn:ietf:params:jmap:submission",
            Self::VacationResponse => "urn:ietf:params:jmap:vacationresponse",
            Self::Quota => "urn:ietf:params:jmap:quota",
            Self::Blob => "urn:ietf:params:jmap:blob",
            Self::WebSocket => "urn:ietf:params:jmap:websocket",
            Self::Mdn => "urn:ietf:params:jmap:mdn",
            Self::Sieve => "urn:ietf:params:jmap:sieve",
            Self::Contacts => "urn:ietf:params:jmap:contacts",
            Self::ContactsParse => "urn:ietf:params:jmap:contacts:parse",
            Self::Calendars => "urn:ietf:params:jmap:calendars",
            Self::CalendarsParse => "urn:ietf:params:jmap:calendars:parse",
            Self::Principals => "urn:ietf:params:jmap:principals",
            Self::PrincipalsAvailability => "urn:ietf:params:jmap:principals:availability",
            Self::MailShare => "urn:ietf:params:jmap:mail:share",
            Self::WebPushVapid => "urn:ietf:params:jmap:webpush-vapid",
            Self::EmailPush => "urn:ietf:params:jmap:emailpush",
            Self::Bridge => BRIDGE_URI,
            Self::Other(uri) => uri.as_str(),
        }
    }
}

impl FromStr for Capability {
    type Err = CapabilityError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "urn:ietf:params:jmap:core" => Self::Core,
            "urn:ietf:params:jmap:mail" => Self::Mail,
            "urn:ietf:params:jmap:submission" => Self::Submission,
            "urn:ietf:params:jmap:vacationresponse" => Self::VacationResponse,
            "urn:ietf:params:jmap:quota" => Self::Quota,
            "urn:ietf:params:jmap:blob" => Self::Blob,
            "urn:ietf:params:jmap:websocket" => Self::WebSocket,
            "urn:ietf:params:jmap:mdn" => Self::Mdn,
            "urn:ietf:params:jmap:sieve" => Self::Sieve,
            "urn:ietf:params:jmap:contacts" => Self::Contacts,
            "urn:ietf:params:jmap:contacts:parse" => Self::ContactsParse,
            "urn:ietf:params:jmap:calendars" => Self::Calendars,
            "urn:ietf:params:jmap:calendars:parse" => Self::CalendarsParse,
            "urn:ietf:params:jmap:principals" => Self::Principals,
            "urn:ietf:params:jmap:principals:availability" => Self::PrincipalsAvailability,
            "urn:ietf:params:jmap:mail:share" => Self::MailShare,
            "urn:ietf:params:jmap:webpush-vapid" => Self::WebPushVapid,
            "urn:ietf:params:jmap:emailpush" => Self::EmailPush,
            BRIDGE_URI => Self::Bridge,
            "" => return Err(CapabilityError),
            other => Self::Other(OtherCapability(other.to_owned())),
        })
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

serde_via_str!(Capability);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for uri in [
            "urn:ietf:params:jmap:core",
            "urn:ietf:params:jmap:mail",
            "urn:ietf:params:jmap:submission",
            "urn:ietf:params:jmap:mail:share",
            "urn:ietf:params:jmap:calendars",
            "urn:ietf:params:jmap:calendars:parse",
            "urn:ietf:params:jmap:contacts",
            "urn:ietf:params:jmap:contacts:parse",
            "urn:ietf:params:jmap:principals",
            "urn:ietf:params:jmap:principals:availability",
            "https://helixnebula.tech/jmap/bridge",
            "urn:ietf:params:jmap:webpush-vapid",
            "urn:ietf:params:jmap:emailpush",
            "https://example.com/apis/foobar",
        ] {
            assert_eq!(uri.parse::<Capability>().unwrap().as_str(), uri);
        }
        assert!(matches!(
            "https://example.com/apis/foobar".parse::<Capability>(),
            Ok(Capability::Other(_))
        ));
        assert_eq!(
            "urn:ietf:params:jmap:calendars:parse".parse::<Capability>(),
            Ok(Capability::CalendarsParse)
        );
        assert_eq!(
            "urn:ietf:params:jmap:contacts:parse".parse::<Capability>(),
            Ok(Capability::ContactsParse)
        );
        assert!("".parse::<Capability>().is_err());
    }
}
