// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Invitations in the mail reader. The .ics part is
// parsed by the server (`CalendarEvent/parse`); nothing is stored until the
// user presses a button.
//
// Replies (decided by testing Stalwart): the server sends an iTIP
// REPLY by itself only for events it took in from an iMIP email, and only
// with `sendSchedulingMessages`; for an invitation the user adds by hand it
// sends nothing. So the app always updates the stored event *without*
// scheduling and sends the REPLY itself (one email, never two). The
// organizer side (invitations, updates, cancellations) is left to the
// server (`sendSchedulingMessages: true`).
//
// With the bridge the bridge also sends the
// REQUEST of one changed occurrence (with `sendSchedulingMessages`) and the
// proposals: COUNTER and DECLINECOUNTER go by its own methods
// `CalendarEvent/counter` and `CalendarEvent/declineCounter`, which build
// the message from the stored event or the email. With Stalwart the app
// sends those itself, as before.

import {
  fromAbsolute,
  toCalendarDateTime,
  toZoned,
} from "@internationalized/date";
import type { CalendarDate } from "@internationalized/date";
import type {
  AccountId,
  BlobId,
  CalendarEventId,
  CalendarId,
  JmapClient,
  MailboxId,
  Participant,
} from "@nebulamail/jmap-client";
import {
  CAPABILITIES,
  CalendarEventId as CalendarEventIdSchema,
  hasCapability,
  isTask,
} from "@nebulamail/jmap-client";

import { getClient } from "../mail/client";
import {
  createEvent,
  deleteEvent,
  findByUid,
  listOccurrences,
  updateEvent,
} from "./api";
import type {
  CardAction,
  Invitation,
  InvitationAttendee,
  InvitationSender,
} from "./itip";
import {
  addressOf,
  analyzeInvitation,
  MAX_COMMENT,
  buildCounter,
  buildDeclineCounter,
  buildReply,
  cardAction,
  eventToImport,
  ownSet,
  replyAuthor,
  responseStamp,
  responseStampOf,
  senderBlocked,
  withStoredTiming,
} from "./itip";
import { mapContext, toOccurrence } from "./mapping";
import type {
  CalendarSummary,
  EventDetail,
  Occurrence,
  ParticipationStatus,
  Person,
} from "./model";
import { NoSendingIdentityError } from "./send-identity-error";
import { ownAccountOf } from "./targets";
import {
  dateInZone,
  formatDuration,
  formatLocalDateTime,
  parseLocalDateTime,
} from "./time";

export { MAX_INVITATION_BYTES, invitationPart } from "./invitation-part";
export { NoSendingIdentityError } from "./send-identity-error";

export interface StoredCopy {
  readonly id: CalendarEventId;
  readonly participants: Readonly<Record<string, Participant>> | null;
  readonly cancelled: boolean;
  /** Its organizer as stored (lower-case address; null: none). */
  readonly organizer: string | null;
  /** That organizer is the user. */
  readonly organizerIsMe: boolean;
  readonly sequence: number;
  /** Its zone (null: floating or all-day). */
  readonly timeZone: string | null;
  /** Its start and length as stored (to show "now at …" for a proposal). */
  readonly start: string | null;
  readonly duration: string | null;
}

export interface InvitationView {
  /** The email's iCalendar part (the bridge reads it for a proposal). */
  readonly blobId: BlobId;
  readonly invitation: Invitation;
  /** The event as it would show in the calendar (times in the user's zone). */
  readonly occurrence: Occurrence;
  /** The same event already in the calendar (by UID). */
  readonly stored: StoredCopy | null;
  readonly action: CardAction;
  /** Who sent the email (the card's actions depend on it). */
  readonly sender: InvitationSender;
  /** It would offer actions if the email came from the right person. */
  readonly senderBlocked: boolean;
  /** The user's events at the same time (first occurrence only). */
  readonly conflicts: readonly Occurrence[];
  /** The calendar has a newer version (this email is an old one). */
  readonly outdated: boolean;
  /** The user's status now: the stored one, else the invitation's. */
  readonly myStatus: ParticipationStatus | null;
}

const PLACEHOLDER_ID = CalendarEventIdSchema.parse("invitation");

export interface LoadOptions {
  readonly zone: string;
  readonly own: readonly string[];
  readonly calendars: readonly CalendarSummary[];
}

const storedStatus = (
  stored: StoredCopy | null,
  me: InvitationAttendee | null
): ParticipationStatus | null => {
  if (stored === null || me === null) {
    return me?.status ?? null;
  }
  for (const participant of Object.values(stored.participants ?? {})) {
    const email =
      addressOf(participant.calendarAddress) ?? addressOf(participant.email);
    if (email === me.email) {
      const status = participant.participationStatus?.toLowerCase();
      return status === "accepted" ||
        status === "declined" ||
        status === "tentative" ||
        status === "delegated"
        ? status
        : "needs-action";
    }
  }
  return me.status;
};

/** The days [start, end) an occurrence touches in a zone. */
const daysOf = (
  occurrence: { readonly start: number; readonly end: number },
  zone: string
): { readonly start: CalendarDate; readonly end: CalendarDate } => ({
  end: dateInZone(Math.max(occurrence.start, occurrence.end - 1), zone).add({
    days: 1,
  }),
  start: dateInZone(occurrence.start, zone),
});

/** The user's events on those days, for conflicts (none if that fails). */
const eventsAround = async (
  days: { readonly start: CalendarDate; readonly end: CalendarDate },
  options: LoadOptions
): Promise<Occurrence[]> =>
  await listOccurrences({
    calendars: options.calendars,
    end: days.end,
    own: options.own,
    start: days.start,
    zone: options.zone,
  }).catch((): Occurrence[] => []);

/** Parses an invitation and compares it with the calendar; null if unreadable. */
export const loadInvitation = async (
  blobId: BlobId,
  sender: InvitationSender,
  options: LoadOptions
): Promise<InvitationView | null> => {
  const client = await getClient();
  const accountId = client.primaryAccountId(CAPABILITIES.calendars);
  const [parsed] = await client.request((b) => [
    b.call("CalendarEvent/parse", { accountId, blobIds: [blobId] }),
  ]);
  const invitation = analyzeInvitation(
    parsed.parsed[blobId] ?? [],
    options.own
  );
  // An assigned task (VTODO) has its own card (components/tasks).
  if (invitation === null || isTask(invitation.event)) {
    return null;
  }
  const context = mapContext(
    options.zone,
    options.own,
    options.calendars,
    accountId
  );
  const placed = (copy: Parameters<typeof withStoredTiming>[1]) =>
    toOccurrence(
      { ...withStoredTiming(invitation.event, copy), id: PLACEHOLDER_ID },
      undefined,
      context
    );
  // The days around the invitation's own time, asked while looking for
  // the stored copy: usually the copy has the same time.
  const guess = placed(undefined);
  const guessWindow = guess === null ? null : daysOf(guess, options.zone);
  const guessAround =
    guessWindow === null ? null : eventsAround(guessWindow, options);
  const copies = await findByUid(invitation.uid);
  const [first] = copies;
  const occurrence = placed(first);
  if (occurrence === null) {
    return null;
  }
  const storedOrganizer =
    first === undefined ? null : addressOf(first.organizerCalendarAddress);
  const stored: StoredCopy | null =
    first === undefined
      ? null
      : {
          cancelled: first.status?.toLowerCase() === "cancelled",
          duration: first.duration,
          id: first.id,
          organizer: storedOrganizer,
          organizerIsMe:
            storedOrganizer !== null &&
            ownSet(options.own).has(storedOrganizer),
          participants: first.participants ?? null,
          sequence: first.sequence,
          start: first.start,
          timeZone: first.timeZone,
        };
  const window = daysOf(occurrence, options.zone);
  // The stored copy has another time (the organizer moved it, and this is
  // an older email): ask again for its days. Conflicts are checked by
  // time below, so more days than needed are fine.
  const around =
    guessAround !== null &&
    guessWindow !== null &&
    guessWindow.start.compare(window.start) <= 0 &&
    guessWindow.end.compare(window.end) >= 0
      ? await guessAround
      : await eventsAround(window, options);
  const visible = new Set(
    options.calendars
      .filter((calendar) => calendar.visible)
      .map((calendar) => calendar.key)
  );
  const conflicts = around.filter(
    (other) =>
      other.baseId !== stored?.id &&
      other.status !== "cancelled" &&
      other.myStatus !== "declined" &&
      (other.calendarKey === null || visible.has(other.calendarKey)) &&
      other.start < occurrence.end &&
      other.end > occurrence.start &&
      !(other.allDay && !occurrence.allDay)
  );
  return {
    action: cardAction(invitation, stored, sender),
    blobId,
    conflicts,
    invitation,
    myStatus: storedStatus(stored, invitation.me),
    occurrence,
    outdated: stored !== null && stored.sequence > invitation.sequence,
    sender,
    senderBlocked: senderBlocked(invitation, stored, sender),
    stored,
  };
};

export type Answer = Extract<
  ParticipationStatus,
  "accepted" | "tentative" | "declined"
>;

export interface ReplyTexts {
  /** "Accepted: Planning" in the user's language. */
  readonly subject: string;
  /** Short plain-text body. */
  readonly text: string;
}

/**
 * Whether the bridge sends the scheduling messages the app would (its
 * vendor capability): one occurrence's REQUEST, COUNTER, DECLINECOUNTER.
 */
const bridgeSchedules = (client: JmapClient): boolean =>
  hasCapability(client.session, CAPABILITIES.bridge);

/** "2026-10-20T08:00:00Z" (UTCDate, RFC 8620 §1.4: no fraction). */
const utcDate = (ms: number): string =>
  new Date(ms).toISOString().replace(/\.\d{3}Z$/u, "Z");

/** The note as sent: trimmed, at most MAX_COMMENT (empty: none). */
const cleanComment = (comment: string | undefined): string | undefined => {
  const note = comment?.trim().slice(0, MAX_COMMENT) ?? "";
  return note === "" ? undefined : note;
};

/**
 * A COUNTER by the bridge (`CalendarEvent/counter`): of a stored event or
 * of the invitation email's part. It builds the message from them and
 * mails it in the user's language; texts here are not used.
 */
const bridgeCounter = async (
  client: JmapClient,
  subject:
    | {
        readonly eventId: CalendarEventId;
        readonly accountId: AccountId | null;
      }
    | { readonly blobId: BlobId },
  proposal: Proposal,
  comment: string | undefined
): Promise<void> => {
  const note = cleanComment(comment);
  const common = {
    utcEnd: utcDate(Math.max(proposal.start, proposal.end)),
    utcStart: utcDate(proposal.start),
    ...(note === undefined ? {} : { comment: note }),
  };
  await client.request((b) => {
    b.using(CAPABILITIES.bridge);
    return [
      "eventId" in subject
        ? b.call("CalendarEvent/counter", {
            ...common,
            accountId:
              subject.accountId ??
              client.primaryAccountId(CAPABILITIES.calendars),
            eventId: subject.eventId,
          })
        : b.call("CalendarEvent/counter", {
            ...common,
            accountId: client.primaryAccountId(CAPABILITIES.calendars),
            blobId: subject.blobId,
          }),
    ] as const;
  });
};

export interface ReplyTarget {
  readonly uid: string;
  readonly sequence: number;
  readonly organizer: Person;
  readonly attendee: Person;
  readonly title: string;
  readonly recurrenceId: string | null;
  readonly recurrenceIdTimeZone: string | null;
  /** An all-day event: its RECURRENCE-ID is a DATE. */
  readonly allDay: boolean;
}

/** What else goes with an answer: a note and/or another time. */
export interface ReplyExtras {
  /** Note for the organizer (COMMENT, and in the email's text). */
  readonly comment?: string;
}

/** A proposed time (COUNTER), epoch ms [start, end). */
export interface Proposal {
  readonly start: number;
  readonly end: number;
}

/**
 * Emails an iTIP message (iMIP: text/plain + text/calendar with its
 * method) from the identity with `from`'s address to `to`.
 *
 * Stalwart quirk: a body part's `type` may carry parameters
 * ("text/calendar; method=REPLY"), which is how the `method` parameter gets
 * in; a separate Content-Type header would be sent twice.
 */
export const sendItip = async (message: {
  readonly method: "REPLY" | "COUNTER" | "DECLINECOUNTER" | "REQUEST";
  readonly from: Person;
  readonly to: Person | readonly Person[];
  readonly ics: string;
  readonly texts: ReplyTexts;
}): Promise<void> => {
  const client = await getClient();
  const accountId = client.primaryAccountId();
  // Who sends it and where the copy goes, in one request (RFC 8620 §3.3:
  // several method calls).
  const [identities, mailboxes] = await client.request((b) => [
    b.call("Identity/get", {
      accountId: client.primaryAccountId(CAPABILITIES.submission),
      properties: ["email", "name"],
    }),
    b.call("Mailbox/get", { accountId, properties: ["role"] }),
  ]);
  // Only from the very address the message speaks for: the recipient
  // matches the From with the ATTENDEE (REPLY, COUNTER) or the ORGANIZER
  // (REQUEST, DECLINECOUNTER) and ignores anything else (RFC 6047 §3,
  // RFC 5546 §6.1). Another identity would send a message nobody applies.
  const from = message.from.email.toLowerCase();
  const identity = identities.list.find(
    (item) => item.email.toLowerCase() === from
  );
  if (identity === undefined) {
    throw new NoSendingIdentityError(message.from.email);
  }
  const sent: MailboxId | null =
    mailboxes.list.find((mailbox) => mailbox.role === "sent")?.id ?? null;
  const { ics, texts, to } = message;
  const [, submission] = await client.request((b) => {
    const email = b.call("Email/set", {
      accountId,
      create: {
        reply: {
          bodyStructure: {
            subParts: [
              { partId: "text", type: "text/plain" },
              {
                partId: "ics",
                type: `text/calendar; method=${message.method}`,
              },
            ],
            type: "multipart/alternative",
          },
          bodyValues: { ics: { value: ics }, text: { value: texts.text } },
          from: [{ email: identity.email, name: identity.name || null }],
          keywords: { $seen: true },
          mailboxIds: sent === null ? {} : { [sent]: true },
          subject: texts.subject,
          to: (Array.isArray(to) ? to : [to]).map((person: Person) => ({
            email: person.email,
            name: person.name,
          })),
        },
      },
    });
    const submit = b.call("EmailSubmission/set", {
      accountId,
      create: { send: { emailId: "#reply", identityId: identity.id } },
    });
    return [email, submit] as const;
  });
  if (submission.created.send === undefined) {
    throw new Error(
      submission.notCreated.send?.description ?? "The reply was not sent"
    );
  }
};

/** The note, as the last lines of the email's text. */
const withComment = (texts: ReplyTexts, comment: string | undefined) => {
  const note = comment?.trim().slice(0, MAX_COMMENT) ?? "";
  return note === "" ? texts : { ...texts, text: `${texts.text}\n\n${note}` };
};

/** Emails a REPLY (RFC 5546 §3.2.3) to the organizer, with an optional note. */
export const sendReply = async (
  target: ReplyTarget,
  status: Answer,
  texts: ReplyTexts,
  extras: ReplyExtras = {}
): Promise<void> => {
  const { comment } = extras;
  await sendItip({
    from: target.attendee,
    ics: buildReply({
      allDay: target.allDay,
      attendee: target.attendee,
      now: Date.now(),
      organizer: target.organizer,
      recurrenceId: target.recurrenceId,
      recurrenceIdTimeZone: target.recurrenceIdTimeZone,
      sequence: target.sequence,
      status,
      title: target.title,
      uid: target.uid,
      ...(comment === undefined ? {} : { comment }),
    }),
    method: "REPLY",
    texts: withComment(texts, comment),
    to: target.organizer,
  });
};

/**
 * Proposes another time to the organizer (COUNTER, RFC 5546 §3.2.7). The
 * event in the user's calendar doesn't change: the organizer decides.
 * Stalwart doesn't process COUNTER itself (it answers "unsupported
 * method"), so it arrives as an email the organizer's app shows.
 */
export const sendCounter = async (
  target: ReplyTarget,
  proposal: Proposal,
  status: ParticipationStatus,
  texts: ReplyTexts,
  extras: ReplyExtras = {}
): Promise<void> => {
  const { comment } = extras;
  await sendItip({
    from: target.attendee,
    ics: buildCounter({
      allDay: target.allDay,
      attendee: target.attendee,
      end: proposal.end,
      now: Date.now(),
      organizer: target.organizer,
      recurrenceId: target.recurrenceId,
      recurrenceIdTimeZone: target.recurrenceIdTimeZone,
      sequence: target.sequence,
      start: proposal.start,
      status,
      title: target.title,
      uid: target.uid,
      ...(comment === undefined ? {} : { comment }),
    }),
    method: "COUNTER",
    texts: withComment(texts, comment),
    to: target.organizer,
  });
};
const myKeyIn = (
  participants: Readonly<Record<string, Participant>> | null,
  email: string
): string | null => {
  for (const [key, participant] of Object.entries(participants ?? {})) {
    if (
      (addressOf(participant.calendarAddress) ??
        addressOf(participant.email)) === email
    ) {
      return key;
    }
  }
  return null;
};

/** The occurrence of a stored series (own account) that starts at `start`. */
const occurrenceAt = async (
  baseId: CalendarEventId,
  start: number,
  options: LoadOptions
): Promise<CalendarEventId | null> => {
  const day = dateInZone(start, options.zone);
  const own = ownAccountOf(options.calendars);
  const around = await listOccurrences({
    calendars: options.calendars,
    end: day.add({ days: 2 }),
    own: options.own,
    start: day.subtract({ days: 1 }),
    zone: options.zone,
  });
  return (
    around.find(
      (occurrence) =>
        occurrence.baseId === baseId &&
        (own === null || occurrence.accountId === own) &&
        Math.abs(occurrence.start - start) < 60_000
    )?.id ?? null
  );
};

/**
 * The start to write with a change to one occurrence, in the event's zone:
 * every override carries its own, so the instance a server writes to
 * CalDAV always has DTSTART (strict clients like DAVx5 refuse a calendar
 * without it; support.stalw.art/t/2007).
 */
const occurrenceStart = (
  start: number,
  stored: { readonly timeZone: string | null },
  options: LoadOptions
): string =>
  formatLocalDateTime(
    toCalendarDateTime(fromAbsolute(start, stored.timeZone ?? options.zone))
  );

/** The id of one occurrence of a stored series (to answer for just it). */
const occurrenceId = async (
  view: InvitationView,
  options: LoadOptions
): Promise<CalendarEventId | null> => {
  const { recurrenceId } = view.invitation;
  if (recurrenceId === null || view.stored === null) {
    return view.stored?.id ?? null;
  }
  return await occurrenceAt(view.stored.id, view.occurrence.start, options);
};

/**
 * Who gets the REPLY: with a copy in the calendar, its stored organizer
 * (never just whoever the .ics names); else the invitation's.
 */
export const replyRecipient = (view: InvitationView): Person | null => {
  const { organizer } = view.invitation;
  if (view.stored === null) {
    return organizer;
  }
  const email = view.stored.organizer;
  if (email === null) {
    return null;
  }
  return { email, name: organizer?.email === email ? organizer.name : null };
};

/**
 * Accept, maybe or decline: updates (or, for accept and maybe, adds) the
 * event, then emails the REPLY. Declining an invitation that isn't in the
 * calendar only sends the reply.
 */
export const answerInvitation = async (
  view: InvitationView,
  status: Answer,
  texts: ReplyTexts,
  options: LoadOptions & { readonly calendarId: CalendarId | null },
  extras: ReplyExtras = {}
): Promise<void> => {
  const { invitation } = view;
  const { me } = invitation;
  const organizer = replyRecipient(view);
  if (view.action !== "rsvp" || me === null || organizer === null) {
    throw new Error("Not an invitation to this user");
  }
  if (view.stored === null) {
    if (status !== "declined") {
      if (options.calendarId === null) {
        throw new Error("No calendar to add the event to");
      }
      await createEvent(
        {
          ...eventToImport(invitation.event, me, status),
          calendarIds: { [options.calendarId]: true },
        },
        false
      );
    }
  } else {
    const key = myKeyIn(view.stored.participants, me.email);
    const id = await occurrenceId(view, options);
    if (key !== null && id !== null) {
      await updateEvent(
        id,
        {
          [`participants/${key}/participationStatus`]: status,
          ...(id === view.stored.id
            ? {}
            : {
                start: occurrenceStart(
                  view.occurrence.start,
                  view.stored,
                  options
                ),
              }),
        },
        false
      );
    }
  }
  if (organizer.email !== me.email) {
    await sendReply(
      {
        allDay: invitation.event.showWithoutTime === true,
        attendee: { email: me.email, name: me.name },
        organizer,
        recurrenceId: invitation.recurrenceId,
        recurrenceIdTimeZone: invitation.recurrenceIdTimeZone,
        sequence: invitation.sequence,
        title: invitation.event.title ?? "",
        uid: invitation.uid,
      },
      status,
      texts,
      extras
    );
  }
};

/** The occurrence a message is about is no longer in the calendar. */
export class OccurrenceGoneError extends Error {
  override readonly name = "OccurrenceGoneError";
}

/**
 * A cancelled invitation: removes it from the calendar (sends nothing).
 * A cancelled occurrence removes only that one (destroying an
 * occurrence's id excludes it from its series).
 */
export const removeCancelled = async (
  view: InvitationView,
  options: LoadOptions
): Promise<void> => {
  if (view.stored === null || view.action !== "remove") {
    return;
  }
  const id = await occurrenceId(view, options);
  if (id === null) {
    throw new OccurrenceGoneError(
      "The occurrence is no longer in the calendar"
    );
  }
  await deleteEvent(id, false);
};

/** Adds an event the user wasn't invited to by name (no reply). */
export const addInvitation = async (
  view: InvitationView,
  calendarId: CalendarId
): Promise<void> => {
  await createEvent(
    {
      ...eventToImport(view.invitation.event, null, "needs-action"),
      calendarIds: { [calendarId]: true },
    },
    false
  );
};

/**
 * A REPLY to the user's event: stores the answer of the guest who sent it
 * (only theirs, whatever else the .ics says), for the one occurrence it
 * is about if it names one.
 */
export const applyReply = async (
  view: InvitationView,
  options: LoadOptions
): Promise<void> => {
  const { stored } = view;
  const author = replyAuthor(view.invitation, stored, view.sender);
  if (stored === null || author === null || view.action !== "apply-reply") {
    return;
  }
  const key = myKeyIn(stored.participants, author.email);
  if (key === null) {
    return;
  }
  const id = await occurrenceId(view, options);
  if (id === null) {
    throw new OccurrenceGoneError(
      "The occurrence is no longer in the calendar"
    );
  }
  // With the SEQUENCE and DTSTAMP of this response, so an older one that
  // arrives later isn't applied over it (RFC 5546 §2.1.5, `replyAction`).
  const stamp = responseStampOf(responseStamp(view.invitation.event));
  await updateEvent(
    id,
    {
      [`participants/${key}/participationStatus`]: author.status,
      ...Object.fromEntries(
        Object.entries(stamp).map(([name, value]) => [
          `participants/${key}/${name}`,
          value,
        ])
      ),
      // One occurrence: with its start (see `occurrenceStart`).
      ...(id === stored.id
        ? {}
        : { start: occurrenceStart(view.occurrence.start, stored, options) }),
    },
    false
  );
};

/** The REPLY/COUNTER target of an event the user is invited to. */
const eventTarget = (detail: EventDetail): ReplyTarget | null => {
  const mine = detail.participants.find(
    (participant) => participant.isMe && !participant.isOrganizer
  );
  const organizer = detail.participants.find(
    (participant) => participant.isOrganizer
  );
  if (mine === undefined || organizer === undefined || detail.uid === null) {
    return null;
  }
  return {
    allDay: detail.allDay,
    attendee: { email: mine.email, name: mine.name },
    organizer: { email: organizer.email, name: organizer.name },
    recurrenceId: detail.isOccurrence ? detail.recurrenceId : null,
    recurrenceIdTimeZone: detail.timeZone,
    sequence: detail.sequence,
    title: detail.title,
    uid: detail.uid,
  };
};

/** Answering from the calendar itself (event details). */
export const answerEvent = async (
  detail: EventDetail,
  status: Answer,
  texts: ReplyTexts,
  extras: ReplyExtras = {}
): Promise<void> => {
  const mine = detail.participants.find(
    (participant) => participant.isMe && !participant.isOrganizer
  );
  if (mine === undefined) {
    throw new Error("Not invited to this event");
  }
  await updateEvent(
    detail.id,
    {
      [`participants/${mine.key}/participationStatus`]: status,
      // One occurrence: with its start (see `occurrenceStart`).
      ...(detail.isOccurrence ? { start: detail.localStart } : {}),
    },
    false,
    detail.accountId
  );
  const target = eventTarget(detail);
  if (target !== null) {
    await sendReply(target, status, texts, extras);
  }
};

/** Proposing another time from the calendar (event details). */
export const proposeForEvent = async (
  detail: EventDetail,
  proposal: Proposal,
  texts: ReplyTexts,
  extras: ReplyExtras = {}
): Promise<void> => {
  const target = eventTarget(detail);
  if (target === null) {
    throw new Error("Not invited to this event");
  }
  const client = await getClient();
  if (bridgeSchedules(client)) {
    await bridgeCounter(
      client,
      { accountId: detail.accountId, eventId: detail.id },
      proposal,
      extras.comment
    );
    return;
  }
  await sendCounter(
    target,
    proposal,
    detail.myStatus ?? "needs-action",
    texts,
    extras
  );
};

/** Proposing another time from an invitation email. */
export const proposeForInvitation = async (
  view: InvitationView,
  proposal: Proposal,
  texts: ReplyTexts,
  extras: ReplyExtras = {}
): Promise<void> => {
  const { me, organizer } = view.invitation;
  if (me === null || organizer === null) {
    throw new Error("Not an invitation to this user");
  }
  const client = await getClient();
  if (bridgeSchedules(client)) {
    await bridgeCounter(
      client,
      { blobId: view.blobId },
      proposal,
      extras.comment
    );
    return;
  }
  await sendCounter(
    {
      allDay: view.invitation.event.showWithoutTime === true,
      attendee: { email: me.email, name: me.name },
      organizer,
      recurrenceId: view.invitation.recurrenceId,
      recurrenceIdTimeZone: view.invitation.recurrenceIdTimeZone,
      sequence: view.invitation.sequence,
      title: view.invitation.event.title ?? "",
      uid: view.invitation.uid,
    },
    proposal,
    view.myStatus ?? "needs-action",
    texts,
    extras
  );
};

/** The guest who proposes in a COUNTER: the one who sent it, if invited. */
export const counterAuthor = (
  view: InvitationView
): InvitationAttendee | null =>
  replyAuthor(view.invitation, view.stored, view.sender);

/**
 * The organizer accepts a proposed time: the stored event (or the one
 * occurrence the COUNTER is about) moves there, and the server sends the
 * update to every guest (`sendSchedulingMessages`), as for any change
 * (RFC 5546 §3.2.7: the organizer sends a REQUEST to the attendees).
 * Stalwart 1.0.1 and the bridge also do it for ONE occurrence: a REQUEST
 * with its RECURRENCE-ID and SEQUENCE raised (checked live,
 * `occurrence.live.test.ts`). The new start is written in the event's own
 * zone.
 */
export const acceptCounter = async (
  view: InvitationView,
  options: LoadOptions
): Promise<void> => {
  const { stored } = view;
  if (
    stored === null ||
    view.action !== "counter" ||
    counterAuthor(view) === null
  ) {
    throw new Error("Not a proposal for the user's event");
  }
  const { end, start } = view.occurrence;
  const zone = stored.timeZone ?? options.zone;
  const patch: Record<string, unknown> = {
    duration: formatDuration(
      0,
      Math.max(0, Math.round((end - start) / 60_000))
    ),
    start: formatLocalDateTime(toCalendarDateTime(fromAbsolute(start, zone))),
  };
  let id: CalendarEventId | null = stored.id;
  const { recurrenceId, recurrenceIdTimeZone } = view.invitation;
  if (recurrenceId !== null) {
    const original = parseLocalDateTime(recurrenceId);
    id =
      original === null
        ? null
        : await occurrenceAt(
            stored.id,
            toZoned(original, recurrenceIdTimeZone ?? zone)
              .toDate()
              .getTime(),
            options
          );
  }
  if (id === null) {
    throw new Error("The occurrence is no longer in the calendar");
  }
  await updateEvent(id, patch, true);
};

/** The organizer keeps the time: a DECLINECOUNTER to the guest. */
export const declineCounter = async (
  view: InvitationView,
  texts: ReplyTexts,
  extras: ReplyExtras = {}
): Promise<void> => {
  const author = counterAuthor(view);
  // The user, as the stored event names them (not as the .ics does).
  const organizer = replyRecipient(view);
  if (view.action !== "counter" || author === null || organizer === null) {
    throw new Error("Not a proposal for the user's event");
  }
  const { comment } = extras;
  const client = await getClient();
  const { stored } = view;
  if (bridgeSchedules(client) && stored !== null) {
    const note = cleanComment(comment);
    await client.request((b) => {
      b.using(CAPABILITIES.bridge);
      return [
        b.call("CalendarEvent/declineCounter", {
          accountId: client.primaryAccountId(CAPABILITIES.calendars),
          blobId: view.blobId,
          eventId: stored.id,
          ...(note === undefined ? {} : { comment: note }),
        }),
      ] as const;
    });
    return;
  }
  await sendItip({
    from: organizer,
    ics: buildDeclineCounter({
      allDay: view.invitation.event.showWithoutTime === true,
      attendee: { email: author.email, name: author.name },
      now: Date.now(),
      organizer,
      recurrenceId: view.invitation.recurrenceId,
      recurrenceIdTimeZone: view.invitation.recurrenceIdTimeZone,
      sequence: view.stored?.sequence ?? view.invitation.sequence,
      uid: view.invitation.uid,
      ...(comment === undefined ? {} : { comment }),
    }),
    method: "DECLINECOUNTER",
    texts: withComment(texts, comment),
    to: { email: author.email, name: author.name },
  });
};
