// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// View types of the calendar (the UI never sees JMAP). Event
// text (title, location, names) comes from other people: show it with
// `cleanDisplayText` inside `<bdi>`, never as HTML.

import type {
  AccountId,
  Alert,
  CalendarEventId,
  CalendarId,
} from "@nebulamail/jmap-client";

export type ParticipationStatus =
  | "needs-action"
  | "accepted"
  | "declined"
  | "tentative"
  | "delegated";

export type EventStatus = "confirmed" | "tentative" | "cancelled";

export interface CalendarSummary {
  readonly id: CalendarId;
  /** The JMAP account it lives in (calendars shared by others: theirs). */
  readonly accountId: AccountId;
  /** Unique across accounts (ids repeat between accounts): `calendarKey`. */
  readonly key: string;
  /** Who shares it (the account's name); null for the user's own. */
  readonly owner: string | null;
  /** The calendar itself may be deleted (own, or `mayDelete`). */
  readonly mayDelete: boolean;
  readonly name: string;
  /** CSS colour (validated) or a generated one. */
  readonly color: string;
  readonly visible: boolean;
  readonly isDefault: boolean;
  readonly sortOrder: number;
  readonly mayWrite: boolean;
  /** It may be shared with others (`myRights.mayShare`); absent: no. */
  readonly mayShare?: boolean;
  /**
   * What it holds ("VEVENT", "VTODO"), when the server says (the bridge's
   * `BRIDGE_COMPONENTS`); null or absent: anything (Stalwart).
   */
  readonly components?: readonly string[] | null;
  /** IANA zone of the calendar (`Calendar.timeZone`); null/absent: none. */
  readonly timeZone?: string | null;
  /**
   * Reminders for new events with a time (`defaultAlertsWithTime`); null
   * or absent: the server doesn't say (`calendar/defaults.ts`).
   */
  readonly defaultAlertsWithTime?: DefaultAlerts | null;
  /** The same for all-day events (`defaultAlertsWithoutTime`). */
  readonly defaultAlertsWithoutTime?: DefaultAlerts | null;
  /**
   * Published at a public link (Nextcloud through the bridge)
   * null or absent: it can't be published here.
   */
  readonly publishing?: CalendarPublishing | null;
  /**
   * A webcal subscription (read-only; the server fetches it): where it
   * comes from (null: unknown). Absent or null: not a subscription.
   */
  readonly subscription?: { readonly url: string | null } | null;
}

/** A calendar's public link, as the bridge says it. */
export interface CalendarPublishing {
  readonly published: boolean;
  /** Only `http(s)`; null while not published. Only shown, never opened. */
  readonly url: string | null;
}

/** A calendar's default alerts as the app shows them. */
export interface DefaultAlerts {
  /** Display alerts relative to the start: minutes before it. */
  readonly minutes: readonly number[];
  /**
   * The ones the app can't show (email, relative to the end…), by id:
   * written back as they are.
   */
  readonly others: Readonly<Record<string, Alert>>;
}

/** The bridge's Calendar property with the components it holds. */
export const BRIDGE_COMPONENTS =
  "https://helixnebula.tech/jmap/bridge/components";

/** Whether a calendar takes events (a bridge task list doesn't). */
export const holdsEvents = (
  calendar: Pick<CalendarSummary, "components">
): boolean =>
  calendar.components === null ||
  calendar.components === undefined ||
  calendar.components.includes("VEVENT");

/** One occurrence on screen (a non-repeating event is one occurrence). */
export interface Occurrence {
  /** The occurrence's id (edits only this one). */
  readonly id: CalendarEventId;
  /** The stored event (edits the whole series). */
  readonly baseId: CalendarEventId;
  readonly accountId: AccountId;
  readonly calendarId: CalendarId | null;
  /** `calendarKey` of its calendar (null: none). */
  readonly calendarKey: string | null;
  readonly title: string;
  readonly allDay: boolean;
  /** Epoch ms, [start, end). For all-day events, midnights in the user's zone. */
  readonly start: number;
  readonly end: number;
  /** All-day only: "YYYY-MM-DD" of the first day and the day after the last. */
  readonly firstDay: string | null;
  readonly endDay: string | null;
  readonly location: string | null;
  readonly color: string;
  readonly status: EventStatus;
  /** The user's answer when invited (null: not a participant). */
  readonly myStatus: ParticipationStatus | null;
  readonly recurring: boolean;
  /** The user organizes it (or nobody does). */
  readonly isOrganizer: boolean;
  readonly hasParticipants: boolean;
}

export interface Person {
  readonly name: string | null;
  /** Lower-case address (from a `mailto:` calendar address). */
  readonly email: string;
}

export interface EventParticipant extends Person {
  /** Key in the event's participants map (kept when editing). */
  readonly key: string;
  readonly status: ParticipationStatus;
  readonly isOrganizer: boolean;
  readonly optional: boolean;
  readonly isMe: boolean;
}

export interface EventAlert {
  readonly key: string;
  /** Minutes before the start (negative: after). */
  readonly minutesBefore: number;
}

/** A calendar's unique key: account and id ("c/b"). */
export const calendarKey = (accountId: string, calendarId: string): string =>
  `${accountId}/${calendarId}`;

/** An event opened in the details panel or the editor. */
/** Our snooze alerts' keys start with this (calendar/alerts.ts). */
export const SNOOZE_PREFIX = "snz";

export interface EventDetail {
  readonly id: CalendarEventId;
  readonly baseId: CalendarEventId;
  readonly accountId: AccountId;
  readonly calendarId: CalendarId | null;
  readonly calendarKey: string | null;
  readonly uid: string | null;
  readonly title: string;
  readonly allDay: boolean;
  /** Wall time of the start in `timeZone` ("2026-10-26T10:00:00"). */
  readonly localStart: string;
  /** In minutes (timed) or days (all-day, `durationDays`). */
  readonly durationMinutes: number;
  readonly durationDays: number;
  /** IANA zone; null for floating events. */
  readonly timeZone: string | null;
  readonly start: number;
  readonly end: number;
  /** All-day only: "YYYY-MM-DD" of the first day and the day after the last. */
  readonly firstDay: string | null;
  readonly endDay: string | null;
  readonly location: string | null;
  /** Meeting links (`virtualLocations`); untrusted, open after confirming. */
  readonly links: readonly {
    readonly name: string | null;
    readonly uri: string;
  }[];
  readonly description: string | null;
  readonly descriptionIsHtml: boolean;
  readonly participants: readonly EventParticipant[];
  readonly alerts: readonly EventAlert[];
  readonly status: EventStatus;
  readonly recurring: boolean;
  /** This is one occurrence of a repeating event. */
  readonly isOccurrence: boolean;
  readonly recurrenceId: string | null;
  readonly isOrganizer: boolean;
  readonly myStatus: ParticipationStatus | null;
  readonly color: string;
  readonly sequence: number;
}

export const PARTICIPATION_STATUSES: readonly ParticipationStatus[] = [
  "needs-action",
  "accepted",
  "declined",
  "tentative",
  "delegated",
];

export const toParticipationStatus = (
  value: string | null | undefined
): ParticipationStatus => {
  const lower = value?.toLowerCase();
  return (
    PARTICIPATION_STATUSES.find((status) => status === lower) ?? "needs-action"
  );
};

export const toEventStatus = (
  value: string | null | undefined
): EventStatus => {
  const lower = value?.toLowerCase();
  return lower === "cancelled" || lower === "tentative" ? lower : "confirmed";
};
