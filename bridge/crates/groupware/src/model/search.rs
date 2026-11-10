// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Text filters of `CalendarEvent/query` and `ContactCard/query`
//! (4.4): the searchable text of an iCalendar object or a vCard,
//! read from the opened copy (never stored in clear: the copy is sealed,
//! so the text is taken out of it for one query and dropped).
//!
//! Matching as Stalwart's index does where it can, but as a substring
//! (Stalwart matches words): case and accents do not count ([`Needle`]:
//! "jose" finds "José", "MULLER" finds "Müller"). The fields are those
//! Stalwart indexes:
//!
//! - **Events** (every `VEVENT`/`VTODO`/`VJOURNAL`, overrides too):
//!   `SUMMARY` (title), `DESCRIPTION`, `LOCATION`, `ORGANIZER` (owner) and
//!   `ATTENDEE`, the last two with their parameters (`CN`…) and without
//!   `mailto:`.
//! - **Cards:** `N` (and `FN`, which Stalwart leaves out) for `name`, its
//!   parts for `name/given`, `name/surname`, `name/surname2`; `NICKNAME`,
//!   `ORG`, `EMAIL`, `TEL` (also by digits only: "600 11 22" finds
//!   "+34600112233"), `IMPP`/`SOCIALPROFILE` (online services), `ADR`,
//!   `NOTE`; `KIND` (exact; none = `individual`), `MEMBER` (exact),
//!   `CREATED` and `REV` (JSContact `created` and `updated`).
//!
//! CPU work (calcard): run it off the async threads.

use calcard::icalendar::{ICalendar, ICalendarParameterValue, ICalendarProperty, ICalendarValue};
use calcard::vcard::{VCard, VCardKind, VCardProperty, VCardValue};

use super::convert::{Format, checked_text, guarded};
use crate::ConvertError;

/// Longest needle (characters): a longer one can't be in any field we
/// keep anyway.
const MAX_NEEDLE: usize = 1024;
/// Longest text kept per field value (characters).
const MAX_VALUE: usize = 16 * 1024;

/// A search term, folded once ([`fold`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Needle {
    folded: String,
    digits: String,
}

impl Needle {
    /// The term, folded (and cut at [`MAX_NEEDLE`] characters).
    #[must_use]
    pub fn new(term: &str) -> Self {
        let term: String = term.trim().chars().take(MAX_NEEDLE).collect();
        Self {
            digits: phone_digits(&term),
            folded: fold(&term),
        }
    }

    /// Whether a folded value contains it (an empty term matches all).
    fn in_value(&self, folded: &str) -> bool {
        folded.contains(&self.folded)
    }

    fn in_any(&self, values: &[String]) -> bool {
        values.iter().any(|value| self.in_value(value))
    }
}

/// The digits of a term that looks like a phone number (digits, and only
/// spaces and `+ - ( ) . /` between them), for matching phones by digits
/// alone; empty for anything else ("Calle 5" or "ana2" would otherwise
/// match every phone with a 5 or a 2).
fn phone_digits(term: &str) -> String {
    let phone_like = term.chars().any(|c| c.is_ascii_digit())
        && term
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, ' ' | '+' | '-' | '(' | ')' | '.' | '/'));
    if phone_like {
        term.chars().filter(char::is_ascii_digit).collect()
    } else {
        String::new()
    }
}

/// Lower case without accents: each letter of Latin-1 and Latin
/// Extended-A as its base letters (`é` → `e`, `ß` → `ss`, `æ` → `ae`),
/// combining marks (U+0300–U+036F) dropped, everything else lower-cased.
#[must_use]
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if ('\u{300}'..='\u{36f}').contains(&c) {
            continue;
        }
        match base_letters(c) {
            Some(base) => out.push_str(base),
            None => out.extend(c.to_lowercase()),
        }
    }
    out
}

/// The base letters of an accented Latin letter (lower case).
#[allow(clippy::too_many_lines, reason = "one table")]
const fn base_letters(c: char) -> Option<&'static str> {
    Some(match c {
        'À'..='Å' | 'à'..='å' | 'Ā'..='ą' => "a",
        'Æ' | 'æ' => "ae",
        'Ç' | 'ç' | 'Ć'..='č' => "c",
        'Ď'..='đ' | 'Ð' | 'ð' => "d",
        'È'..='Ë' | 'è'..='ë' | 'Ē'..='ě' => "e",
        'Ĝ'..='ģ' => "g",
        'Ĥ'..='ħ' => "h",
        'Ì'..='Ï' | 'ì'..='ï' | 'Ĩ'..='ı' => "i",
        'Ĳ' | 'ĳ' => "ij",
        'Ĵ' | 'ĵ' => "j",
        'Ķ'..='ĸ' => "k",
        'Ĺ'..='ł' => "l",
        'Ñ' | 'ñ' | 'Ń'..='ŋ' => "n",
        'Ò'..='Ö' | 'Ø' | 'ò'..='ö' | 'ø' | 'Ō'..='ő' => "o",
        'Œ' | 'œ' => "oe",
        'Ŕ'..='ř' => "r",
        'Ś'..='š' => "s",
        'ß' => "ss",
        'Ţ'..='ŧ' => "t",
        'Þ' | 'þ' => "th",
        'Ù'..='Ü' | 'ù'..='ü' | 'Ũ'..='ų' => "u",
        'Ŵ' | 'ŵ' => "w",
        'Ý' | 'ý' | 'ÿ' | 'Ŷ'..='Ÿ' => "y",
        'Ź'..='ž' => "z",
        _ => return None,
    })
}

fn folded(text: &str) -> String {
    let cut: String = text.chars().take(MAX_VALUE).collect();
    fold(&cut)
}

/// A text field of an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EventField {
    /// `title` (`SUMMARY`).
    Title,
    /// `description`.
    Description,
    /// A location.
    Location,
    /// The organizer (`owner`).
    Owner,
    /// An attendee.
    Attendee,
    /// The organizer or an attendee (`participant`).
    Participant,
    /// Any of them (`text`).
    Text,
}

/// The searchable text of an iCalendar object, folded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventText {
    title: Vec<String>,
    description: Vec<String>,
    location: Vec<String>,
    owner: Vec<String>,
    attendee: Vec<String>,
}

fn without_mailto(text: &str) -> &str {
    text.get(..7)
        .filter(|scheme| scheme.eq_ignore_ascii_case("mailto:"))
        .and_then(|_| text.get(7..))
        .unwrap_or(text)
}

impl EventText {
    /// Reads an iCalendar object.
    ///
    /// # Errors
    /// [`ConvertError`] if it is too large, not UTF-8 or does not parse.
    pub fn parse(input: &[u8]) -> Result<Self, ConvertError> {
        let format = Format::ICalendar;
        let text = checked_text(input, format)?;
        guarded(format, || {
            let ical = ICalendar::parse(text).map_err(|_| ConvertError::Invalid { format })?;
            let mut out = Self::default();
            for component in ical
                .components
                .iter()
                .filter(|c| c.component_type.is_calendar_object())
            {
                for entry in &component.entries {
                    let list = match entry.name {
                        ICalendarProperty::Summary => &mut out.title,
                        ICalendarProperty::Description => &mut out.description,
                        ICalendarProperty::Location => &mut out.location,
                        ICalendarProperty::Organizer => &mut out.owner,
                        ICalendarProperty::Attendee => &mut out.attendee,
                        _ => continue,
                    };
                    let values = entry.values.iter().filter_map(|value| match value {
                        ICalendarValue::Text(text) => Some(text.as_str()),
                        ICalendarValue::Uri(uri) => uri.as_str(),
                        _ => None,
                    });
                    let params = entry.params.iter().filter_map(|param| match &param.value {
                        ICalendarParameterValue::Text(text) => Some(text.as_str()),
                        ICalendarParameterValue::Uri(uri) => uri.as_str(),
                        _ => None,
                    });
                    let people = matches!(
                        entry.name,
                        ICalendarProperty::Organizer | ICalendarProperty::Attendee
                    );
                    for value in values.chain(params.filter(|_| people)) {
                        list.push(folded(without_mailto(value)));
                    }
                }
            }
            Ok(out)
        })
    }

    /// Whether `field` contains `needle`.
    #[must_use]
    pub fn matches(&self, field: EventField, needle: &Needle) -> bool {
        match field {
            EventField::Title => needle.in_any(&self.title),
            EventField::Description => needle.in_any(&self.description),
            EventField::Location => needle.in_any(&self.location),
            EventField::Owner => needle.in_any(&self.owner),
            EventField::Attendee => needle.in_any(&self.attendee),
            EventField::Participant => needle.in_any(&self.owner) || needle.in_any(&self.attendee),
            EventField::Text => [
                &self.title,
                &self.description,
                &self.location,
                &self.owner,
                &self.attendee,
            ]
            .iter()
            .any(|values| needle.in_any(values)),
        }
    }
}

/// A text field of a card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CardField {
    /// Any part of the name (`name`).
    Name,
    /// `name/given`.
    Given,
    /// `name/surname`.
    Surname,
    /// `name/surname2`.
    Surname2,
    /// `nickname`.
    Nickname,
    /// `organization`.
    Organization,
    /// `email`.
    Email,
    /// `phone`.
    Phone,
    /// `onlineService`.
    OnlineService,
    /// `address`.
    Address,
    /// `note`.
    Note,
    /// Any of them (`text`).
    Text,
}

/// The searchable text of a vCard, folded, and what the other filters
/// compare.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CardText {
    name: Vec<String>,
    given: Vec<String>,
    surname: Vec<String>,
    surname2: Vec<String>,
    nickname: Vec<String>,
    organization: Vec<String>,
    email: Vec<String>,
    phone: Vec<String>,
    online: Vec<String>,
    address: Vec<String>,
    note: Vec<String>,
    kind: Option<VCardKind>,
    members: Vec<String>,
    /// `CREATED`, seconds since the epoch.
    pub created: Option<i64>,
    /// `REV`, seconds since the epoch.
    pub updated: Option<i64>,
}

fn card_values(values: &[VCardValue]) -> impl Iterator<Item = &str> {
    values.iter().flat_map(|value| -> Vec<&str> {
        match value {
            VCardValue::Text(text) => vec![text.as_str()],
            VCardValue::Component(parts) => parts.iter().map(String::as_str).collect(),
            _ => Vec::new(),
        }
    })
}

fn kind_name(kind: VCardKind) -> &'static str {
    match kind {
        VCardKind::Individual => "individual",
        VCardKind::Group => "group",
        VCardKind::Org => "org",
        VCardKind::Location => "location",
        VCardKind::Application => "application",
        VCardKind::Device => "device",
    }
}

impl CardText {
    /// Reads a vCard (3.0 or 4.0).
    ///
    /// # Errors
    /// [`ConvertError`] if it is too large, not UTF-8 or does not parse.
    pub fn parse(input: &[u8]) -> Result<Self, ConvertError> {
        let format = Format::VCard;
        let text = checked_text(input, format)?;
        guarded(format, || {
            let mut card = VCard::parse(text).map_err(|_| ConvertError::Invalid { format })?;
            // Apple's groups filter like `KIND` and `MEMBER` (as `get`).
            super::apple_groups::read(&mut card);
            let mut out = Self::default();
            for entry in &card.entries {
                let push = |list: &mut Vec<String>| {
                    for value in card_values(&entry.values).filter(|v| !v.trim().is_empty()) {
                        list.push(folded(value));
                    }
                };
                match &entry.name {
                    VCardProperty::N => {
                        push(&mut out.name);
                        let part = |index: usize| match entry.values.get(index) {
                            Some(VCardValue::Component(parts)) => {
                                parts.iter().map(|p| folded(p)).collect()
                            }
                            Some(VCardValue::Text(text)) => text.split(',').map(folded).collect(),
                            _ => Vec::new(),
                        };
                        out.surname.extend(part(0));
                        out.given.extend(part(1));
                        out.surname2.extend(part(5));
                    }
                    VCardProperty::Fn => push(&mut out.name),
                    VCardProperty::Nickname => push(&mut out.nickname),
                    VCardProperty::Org => push(&mut out.organization),
                    VCardProperty::Email => push(&mut out.email),
                    VCardProperty::Tel => push(&mut out.phone),
                    VCardProperty::Impp | VCardProperty::Socialprofile => push(&mut out.online),
                    VCardProperty::Adr => push(&mut out.address),
                    VCardProperty::Note => push(&mut out.note),
                    VCardProperty::Kind => {
                        out.kind = entry.values.iter().find_map(|value| match value {
                            VCardValue::Kind(kind) => Some(*kind),
                            _ => None,
                        });
                    }
                    VCardProperty::Member => {
                        out.members
                            .extend(card_values(&entry.values).map(|v| v.trim().to_owned()));
                    }
                    VCardProperty::Created | VCardProperty::Rev => {
                        let at = entry.values.iter().find_map(|value| match value {
                            VCardValue::PartialDateTime(dt) => dt.to_timestamp(),
                            _ => None,
                        });
                        if entry.name == VCardProperty::Created {
                            out.created = at;
                        } else {
                            out.updated = at;
                        }
                    }
                    _ => {}
                }
            }
            for list in [&mut out.surname, &mut out.given, &mut out.surname2] {
                list.retain(|value| !value.is_empty());
            }
            Ok(out)
        })
    }

    /// Whether `field` contains `needle`.
    #[must_use]
    pub fn matches(&self, field: CardField, needle: &Needle) -> bool {
        match field {
            CardField::Name => needle.in_any(&self.name),
            CardField::Given => needle.in_any(&self.given),
            CardField::Surname => needle.in_any(&self.surname),
            CardField::Surname2 => needle.in_any(&self.surname2),
            CardField::Nickname => needle.in_any(&self.nickname),
            CardField::Organization => needle.in_any(&self.organization),
            CardField::Email => needle.in_any(&self.email),
            CardField::Phone => {
                needle.in_any(&self.phone)
                    || (!needle.digits.is_empty()
                        && self.phone.iter().any(|phone| {
                            phone
                                .chars()
                                .filter(char::is_ascii_digit)
                                .collect::<String>()
                                .contains(&needle.digits)
                        }))
            }
            CardField::OnlineService => needle.in_any(&self.online),
            CardField::Address => needle.in_any(&self.address),
            CardField::Note => needle.in_any(&self.note),
            CardField::Text => [
                CardField::Name,
                CardField::Nickname,
                CardField::Organization,
                CardField::Email,
                CardField::Phone,
                CardField::OnlineService,
                CardField::Address,
                CardField::Note,
            ]
            .into_iter()
            .any(|field| self.matches(field, needle)),
        }
    }

    /// Whether its kind is `kind` (case-insensitive; no `KIND` is
    /// `individual`, RFC 6350 §6.1.4).
    #[must_use]
    pub fn is_kind(&self, kind: &str) -> bool {
        kind_name(self.kind.unwrap_or(VCardKind::Individual)).eq_ignore_ascii_case(kind.trim())
    }

    /// Whether it is a group with this member (the `MEMBER` value, as
    /// JSContact's `members` keys).
    #[must_use]
    pub fn has_member(&self, member: &str) -> bool {
        self.members.iter().any(|m| m == member.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding() {
        assert_eq!(
            fold("José MÜLLER Ñandú Œuvre Straße Ærø"),
            "jose muller nandu oeuvre strasse aero"
        );
        assert_eq!(fold("e\u{301}cole"), "ecole");
        assert_eq!(fold("Łódź Dvořák"), "lodz dvorak");
        assert_eq!(fold("ΑΘΗΝΑ Кирилл"), "αθηνα кирилл");
    }

    /// Only a term that looks like a phone number is matched by its
    /// digits.
    #[test]
    fn phone_digits_only_for_phone_numbers() {
        assert_eq!(Needle::new("Calle 5").digits, "");
        assert_eq!(Needle::new("ana2").digits, "");
        assert_eq!(Needle::new("+34 (600) 12-3").digits, "34600123");
        assert_eq!(Needle::new("600.555/1").digits, "6005551");
        assert_eq!(Needle::new("  ").digits, "");
        let card = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:p\r\nFN:Pat\r\n\
            TEL:+34 600 555 123\r\nEND:VCARD\r\n";
        let text = CardText::parse(card.as_bytes()).unwrap();
        let has = |field, term| text.matches(field, &Needle::new(term));
        assert!(!has(CardField::Text, "Calle 5"));
        assert!(!has(CardField::Phone, "ana2"));
        assert!(has(CardField::Text, "600 555"));
        assert!(has(CardField::Phone, "600555"));
        assert!(has(CardField::Phone, "(600) 555-123"));
    }

    const EVENT: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:u1\r\n\
        DTSTART:20261102T100000Z\r\nSUMMARY:Reunión de presupuesto\r\nDESCRIPTION:Traer el informe\r\n\
        LOCATION:Sala Ébano\r\nORGANIZER;CN=Ana Pérez:mailto:ana@example.com\r\n\
        ATTENDEE;CN=Bob Smith:mailto:bob@example.com\r\nEND:VEVENT\r\n\
        BEGIN:VEVENT\r\nUID:u1\r\nRECURRENCE-ID:20261109T100000Z\r\nDTSTART:20261109T110000Z\r\n\
        SUMMARY:Movida al jueves\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn events() {
        let text = EventText::parse(EVENT.as_bytes()).unwrap();
        let has = |field, term| text.matches(field, &Needle::new(term));
        assert!(has(EventField::Title, "REUNION"));
        assert!(has(EventField::Title, "jueves"));
        assert!(!has(EventField::Title, "informe"));
        assert!(has(EventField::Description, "informe"));
        assert!(has(EventField::Location, "ebano"));
        assert!(has(EventField::Owner, "perez") && has(EventField::Owner, "ana@example"));
        assert!(!has(EventField::Owner, "mailto"));
        assert!(has(EventField::Attendee, "bob") && !has(EventField::Attendee, "ana"));
        assert!(has(EventField::Participant, "ana") && has(EventField::Participant, "smith"));
        assert!(has(EventField::Text, "sala") && !has(EventField::Text, "nada"));
        assert!(EventText::parse(b"garbage").is_err());
    }

    const CARD: &str = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:c1\r\nFN:José Müller\r\n\
        N:Müller;José;Luis;Dr.;;García;\r\nNICKNAME:Pepe\r\nORG:Ácme S.L.;Ventas\r\n\
        EMAIL:jose@example.com\r\nTEL:+34 600 11 22 33\r\nIMPP:xmpp:jose@chat.example\r\n\
        ADR:;;Calle Mayor 1;Málaga;;29001;España\r\nNOTE:Prefiere café\r\n\
        CREATED:20260101T100000Z\r\nREV:20261020T080000Z\r\nEND:VCARD\r\n";

    #[test]
    fn cards() {
        let text = CardText::parse(CARD.as_bytes()).unwrap();
        let has = |field, term| text.matches(field, &Needle::new(term));
        assert!(has(CardField::Name, "jose muller") && has(CardField::Name, "luis"));
        assert!(has(CardField::Given, "Jose") && !has(CardField::Given, "muller"));
        assert!(has(CardField::Surname, "MULLER") && !has(CardField::Surname, "jose"));
        assert!(has(CardField::Surname2, "garcia"));
        assert!(has(CardField::Nickname, "pepe"));
        assert!(has(CardField::Organization, "acme") && has(CardField::Organization, "ventas"));
        assert!(has(CardField::Email, "JOSE@EXAMPLE"));
        assert!(has(CardField::Phone, "600112233") && has(CardField::Phone, "600 11"));
        assert!(!has(CardField::Phone, "999"));
        assert!(has(CardField::OnlineService, "chat.example"));
        assert!(has(CardField::Address, "malaga") && has(CardField::Address, "espana"));
        assert!(has(CardField::Note, "cafe"));
        assert!(has(CardField::Text, "ventas") && !has(CardField::Text, "zzz"));
        assert!(text.is_kind("individual") && !text.is_kind("group"));
        assert_eq!(text.created, Some(1_767_261_600));
        assert_eq!(text.updated, Some(1_792_483_200));

        let group = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:g\r\nFN:Equipo\r\nKIND:group\r\n\
            MEMBER:urn:uuid:c1\r\nEND:VCARD\r\n";
        let group = CardText::parse(group.as_bytes()).unwrap();
        assert!(group.is_kind("GROUP") && group.has_member("urn:uuid:c1"));
        assert!(!group.has_member("urn:uuid:c2") && group.created.is_none());
        // Nextcloud's 3.0 form of the same group.
        let apple = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:g\r\nFN:Equipo\r\n\
            X-ADDRESSBOOKSERVER-KIND:GROUP\r\nX-ADDRESSBOOKSERVER-MEMBER:urn:uuid:c1\r\nEND:VCARD\r\n";
        let apple = CardText::parse(apple.as_bytes()).unwrap();
        assert!(apple.is_kind("group") && apple.has_member("urn:uuid:c1"));
    }
}
