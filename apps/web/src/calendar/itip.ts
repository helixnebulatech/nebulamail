// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Invitations by email (iTIP, RFC 5546, over iMIP, RFC 6047). The server
// parses the .ics (`CalendarEvent/parse`); this file decides what the card
// offers and writes our REPLY. Everything in an invitation comes from the
// sender: nothing is added to the calendar until the user acts
// (calendar spam).

import type { Participant, ParsedEvent } from "@nebulamail/jmap-client";

import type { AuthVerdict } from "../mail/auth-results";
import type { ICalTime } from "./ical-series";
import {
  dateStamp,
  isUtcZone,
  instantOf,
  localStamp,
  timeLine,
  tzid,
  utcStamp,
  vtimezoneLines,
} from "./ical-series";
import type { ParticipationStatus, Person } from "./model";
import { toParticipationStatus } from "./model";
import { parseLocalDate, parseLocalDateTime } from "./time";

export type ItipMethod =
  | "request"
  | "cancel"
  | "reply"
  | "publish"
  | "counter"
  | "declinecounter"
  | "other";

const METHODS: readonly ItipMethod[] = [
  "request",
  "cancel",
  "reply",
  "publish",
  "counter",
  "declinecounter",
];
/** Longest note (COMMENT) sent or shown, in characters. */
export const MAX_COMMENT = 2000;

export const toMethod = (value: string | null | undefined): ItipMethod =>
  METHODS.find((method) => method === value?.toLowerCase()) ?? "other";

/** Lower-case address of a `mailto:` URI (or a bare address); null if neither. */
export const addressOf = (uri: string | null | undefined): string | null => {
  if (uri === null || uri === undefined) {
    return null;
  }
  const value = uri.trim().replace(/^mailto:/iu, "");
  let decoded = value;
  try {
    decoded = decodeURIComponent(value);
  } catch {
    // Keep it as written.
  }
  const address = decoded.toLowerCase();
  return /^[^\s@<>()"]{1,64}@[^\s@<>()"]{1,253}$/u.test(address)
    ? address
    : null;
};

const participantAddress = (participant: Participant): string | null =>
  addressOf(participant.calendarAddress) ?? addressOf(participant.email);

const isOwn = (address: string | null, own: ReadonlySet<string>): boolean =>
  address !== null && own.has(address);

/** Lower-cased set of the user's addresses. */
export const ownSet = (addresses: readonly string[]): ReadonlySet<string> =>
  new Set(addresses.map((address) => address.trim().toLowerCase()));

export interface InvitationAttendee extends Person {
  readonly key: string;
  readonly status: ParticipationStatus;
  readonly isMe: boolean;
  readonly isOrganizer: boolean;
}

/** What the card shows and acts on. */
export interface Invitation {
  readonly method: ItipMethod;
  readonly uid: string;
  readonly sequence: number;
  /** The single occurrence it is about (RECURRENCE-ID), if any. */
  readonly recurrenceId: string | null;
  /** The zone `recurrenceId` is written in. */
  readonly recurrenceIdTimeZone: string | null;
  /** The sender's note (COMMENT), untrusted text. */
  readonly comment: string | null;
  readonly organizer: Person | null;
  readonly attendees: readonly InvitationAttendee[];
  /** The user's entry among the attendees (null: not invited by name). */
  readonly me: InvitationAttendee | null;
  /** The user organizes it (a REPLY to them, usually). */
  readonly iAmOrganizer: boolean;
  readonly cancelled: boolean;
  /** The event as parsed, to add it to the calendar. */
  readonly event: ParsedEvent;
}

/**
 * The event as parsed, with the stored copy's time if it has none: a
 * REPLY may leave out DTSTART (RFC 5546 §3.2.3; ours does), and its card
 * is shown at the time of the event in our calendar. One occurrence's
 * reply starts at its RECURRENCE-ID.
 */
export const withStoredTiming = (
  event: ParsedEvent,
  stored:
    | {
        readonly start: string | null;
        readonly duration: string | null;
        readonly timeZone: string | null;
        readonly showWithoutTime: boolean;
      }
    | undefined
): ParsedEvent => {
  if (typeof event.start === "string" || typeof stored?.start !== "string") {
    return event;
  }
  return {
    ...event,
    duration: stored.duration,
    showWithoutTime: stored.showWithoutTime,
    start:
      typeof event.recurrenceId === "string"
        ? event.recurrenceId
        : stored.start,
    timeZone: stored.timeZone,
  };
};

/** The main event of a parsed .ics: the series itself, else the first. */
export const mainEvent = (events: readonly ParsedEvent[]): ParsedEvent | null =>
  events.find(
    (event) =>
      (event.recurrenceId === null || event.recurrenceId === undefined) &&
      typeof event.uid === "string"
  ) ??
  events.find((event) => typeof event.uid === "string") ??
  null;

const readAttendees = (
  event: ParsedEvent,
  own: ReadonlySet<string>,
  organizerAddress: string | null
): InvitationAttendee[] =>
  Object.entries(event.participants ?? {}).flatMap(([key, participant]) => {
    const email = participantAddress(participant);
    if (email === null) {
      return [];
    }
    const name = participant.name?.trim() ?? "";
    return [
      {
        email,
        isMe: isOwn(email, own),
        isOrganizer:
          email === organizerAddress || participant.roles?.owner === true,
        key,
        name: name === "" ? null : name,
        status: toParticipationStatus(participant.participationStatus),
      },
    ];
  });

/**
 * The COMMENT of an iTIP message. JSCalendar has no such property; Stalwart
 * keeps it under `iCalendar.properties` as `["comment", {}, type, text]`.
 */
/** Text values of an iCalendar property the server left unconverted. */
const unconverted = (event: ParsedEvent, name: string): string[] => {
  const raw: unknown = Reflect.get(event, "iCalendar");
  if (typeof raw !== "object" || raw === null) {
    return [];
  }
  const properties: unknown = Reflect.get(raw, "properties");
  if (!Array.isArray(properties)) {
    return [];
  }
  return properties.flatMap((entry: unknown) =>
    Array.isArray(entry) &&
    typeof entry[0] === "string" &&
    entry[0].toLowerCase() === name &&
    typeof entry[3] === "string"
      ? [entry[3]]
      : []
  );
};

export const commentOf = (event: ParsedEvent): string | null => {
  const text = unconverted(event, "comment")
    .join("\n")
    .trim()
    .slice(0, MAX_COMMENT);
  return text === "" ? null : text;
};

export const analyzeInvitation = (
  events: readonly ParsedEvent[],
  ownAddresses: readonly string[]
): Invitation | null => {
  const event = mainEvent(events);
  if (event === null || typeof event.uid !== "string" || event.uid === "") {
    return null;
  }
  const own = ownSet(ownAddresses);
  const organizerAddress = addressOf(event.organizerCalendarAddress);
  const attendees = readAttendees(event, own, organizerAddress);
  const host = attendees.find((attendee) => attendee.isOrganizer);
  let organizer: Person | null =
    host === undefined ? null : { email: host.email, name: host.name };
  if (organizer === null && organizerAddress !== null) {
    organizer = { email: organizerAddress, name: null };
  }
  const me =
    attendees.find((attendee) => attendee.isMe && !attendee.isOrganizer) ??
    attendees.find((attendee) => attendee.isMe) ??
    null;
  const method = toMethod(event.method);
  return {
    attendees,
    cancelled:
      method === "cancel" || event.status?.toLowerCase() === "cancelled",
    event,
    iAmOrganizer: isOwn(organizer?.email ?? null, own),
    me,
    method,
    organizer,
    recurrenceId: event.recurrenceId ?? null,
    // Stalwart writes `recurrenceId` in the event's own zone (RFC 8984),
    // whatever `recurrenceIdTimeZone` says (checked with a COUNTER whose
    // DTSTART and RECURRENCE-ID have different zones, 2026-10-26).
    recurrenceIdTimeZone: event.timeZone ?? event.recurrenceIdTimeZone ?? null,
    comment: commentOf(event),
    sequence: Math.max(0, Math.trunc(event.sequence ?? 0)),
    uid: event.uid,
  };
};

/** Which buttons the invitation card shows. */
export type CardAction =
  /** Accept / Maybe / Decline (a REQUEST to the user). */
  | "rsvp"
  /** Offer to remove a cancelled event that is in the calendar. */
  | "remove"
  /** An attendee answered the user's event: apply it to the event. */
  | "apply-reply"
  /** Not addressed to the user by name: offer to add it, nothing else. */
  | "add"
  /** A guest proposes another time for the user's event: accept or decline. */
  | "counter"
  | "none";

export interface ExistingEvent {
  /** Its participants as stored, to compare with the invitation. */
  readonly participants: Readonly<Record<string, Participant>> | null;
  readonly cancelled: boolean;
  /** Its organizer as stored (lower-case address; null: none). */
  readonly organizer: string | null;
  /** That organizer is the user (the stored copy says so, not the email). */
  readonly organizerIsMe: boolean;
  readonly sequence: number;
}

/**
 * Who sent the email (RFC 6047 §2.3: the sender is who acts). `email` is
 * the single From address, lower-case (null: none, several or unreadable);
 * `verdict` the receiving server's DKIM/SPF/DMARC verdict.
 */
export interface InvitationSender {
  readonly email: string | null;
  readonly verdict: AuthVerdict | null;
}

/**
 * The From address as written, lower-case. Never URI-decoded like an
 * iCalendar `mailto:` (`addressOf`): `boss%40corp.com` is not
 * boss@corp.com, and the DMARC verdict is for the address as written.
 */
const fromAddress = (email: string | undefined): string | null => {
  const written = email?.trim() ?? "";
  // A character outside ASCII that lower-cases into ASCII (the Kelvin sign
  // K becomes "k"): it would match an address its DMARC verdict isn't for.
  for (const char of written) {
    if (
      !/^[\0-\u007F]$/u.test(char) &&
      /^[\0-\u007F]+$/u.test(char.toLowerCase())
    ) {
      return null;
    }
  }
  const address = written.toLowerCase();
  return /^[^\s@<>()"%]{1,64}@[^\s@<>()"%]{1,253}$/u.test(address)
    ? address
    : null;
};

export const invitationSender = (
  from: readonly { readonly email: string }[],
  verdict: AuthVerdict | null
): InvitationSender => ({
  email: from.length === 1 ? fromAddress(from[0]?.email) : null,
  verdict,
});

/**
 * Card actions change the calendar, so they need a sender that may act:
 * one From address, not refused by the server's checks ("fail" also
 * covers a softfail with nothing passing; some list-forwarded answers
 * lose their actions). No verdict at all doesn't block: many servers
 * don't sign, but the From must still be the right person.
 */
export const mayAct = (
  sender: InvitationSender
): sender is InvitationSender & {
  readonly email: string;
} => sender.email !== null && sender.verdict !== "fail";

/**
 * Sender checks off: what the card would offer if the sender were the
 * right person (to tell the user why it doesn't).
 */
const ANY_SENDER = "any";
type SenderCheck = InvitationSender | typeof ANY_SENDER;

/** The stored copy's organizer sent it (CANCEL, an update of the event). */
const fromStoredOrganizer = (
  existing: ExistingEvent,
  sender: SenderCheck
): boolean =>
  sender === ANY_SENDER ||
  (mayAct(sender) &&
    existing.organizer !== null &&
    existing.organizer === sender.email);

/** Status of an address in a stored event's participants. */
export const statusIn = (
  participants: Readonly<Record<string, Participant>> | null,
  email: string
): ParticipationStatus | null => {
  for (const participant of Object.values(participants ?? {})) {
    if (participantAddress(participant) === email) {
      return toParticipationStatus(participant.participationStatus);
    }
  }
  return null;
};

/** A stored participant by address (null: not in it). */
export const participantIn = (
  participants: Readonly<Record<string, Participant>> | null | undefined,
  email: string
): Participant | null =>
  Object.values(participants ?? {}).find(
    (participant) => participantAddress(participant) === email
  ) ?? null;

/**
 * What orders an attendee's iTIP responses (RFC 5546 §2.1.5 rules 3 and
 * 4): the SEQUENCE it answers and its DTSTAMP (`updated` once parsed,
 * RFC 8984 §4.4.6 `scheduleUpdated`: "the updated property of the last
 * response when using iTIP").
 */
export interface ResponseStamp {
  readonly sequence: number;
  /** UTCDateTime; null: none (DTSTAMP is required, but not ours to trust). */
  readonly updated: string | null;
}

const UTC_DATE_TIME = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z$/u;

const utcMs = (value: unknown): number | null => {
  if (typeof value !== "string" || !UTC_DATE_TIME.test(value)) {
    return null;
  }
  const ms = Date.parse(value);
  return Number.isFinite(ms) ? ms : null;
};

/** The stamp of a parsed REPLY (or COUNTER). */
export const responseStamp = (event: {
  readonly sequence?: number | null | undefined;
  readonly updated?: string | null | undefined;
}): ResponseStamp => ({
  sequence: Math.max(0, Math.trunc(event.sequence ?? 0)),
  updated: utcMs(event.updated) === null ? null : (event.updated ?? null),
});

/**
 * Whether a response is newer than the attendee's last one the organizer
 * stored (`scheduleSequence`, `scheduleUpdated`, RFC 8984 §4.4.6): a
 * higher SEQUENCE wins; at the same SEQUENCE the later DTSTAMP (RFC 5546
 * §2.1.5). Without a stored stamp any response is newer; with one, a
 * response without a DTSTAMP can't be shown to be.
 */
export const isNewerResponse = (
  stored: Participant | null,
  response: ResponseStamp
): boolean => {
  const storedSequence =
    typeof stored?.scheduleSequence === "number"
      ? stored.scheduleSequence
      : null;
  if (storedSequence !== null && response.sequence !== storedSequence) {
    return response.sequence > storedSequence;
  }
  const storedAt = utcMs(stored?.scheduleUpdated);
  if (storedAt === null) {
    return true;
  }
  const at = utcMs(response.updated);
  return at !== null && at > storedAt;
};

/**
 * The stamp of the response the organizer applies, stored on the
 * attendee (RFC 5546 §2.1.5: "persist the SEQUENCE and DTSTAMP property
 * values associated with the Attendee's last response").
 */
export const responseStampOf = (
  response: ResponseStamp
): Pick<Participant, "scheduleSequence" | "scheduleUpdated"> => ({
  scheduleSequence: response.sequence,
  ...(response.updated === null ? {} : { scheduleUpdated: response.updated }),
});

/** Who answered in a REPLY (the attendees that aren't the organizer). */
export const replyAuthors = (
  invitation: Invitation
): readonly InvitationAttendee[] =>
  invitation.attendees.filter((attendee) => !attendee.isOrganizer);

const authorFor = (
  invitation: Invitation,
  existing: ExistingEvent,
  sender: SenderCheck
): InvitationAttendee | null =>
  replyAuthors(invitation).find(
    (attendee) =>
      (sender === ANY_SENDER || attendee.email === sender.email) &&
      statusIn(existing.participants, attendee.email) !== null
  ) ?? null;

/**
 * Who answers (REPLY) or proposes (COUNTER) for the user's stored event:
 * the attendee that sent the email, and only if they are a guest of it.
 * Never another attendee written in the .ics (anyone can write those).
 */
export const replyAuthor = (
  invitation: Invitation,
  existing: ExistingEvent | null,
  sender: InvitationSender
): InvitationAttendee | null =>
  existing === null || !mayAct(sender)
    ? null
    : authorFor(invitation, existing, sender);

const requestAction = (
  invitation: Invitation,
  existing: ExistingEvent | null,
  sender: SenderCheck
): CardAction => {
  if (sender !== ANY_SENDER && !mayAct(sender)) {
    return "none";
  }
  if (existing !== null) {
    // An update of an event in the calendar: only from its organizer.
    // (SENT-BY is written by the sender: never trusted. A delegate's
    // email shows the card without actions.)
    if (!fromStoredOrganizer(existing, sender)) {
      return "none";
    }
    if (invitation.cancelled) {
      return "remove";
    }
    return invitation.me !== null && !existing.organizerIsMe ? "rsvp" : "none";
  }
  if (invitation.cancelled) {
    return "none";
  }
  if (invitation.me !== null && !invitation.iAmOrganizer) {
    return "rsvp";
  }
  return invitation.iAmOrganizer ? "none" : "add";
};

/** A REPLY to the user's event: offer to apply the sender's answer if it differs. */
const replyAction = (
  invitation: Invitation,
  existing: ExistingEvent | null,
  sender: SenderCheck
): CardAction => {
  if (existing === null || !existing.organizerIsMe) {
    return "none";
  }
  if (sender !== ANY_SENDER && !mayAct(sender)) {
    return "none";
  }
  const author = authorFor(invitation, existing, sender);
  if (author === null) {
    return "none";
  }
  const stored = participantIn(existing.participants, author.email);
  // An older response than the last one applied changes nothing (RFC
  // 5546 §2.1.5), even if its answer differs.
  return isNewerResponse(stored, responseStamp(invitation.event)) &&
    statusIn(existing.participants, author.email) !== author.status
    ? "apply-reply"
    : "none";
};

/**
 * A guest of the user's event proposes another time: only from someone
 * already invited (anyone can send a COUNTER), for an event still on.
 */
const counterAction = (
  invitation: Invitation,
  existing: ExistingEvent | null,
  sender: SenderCheck
): CardAction => {
  if (existing === null || !existing.organizerIsMe || existing.cancelled) {
    return "none";
  }
  if (sender !== ANY_SENDER && !mayAct(sender)) {
    return "none";
  }
  return authorFor(invitation, existing, sender) === null ? "none" : "counter";
};

const actionFor = (
  invitation: Invitation,
  existing: ExistingEvent | null,
  sender: SenderCheck
): CardAction => {
  // The calendar has a newer version: an old email changes nothing.
  if (existing !== null && existing.sequence > invitation.sequence) {
    return "none";
  }
  switch (invitation.method) {
    case "request":
    case "publish": {
      return requestAction(invitation, existing, sender);
    }
    case "cancel": {
      return existing !== null && fromStoredOrganizer(existing, sender)
        ? "remove"
        : "none";
    }
    case "reply": {
      return replyAction(invitation, existing, sender);
    }
    case "counter": {
      return counterAction(invitation, existing, sender);
    }
    case "declinecounter":
    case "other": {
      return "none";
    }
    default: {
      return invitation.method satisfies never;
    }
  }
};

/**
 * Which buttons the card shows. Any action needs the email to come from
 * the right person: the stored organizer for an update or a cancellation,
 * the answering guest for a REPLY or a COUNTER.
 */
export const cardAction = (
  invitation: Invitation,
  existing: ExistingEvent | null,
  sender: InvitationSender
): CardAction => actionFor(invitation, existing, sender);

/**
 * The card would offer something if the email came from the right person:
 * it says why it doesn't.
 */
export const senderBlocked = (
  invitation: Invitation,
  existing: ExistingEvent | null,
  sender: InvitationSender
): boolean =>
  actionFor(invitation, existing, sender) === "none" &&
  actionFor(invitation, existing, ANY_SENDER) !== "none";

// --- REPLY ------------------------------------------------------------------

/** RFC 5545 §3.3.11 TEXT: escape backslash, semicolon, comma and newlines. */
export const escapeText = (text: string): string =>
  text
    .replaceAll("\\", "\\\\")
    .replaceAll(";", "\\;")
    .replaceAll(",", "\\,")
    .replaceAll(/\r\n|\r|\n/gu, "\\n");

/** A parameter value (CN=…): quoted, without quotes or control characters. */
const paramValue = (text: string): string =>
  `"${text.replaceAll(/["\p{Cc}]/gu, "").slice(0, 200)}"`;

const encoder = new TextEncoder();

/**
 * RFC 5545 §3.1: lines of at most 75 octets, continued with CRLF + space,
 * never splitting a UTF-8 character.
 */
export const foldLine = (line: string): string => {
  const out: string[] = [];
  let current = "";
  let size = 0;
  for (const char of line) {
    const bytes = encoder.encode(char).length;
    const limit = out.length === 0 ? 75 : 74;
    if (size + bytes > limit) {
      out.push(current);
      current = "";
      size = 0;
    }
    current += char;
    size += bytes;
  }
  out.push(current);
  return out.join("\r\n ");
};

const PARTSTAT: Readonly<Record<ParticipationStatus, string>> = {
  accepted: "ACCEPTED",
  declined: "DECLINED",
  delegated: "DELEGATED",
  "needs-action": "NEEDS-ACTION",
  tentative: "TENTATIVE",
};

export interface ReplyInput extends ItipCommon {
  readonly status: Exclude<ParticipationStatus, "needs-action" | "delegated">;
  readonly title: string;
}

const nameParam = (person: Person): string =>
  person.name === null || person.name === ""
    ? ""
    : `;CN=${paramValue(person.name)}`;

/** RECURRENCE-ID and the VTIMEZONE its TZID needs. */
interface RecurrenceParts {
  readonly line: readonly string[];
  readonly timeZone: readonly string[];
}

/**
 * RECURRENCE-ID in the form of the series' DTSTART (RFC 5545 §3.8.4.4:
 * the same value type): a DATE for an all-day series, a time in UTC, a
 * floating time, or a time in the series' zone (`TZID`). A `TZID` comes
 * with its VTIMEZONE (§3.2.19: one for every TZID used, also in iTIP
 * messages, RFC 5546 §1.4); a zone `Intl` doesn't know can't have one,
 * so the message can't be written (it throws).
 */
const recurrenceParts = (
  recurrenceId: string | null,
  zone: string | null,
  allDay = false
): RecurrenceParts => {
  const none = { line: [], timeZone: [] };
  if (allDay) {
    const day = recurrenceId === null ? null : dateStamp(recurrenceId);
    return day === null
      ? none
      : { line: [`RECURRENCE-ID;VALUE=DATE:${day}`], timeZone: [] };
  }
  const recurrence = recurrenceId === null ? null : localStamp(recurrenceId);
  if (recurrence === null || recurrenceId === null) {
    return none;
  }
  if (isUtcZone(zone)) {
    return { line: [`RECURRENCE-ID:${recurrence}Z`], timeZone: [] };
  }
  const id = zone === null ? null : tzid(zone);
  if (id === null) {
    return { line: [`RECURRENCE-ID:${recurrence}`], timeZone: [] };
  }
  const at = instantOf({ kind: "local", local: recurrenceId, timeZone: id });
  const timeZone = at === null ? null : vtimezoneLines(id, at);
  if (timeZone === null) {
    throw new Error("The occurrence's time zone can't be written");
  }
  return { line: [`RECURRENCE-ID;TZID=${id}:${recurrence}`], timeZone };
};

const commentLine = (comment: string | undefined): string[] => {
  const text = comment?.trim().slice(0, MAX_COMMENT) ?? "";
  return text === "" ? [] : [`COMMENT:${escapeText(text)}`];
};

interface ItipCommon {
  readonly uid: string;
  readonly sequence: number;
  readonly organizer: Person;
  readonly attendee: Person;
  readonly recurrenceId: string | null;
  readonly recurrenceIdTimeZone: string | null;
  /** An all-day event: RECURRENCE-ID is a DATE. */
  readonly allDay?: boolean;
  /** DTSTAMP, epoch ms. */
  readonly now: number;
  readonly comment?: string;
}

const calendarObject = (
  method: "REPLY" | "COUNTER" | "DECLINECOUNTER",
  input: ItipCommon,
  partstat: string | null,
  body: readonly string[],
  component: "VEVENT" | "VTODO" = "VEVENT",
  /** More parameters of the attendee (";DELEGATED-TO=…"). */
  attendeeParams = ""
): string => {
  const recurrence = recurrenceParts(
    input.recurrenceId,
    input.recurrenceIdTimeZone,
    input.allDay
  );
  const lines = [
    "BEGIN:VCALENDAR",
    "VERSION:2.0",
    "PRODID:-//Nebula Mail//EN",
    `METHOD:${method}`,
    ...recurrence.timeZone,
    `BEGIN:${component}`,
    `UID:${escapeText(input.uid)}`,
    `DTSTAMP:${utcStamp(input.now)}`,
    `SEQUENCE:${String(input.sequence)}`,
    ...recurrence.line,
    `ORGANIZER${nameParam(input.organizer)}:mailto:${input.organizer.email}`,
    `ATTENDEE${partstat === null ? "" : `;PARTSTAT=${partstat}`}${attendeeParams}${nameParam(input.attendee)}:mailto:${input.attendee.email}`,
    ...body,
    ...commentLine(input.comment),
    `END:${component}`,
    "END:VCALENDAR",
  ];
  return `${lines.map(foldLine).join("\r\n")}\r\n`;
};

/**
 * The iCalendar object of a REPLY (RFC 5546 §3.2.3), CRLF line ends. The
 * optional comment goes in COMMENT (the email's text says it too).
 */
export const buildReply = (input: ReplyInput): string =>
  calendarObject("REPLY", input, PARTSTAT[input.status], [
    `SUMMARY:${escapeText(input.title)}`,
  ]);

export interface TaskReplyInput extends ItipCommon {
  /** "ACCEPTED", "DECLINED", "IN-PROCESS", "COMPLETED" (RFC 5545 VTODO). */
  readonly partstat: string;
  readonly title: string;
}

/**
 * The REPLY of an assignee to an assigned task (RFC 5546 §3.4.3): a VTODO
 * with their PARTSTAT, which for tasks also says how far along they are
 * (IN-PROCESS, COMPLETED).
 */
export const buildTaskReply = (input: TaskReplyInput): string =>
  calendarObject(
    "REPLY",
    input,
    /^[A-Z-]{1,20}$/u.test(input.partstat) ? input.partstat : "ACCEPTED",
    [`SUMMARY:${escapeText(input.title)}`],
    "VTODO"
  );

/** A task's due in an iTIP message: a day, or an instant (sent in UTC). */
export type ItipDue =
  | { readonly kind: "date"; readonly date: string }
  | { readonly kind: "instant"; readonly at: number };

/** DUE of a VTODO (RFC 5545 §3.8.2.3); none if the day is invalid. */
const dueLine = (due: ItipDue | null): string[] => {
  if (due === null) {
    return [];
  }
  if (due.kind === "instant") {
    return [`DUE:${utcStamp(due.at)}`];
  }
  const day = dateStamp(due.date);
  return day === null ? [] : [`DUE;VALUE=DATE:${day}`];
};

/** A calendar address as a quoted parameter value (DELEGATED-TO="…"). */
const addressParam = (email: string): string =>
  `"mailto:${email.replaceAll(/["\p{Cc}]/gu, "")}"`;

/** A VTODO PARTSTAT (RFC 5545 §3.2.12); anything odd is NEEDS-ACTION. */
const taskPartstat = (value: string): string =>
  /^[A-Z-]{1,20}$/u.test(value) ? value : "NEEDS-ACTION";

export interface TaskCounterInput extends ItipCommon {
  readonly title: string;
  /** The due the assignee proposes. */
  readonly due: ItipDue;
  /** Their answer so far ("ACCEPTED", "IN-PROCESS"…; COUNTER keeps it). */
  readonly partstat: string;
}

/**
 * An assignee proposes another due to the organizer (COUNTER, RFC 5546
 * §3.4.7). Nothing changes until the organizer accepts it (a new REQUEST,
 * sent by the server) or declines it (DECLINECOUNTER).
 */
export const buildTaskCounter = (input: TaskCounterInput): string =>
  calendarObject(
    "COUNTER",
    input,
    taskPartstat(input.partstat),
    [`SUMMARY:${escapeText(input.title)}`, ...dueLine(input.due)],
    "VTODO"
  );

/** The organizer keeps the due (DECLINECOUNTER, RFC 5546 §3.4.8). */
export const buildTaskDeclineCounter = (input: ItipCommon): string =>
  calendarObject("DECLINECOUNTER", input, null, [], "VTODO");

export interface TaskDelegationInput extends ItipCommon {
  readonly title: string;
  /** Who the assignee passes the task on to. */
  readonly delegate: Person;
  /**
   * Who passed it on to the assignee, if they got it delegated (passing it
   * on again: their line keeps DELEGATED-FROM, RFC 5546 §3.2.2.3).
   */
  readonly delegatedFrom?: string | null;
}

/**
 * The assignee passes the task on (RFC 5546 §3.2.2.3, the same for
 * VTODO): a REPLY with PARTSTAT=DELEGATED and DELEGATED-TO, and the
 * delegate as a new attendee with DELEGATED-FROM, so the organizer's
 * server can add them (Stalwart 1.0.1 does, `EntryChange::participation`).
 * A repeating task gets the same message: a REPLY without RECURRENCE-ID is
 * about the whole series (RFC 5546 §3.2.3).
 */
export const buildTaskDelegation = (input: TaskDelegationInput): string =>
  calendarObject(
    "REPLY",
    input,
    "DELEGATED",
    [
      `ATTENDEE;PARTSTAT=NEEDS-ACTION;RSVP=TRUE;DELEGATED-FROM=${addressParam(input.attendee.email)}${nameParam(input.delegate)}:mailto:${input.delegate.email}`,
      `SUMMARY:${escapeText(input.title)}`,
    ],
    "VTODO",
    `;DELEGATED-TO=${addressParam(input.delegate.email)}${
      input.delegatedFrom === null || input.delegatedFrom === undefined
        ? ""
        : `;DELEGATED-FROM=${addressParam(input.delegatedFrom)}`
    }`
  );

export interface TaskRequestAttendee extends Person {
  /** "NEEDS-ACTION", "ACCEPTED", "DELEGATED", "COMPLETED"… */
  readonly partstat: string;
  readonly delegatedTo: string | null;
  readonly delegatedFrom: string | null;
  /** An answer is expected (RSVP=TRUE). */
  readonly rsvp: boolean;
}

/** One occurrence of a series that differs from it (moved, renamed, done). */
export interface ItipOverride {
  /** The occurrence's original start, in the series' form. */
  readonly recurrenceId: ICalTime;
  readonly start: ICalTime;
  readonly due: ICalTime | null;
  /** Its own title (null: the series'). */
  readonly title: string | null;
  readonly completed: boolean;
}

/** A repeating task as an iTIP message carries it. */
export interface ItipSeries {
  /** DTSTART: what the RRULE counts from. */
  readonly start: ICalTime;
  readonly due: ICalTime | null;
  /** The RRULE value (`rruleOf`). */
  readonly rrule: string;
  /** Occurrences taken out (EXDATE). */
  readonly excluded: readonly ICalTime[];
  /**
   * Occurrences that may not come from the rule (RDATE, RFC 5545
   * §3.8.5.2): the set is the union, and duplicates are ignored.
   */
  readonly added?: readonly ICalTime[];
  readonly overrides: readonly ItipOverride[];
}

interface SeriesLines {
  /** The VTIMEZONE (empty: in UTC, floating or days). */
  readonly timeZone: readonly string[];
  /** DTSTART, DUE, RRULE, RDATE and EXDATE of the series. */
  readonly master: readonly string[];
  /** RECURRENCE-ID, DTSTART, DUE, SUMMARY and STATUS of each override. */
  readonly overrides: readonly (readonly string[])[];
}

/**
 * A time line of a series: undefined without a time, null if it can't be
 * written or isn't in the series' zone (one VTIMEZONE at most).
 */
const seriesTimeLine = (
  name: string,
  time: ICalTime | null,
  zone: string | null
): string | null | undefined => {
  if (time === null) {
    return undefined;
  }
  if (time.kind === "local" && time.timeZone !== zone) {
    return null;
  }
  return timeLine(name, time);
};

const overrideLines = (
  item: ItipOverride,
  zone: string | null
): string[] | null => {
  const id = seriesTimeLine("RECURRENCE-ID", item.recurrenceId, zone);
  const start = seriesTimeLine("DTSTART", item.start, zone);
  const due = seriesTimeLine("DUE", item.due, zone);
  if (
    id === null ||
    id === undefined ||
    start === null ||
    start === undefined ||
    due === null
  ) {
    return null;
  }
  return [
    id,
    start,
    ...(due === undefined ? [] : [due]),
    ...(item.title === null ? [] : [`SUMMARY:${escapeText(item.title)}`]),
    ...(item.completed ? ["STATUS:COMPLETED"] : []),
  ];
};

/** The VTIMEZONE a series needs: none for days, floating or UTC. */
const seriesZone = (start: ICalTime): readonly string[] | null => {
  if (start.kind === "date" || start.timeZone === null) {
    return [];
  }
  const from = instantOf(start);
  if (from === null) {
    return null;
  }
  return isUtcZone(start.timeZone) ? [] : vtimezoneLines(start.timeZone, from);
};

/**
 * The iCalendar lines of a series; null if any part can't be written (the
 * app then doesn't offer to send it: `seriesWritable`).
 */
const seriesLines = (series: ItipSeries): SeriesLines | null => {
  const zone = series.start.kind === "local" ? series.start.timeZone : null;
  const allDay = series.start.kind === "date";
  // RFC 5545: RDATE, EXDATE and RECURRENCE-ID have DTSTART's value type.
  const sameKind = (time: ICalTime | null): boolean =>
    time === null || (time.kind === "date") === allDay;
  const addedTimes = series.added ?? [];
  const times = [
    series.due,
    ...addedTimes,
    ...series.excluded,
    ...series.overrides.flatMap((item) => [
      item.recurrenceId,
      item.start,
      item.due,
    ]),
  ];
  const start = seriesTimeLine("DTSTART", series.start, zone);
  const due = seriesTimeLine("DUE", series.due, zone);
  const excluded = series.excluded.map((time) =>
    seriesTimeLine("EXDATE", time, zone)
  );
  const added = addedTimes.map((time) => seriesTimeLine("RDATE", time, zone));
  const overrides = series.overrides.map((item) => overrideLines(item, zone));
  const timeZone = seriesZone(series.start);
  if (
    !times.every(sameKind) ||
    start === null ||
    start === undefined ||
    due === null ||
    timeZone === null ||
    !/^FREQ=[A-Z0-9=;,+-]{1,500}$/u.test(series.rrule)
  ) {
    return null;
  }
  const exdates = excluded.flatMap((line) =>
    typeof line === "string" ? [line] : []
  );
  const rdates = added.flatMap((line) =>
    typeof line === "string" ? [line] : []
  );
  const written = overrides.flatMap((lines) => (lines === null ? [] : [lines]));
  if (
    exdates.length !== excluded.length ||
    rdates.length !== added.length ||
    written.length !== overrides.length
  ) {
    return null;
  }
  return {
    master: [
      start,
      ...(due === undefined ? [] : [due]),
      `RRULE:${series.rrule}`,
      ...rdates,
      ...exdates,
    ],
    overrides: written,
    timeZone,
  };
};

/** Whether a series can be written into an iTIP message. */
export const seriesWritable = (series: ItipSeries): boolean =>
  seriesLines(series) !== null;

export interface TaskRequestInput {
  readonly uid: string;
  readonly sequence: number;
  readonly now: number;
  readonly organizer: Person;
  readonly attendees: readonly TaskRequestAttendee[];
  readonly title: string;
  /** Plain text (null: none). */
  readonly description: string | null;
  readonly due: ItipDue | null;
  /**
   * A repeating task: DTSTART, DUE, RRULE and EXDATE in the task's own
   * zone (with its VTIMEZONE) and one VTODO per occurrence that differs.
   * `due` isn't used then.
   */
  readonly series?: ItipSeries | null;
  readonly comment?: string;
}

/**
 * The task as the delegator sends it to the delegate (RFC 5546 §3.2.2.3:
 * a copy of the REQUEST with the delegation in it). A task that doesn't
 * repeat: its due in UTC or as a day, so no VTIMEZONE is needed. A
 * repeating one: the whole series, its times in the task's zone so that it
 * keeps its wall time across DST changes, with that zone's VTIMEZONE (RFC
 * 5545 §3.6.5). The sender's alerts and links are never sent.
 */
export const buildTaskRequest = (input: TaskRequestInput): string => {
  const description = input.description?.trim() ?? "";
  const wanted = input.series ?? null;
  const series = wanted === null ? null : seriesLines(wanted);
  if (wanted !== null && series === null) {
    throw new Error("The series can't be written");
  }
  const head = [
    `UID:${escapeText(input.uid)}`,
    `DTSTAMP:${utcStamp(input.now)}`,
    `SEQUENCE:${String(input.sequence)}`,
  ];
  const people = [
    `ORGANIZER${nameParam(input.organizer)}:mailto:${input.organizer.email}`,
    ...input.attendees.map(
      (attendee) =>
        `ATTENDEE;PARTSTAT=${taskPartstat(attendee.partstat)}${attendee.rsvp ? ";RSVP=TRUE" : ""}${attendee.delegatedTo === null ? "" : `;DELEGATED-TO=${addressParam(attendee.delegatedTo)}`}${attendee.delegatedFrom === null ? "" : `;DELEGATED-FROM=${addressParam(attendee.delegatedFrom)}`}${nameParam(attendee)}:mailto:${attendee.email}`
    ),
  ];
  const title = `SUMMARY:${escapeText(input.title)}`;
  const lines = [
    "BEGIN:VCALENDAR",
    "VERSION:2.0",
    "PRODID:-//Nebula Mail//EN",
    "METHOD:REQUEST",
    ...(series?.timeZone ?? []),
    "BEGIN:VTODO",
    ...head,
    title,
    ...(description === "" ? [] : [`DESCRIPTION:${escapeText(description)}`]),
    ...(series === null ? dueLine(input.due) : series.master),
    ...people,
    ...commentLine(input.comment),
    "END:VTODO",
    // Each occurrence that differs: its own VTODO with the same UID and
    // people (RFC 5546 §3.2.2: every component of a REQUEST has them).
    ...(series?.overrides ?? []).flatMap((override) => [
      "BEGIN:VTODO",
      ...head,
      ...(override.some((line) => line.startsWith("SUMMARY:")) ? [] : [title]),
      ...override,
      ...people,
      "END:VTODO",
    ]),
    "END:VCALENDAR",
  ];
  return `${lines.map(foldLine).join("\r\n")}\r\n`;
};

export interface CounterInput extends ItipCommon {
  readonly title: string;
  /** The proposed time, epoch ms ([start, end)). */
  readonly start: number;
  readonly end: number;
  /** The attendee's answer so far (COUNTER keeps it). */
  readonly status: ParticipationStatus;
}

/**
 * A COUNTER (RFC 5546 §3.2.7): an attendee proposes another time. Times
 * in UTC, so no VTIMEZONE is needed. Nothing changes until the organizer
 * accepts it (with a new REQUEST) or declines it (DECLINECOUNTER).
 */
export const buildCounter = (input: CounterInput): string =>
  calendarObject("COUNTER", input, PARTSTAT[input.status], [
    `SUMMARY:${escapeText(input.title)}`,
    `DTSTART:${utcStamp(input.start)}`,
    `DTEND:${utcStamp(Math.max(input.start, input.end))}`,
  ]);

export interface RequestGuest extends Person {
  readonly status: ParticipationStatus;
  readonly optional: boolean;
}

interface OccurrenceRequestCommon {
  readonly uid: string;
  readonly sequence: number;
  readonly organizer: Person;
  readonly guests: readonly RequestGuest[];
  /** The occurrence's original start (local, in `recurrenceIdTimeZone`). */
  readonly recurrenceId: string;
  readonly recurrenceIdTimeZone: string | null;
  readonly title: string;
  readonly location: string | null;
  readonly now: number;
}

/** Its new time: instants, or whole days for an all-day occurrence. */
export type OccurrenceRequestInput = OccurrenceRequestCommon &
  (
    | {
        readonly allDay?: false;
        /** Epoch ms ([start, end)). */
        readonly start: number;
        readonly end: number;
      }
    | {
        readonly allDay: true;
        /** "2026-11-30": the first day, and the day after the last one. */
        readonly startDate: string;
        readonly endDate: string;
      }
  );

/**
 * RECURRENCE-ID, DTSTART and DTEND of the occurrence. All-day ones are
 * DATE values without TZID (RFC 5545 §3.8.4.4: RECURRENCE-ID has the value
 * type of the series' DTSTART); DTEND is the day after the last one.
 */
const occurrenceTiming = (input: OccurrenceRequestInput): RecurrenceParts => {
  if (input.allDay !== true) {
    const recurrence = recurrenceParts(
      input.recurrenceId,
      input.recurrenceIdTimeZone
    );
    return {
      line: [
        ...recurrence.line,
        `DTSTART:${utcStamp(input.start)}`,
        `DTEND:${utcStamp(Math.max(input.start, input.end))}`,
      ],
      timeZone: recurrence.timeZone,
    };
  }
  const recurrence = dateStamp(input.recurrenceId);
  const first = parseLocalDate(input.startDate);
  const end = parseLocalDate(input.endDate);
  if (recurrence === null || first === null || end === null) {
    throw new Error("Invalid all-day occurrence");
  }
  const after = end.compare(first) > 0 ? end : first.add({ days: 1 });
  return {
    line: [
      `RECURRENCE-ID;VALUE=DATE:${recurrence}`,
      `DTSTART;VALUE=DATE:${first.toString().replaceAll("-", "")}`,
      `DTEND;VALUE=DATE:${after.toString().replaceAll("-", "")}`,
    ],
    timeZone: [],
  };
};

/**
 * A REQUEST for one occurrence (RFC 5546 §3.2.2, with RECURRENCE-ID),
 * after the organizer changed only it. Times in UTC; all-day ones as
 * dates; RECURRENCE-ID as the series' DTSTART, with its VTIMEZONE.
 * Description and links aren't sent (the series' copy keeps them).
 * The app no longer sends it: Stalwart 1.0.1 and the bridge send the
 * REQUEST for one changed occurrence themselves, with SEQUENCE raised
 * (checked live 2026-11-08). The live test uses it to play
 * an organizer whose server doesn't schedule.
 */
export const buildOccurrenceRequest = (
  input: OccurrenceRequestInput
): string => {
  const timing = occurrenceTiming(input);
  const lines = [
    "BEGIN:VCALENDAR",
    "VERSION:2.0",
    "PRODID:-//Nebula Mail//EN",
    "METHOD:REQUEST",
    ...timing.timeZone,
    "BEGIN:VEVENT",
    `UID:${escapeText(input.uid)}`,
    `DTSTAMP:${utcStamp(input.now)}`,
    `SEQUENCE:${String(input.sequence)}`,
    ...timing.line,
    `SUMMARY:${escapeText(input.title)}`,
    ...(input.location === null || input.location.trim() === ""
      ? []
      : [`LOCATION:${escapeText(input.location.trim())}`]),
    `ORGANIZER${nameParam(input.organizer)}:mailto:${input.organizer.email}`,
    `ATTENDEE;PARTSTAT=ACCEPTED;ROLE=CHAIR${nameParam(input.organizer)}:mailto:${input.organizer.email}`,
    ...input.guests.map(
      (guest) =>
        `ATTENDEE;PARTSTAT=${PARTSTAT[guest.status]};ROLE=${guest.optional ? "OPT-PARTICIPANT" : "REQ-PARTICIPANT"};RSVP=TRUE${nameParam(guest)}:mailto:${guest.email}`
    ),
    "END:VEVENT",
    "END:VCALENDAR",
  ];
  return `${lines.map(foldLine).join("\r\n")}\r\n`;
};

/** The organizer turns down a proposal (RFC 5546 §3.2.8). */
export const buildDeclineCounter = (input: ItipCommon): string =>
  calendarObject("DECLINECOUNTER", input, null, []);

/**
 * What an occurrence's override may change when imported: the same
 * scheduling data as the event. Patch keys are paths ("alerts/a1/trigger"):
 * their first segment decides, so the sender's alerts, links, rules or
 * UID never come in through an override. Never `privacy`: RFC 8984 §4.3.5
 * says a pointer starting with it MUST be ignored.
 */
const OVERRIDE_KEEP: ReadonlySet<string> = new Set([
  "description",
  "descriptionContentType",
  "duration",
  "excluded",
  "freeBusyStatus",
  "locations",
  "participants",
  "sequence",
  "showWithoutTime",
  "start",
  "status",
  "timeZone",
  "title",
  "virtualLocations",
]);

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/**
 * The user's answer in one override, without making the patch invalid: a
 * PatchObject can't have a path and a path under it (RFC 8984 §1.4.9),
 * so it goes inside "participants" or "participants/<me>" when the
 * override has them, and as its own path only otherwise.
 */
const withMyStatus = (
  patch: Record<string, unknown>,
  me: InvitationAttendee,
  status: ParticipationStatus
): Record<string, unknown> => {
  if (patch.excluded === true) {
    return patch;
  }
  const own = `participants/${me.key}`;
  const { participants } = patch;
  if (participants !== undefined) {
    if (!isRecord(participants)) {
      return patch;
    }
    const mine = participants[me.key];
    // The occurrence's own guest list without the user: leave it.
    return isRecord(mine)
      ? {
          ...patch,
          participants: {
            ...participants,
            [me.key]: { ...mine, participationStatus: status },
          },
        }
      : patch;
  }
  const mine = patch[own];
  if (mine !== undefined) {
    return isRecord(mine)
      ? { ...patch, [own]: { ...mine, participationStatus: status } }
      : patch;
  }
  return { ...patch, [`${own}/participationStatus`]: status };
};

/** The overrides to import (null: none left). */
const overridesToImport = (
  overrides: Readonly<Record<string, Readonly<Record<string, unknown>>>>,
  me: InvitationAttendee | null,
  status: ParticipationStatus
): Record<string, Record<string, unknown>> | null => {
  const result: Record<string, Record<string, unknown>> = {};
  for (const [recurrenceId, patch] of Object.entries(overrides)) {
    const kept = Object.fromEntries(
      Object.entries(patch).filter(([path]) =>
        OVERRIDE_KEEP.has(path.split("/")[0] ?? "")
      )
    );
    // Every key stays (a LocalDateTime, RFC 8984 §4.3.5), even with
    // nothing left to patch: one that isn't an occurrence of the rule is
    // an added one, like an RDATE ("the patch object may often be empty in
    // this case"). An empty one inherits the user's answer from the event.
    if (parseLocalDateTime(recurrenceId) !== null) {
      result[recurrenceId] =
        me === null || Object.keys(kept).length === 0
          ? kept
          : withMyStatus(kept, me, status);
    }
  }
  return Object.keys(result).length === 0 ? null : result;
};

/**
 * The event to store when the user accepts an invitation that isn't in the
 * calendar yet. Only scheduling data: the sender's alerts and links are
 * left out (they would ring or open things the user didn't choose).
 */
export const eventToImport = (
  event: ParsedEvent,
  me: InvitationAttendee | null,
  status: ParticipationStatus
): Record<string, unknown> => {
  const keep = [
    "@type",
    "description",
    "descriptionContentType",
    "duration",
    "freeBusyStatus",
    "locations",
    "organizerCalendarAddress",
    "privacy",
    "recurrenceId",
    "recurrenceIdTimeZone",
    "recurrenceRule",
    "sequence",
    "showWithoutTime",
    "start",
    "status",
    "timeZone",
    "title",
    "uid",
    "virtualLocations",
  ] as const;
  const copy: Record<string, unknown> = {};
  for (const key of keep) {
    const value = event[key];
    if (value !== undefined && value !== null) {
      copy[key] = value;
    }
  }
  const participants: Record<string, Participant> = {};
  for (const [key, participant] of Object.entries(event.participants ?? {})) {
    participants[key] =
      me !== null && key === me.key
        ? { ...participant, participationStatus: status }
        : participant;
  }
  if (Object.keys(participants).length > 0) {
    copy.participants = participants;
  }
  const overrides =
    event.recurrenceOverrides === null ||
    event.recurrenceOverrides === undefined
      ? null
      : overridesToImport(event.recurrenceOverrides, me, status);
  if (overrides !== null) {
    copy.recurrenceOverrides = overrides;
  }
  return copy;
};
