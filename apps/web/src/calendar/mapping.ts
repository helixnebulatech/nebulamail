// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// JMAP CalendarEvent → view types (model.ts). Pure, with tests.
//
// The bridge's occurrences from an expanded query drop `showWithoutTime`
// and come with the query's time zone (Stalwart keeps both), so whether an
// event is all-day (and floating) is read from the stored event (`base`).

import type {
  AccountId,
  CalendarEvent,
  CalendarEventId,
  CalendarId,
  Lenient,
  Participant,
} from "@nebulamail/jmap-client";

import { addressOf, ownSet } from "./itip";
import type {
  CalendarSummary,
  EventAlert,
  EventDetail,
  EventParticipant,
  Occurrence,
  ParticipationStatus,
} from "./model";
import { calendarKey, toEventStatus, toParticipationStatus } from "./model";
import {
  MINUTE_MS,
  dayRange,
  durationMinutes,
  parseDuration,
  startOfDayMs,
  timedRange,
} from "./time";

type Event = Lenient<CalendarEvent> & { readonly id: CalendarEventId };

const firstCalendar = (
  calendarIds: Readonly<Record<string, boolean>> | null | undefined
): CalendarId | null => {
  const id = Object.entries(calendarIds ?? {}).find(([, on]) => on)?.[0];
  // Ids come from the server's own response, already checked by the client.
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion
  return id === undefined ? null : (id as CalendarId);
};

const firstLocation = (event: Event): string | null => {
  for (const location of Object.values(event.locations ?? {})) {
    const name = location.name?.trim();
    if (name !== undefined && name !== "") {
      return name;
    }
  }
  return null;
};

const participantEmail = (participant: Participant): string | null =>
  addressOf(participant.calendarAddress) ?? addressOf(participant.email);

/** The user's status among the participants (null: not one of them). */
export const myStatusIn = (
  participants: Readonly<Record<string, Participant>> | null | undefined,
  own: ReadonlySet<string>,
  organizer: string | null
): ParticipationStatus | null => {
  let found: ParticipationStatus | null = null;
  for (const participant of Object.values(participants ?? {})) {
    const email = participantEmail(participant);
    if (email !== null && own.has(email)) {
      const status = toParticipationStatus(participant.participationStatus);
      // As organizer the user's own entry is "accepted"; keep looking for
      // an attendee entry first.
      if (email !== organizer) {
        return status;
      }
      found = status;
    }
  }
  return found;
};

const isOrganizerOf = (event: Event, own: ReadonlySet<string>): boolean => {
  const organizer = addressOf(event.organizerCalendarAddress);
  if (organizer !== null) {
    return own.has(organizer);
  }
  return event.isOrigin !== false;
};

export interface MapContext {
  /** The user's time zone (display). */
  readonly zone: string;
  readonly own: ReadonlySet<string>;
  /** By `calendarKey`. */
  readonly calendars: ReadonlyMap<string, CalendarSummary>;
  /** The account the events come from. */
  readonly accountId: AccountId;
}

export const fallbackColor = "#4F46E5";

const emptyToNull = (text: string | undefined): string | null =>
  text === undefined || text === "" ? null : text;

/** One occurrence for the views; null if its times can't be read. */
interface Times {
  readonly start: number;
  readonly end: number;
  readonly firstDay: string | null;
  readonly endDay: string | null;
}

const allDayTimes = (event: Event, zone: string): Times | null => {
  const days = dayRange({ duration: event.duration, start: event.start });
  return days === null
    ? null
    : {
        end: startOfDayMs(days.end, zone),
        endDay: days.end.toString(),
        firstDay: days.first.toString(),
        start: startOfDayMs(days.first, zone),
      };
};

const utcOf = (value: string | null | undefined): number =>
  typeof value === "string" ? Date.parse(value) : Number.NaN;

const timedTimes = (
  event: Event,
  series: Event,
  zone: string
): Times | null => {
  const utcStart = utcOf(event.utcStart);
  const utcEnd = utcOf(event.utcEnd);
  if (Number.isFinite(utcStart) && Number.isFinite(utcEnd)) {
    return {
      end: Math.max(utcStart, utcEnd),
      endDay: null,
      firstDay: null,
      start: utcStart,
    };
  }
  const range = timedRange(
    {
      duration: event.duration,
      start: event.start,
      // A floating series: the occurrence's zone is the query's.
      timeZone: series.timeZone ?? null,
    },
    zone
  );
  return range === null ? null : { ...range, endDay: null, firstDay: null };
};

/**
 * Whether a calendar object is an event. Tasks (`@type: "Task"`, a VTODO
 * written by another client or by our Tasks app) and anything else that
 * isn't an Event share the calendars and the queries (Stalwart gives a
 * task with a date `utcStart`/`utcEnd`): the
 * calendar views leave them out. No `@type` is read as an Event.
 */
export const isEventObject = (
  event: Pick<Event, "@type">,
  base?: Pick<Event, "@type">
): boolean => {
  const type = event["@type"] ?? base?.["@type"];
  return type === undefined || type === null || type === "Event";
};

export const toOccurrence = (
  event: Event,
  base: Event | undefined,
  context: MapContext
): Occurrence | null => {
  const series = base ?? event;
  if (!isEventObject(event, series)) {
    return null;
  }
  const allDay = series.showWithoutTime === true;
  const times = allDay
    ? allDayTimes(event, context.zone)
    : timedTimes(event, series, context.zone);
  if (times === null) {
    return null;
  }
  const calendarId = firstCalendar(event.calendarIds ?? series.calendarIds);
  const key =
    calendarId === null ? null : calendarKey(context.accountId, calendarId);
  const calendar = key === null ? undefined : context.calendars.get(key);
  const participants = event.participants ?? series.participants;
  const organizer = addressOf(
    event.organizerCalendarAddress ?? series.organizerCalendarAddress
  );
  const { end, endDay, firstDay, start } = times;
  return {
    accountId: context.accountId,
    allDay,
    baseId: event.baseEventId ?? series.id,
    calendarId,
    calendarKey: key,
    color: calendar?.color ?? fallbackColor,
    end,
    endDay,
    firstDay,
    hasParticipants: Object.keys(participants ?? {}).length > 1,
    id: event.id,
    isOrganizer: isOrganizerOf({ ...series, ...event }, context.own),
    location: firstLocation(event) ?? firstLocation(series),
    myStatus: myStatusIn(participants, context.own, organizer),
    recurring:
      series.recurrenceRule !== null && series.recurrenceRule !== undefined,
    start,
    status: toEventStatus(event.status ?? series.status),
    title: event.title ?? series.title ?? "",
  };
};

const toParticipants = (
  participants: Readonly<Record<string, Participant>> | null | undefined,
  own: ReadonlySet<string>,
  organizer: string | null
): EventParticipant[] =>
  Object.entries(participants ?? {}).flatMap(([key, participant]) => {
    const email = participantEmail(participant);
    if (email === null) {
      return [];
    }
    const name = participant.name?.trim();
    return [
      {
        email,
        isMe: own.has(email),
        isOrganizer:
          email === organizer ||
          (organizer === null && participant.roles?.owner === true),
        key,
        name: name === undefined || name === "" ? null : name,
        optional: participant.roles?.optional === true,
        status: toParticipationStatus(participant.participationStatus),
      },
    ];
  });

const toAlerts = (event: Event): EventAlert[] =>
  Object.entries(event.alerts ?? {}).flatMap(([key, alert]) => {
    const { offset } = alert.trigger;
    if (
      offset === undefined ||
      (alert.trigger.relativeTo !== undefined &&
        alert.trigger.relativeTo !== "start")
    ) {
      return [];
    }
    const duration = parseDuration(offset);
    return duration === null
      ? []
      : [{ key, minutesBefore: -durationMinutes(duration) }];
  });

const minutesOf = (value: string | null | undefined): number => {
  const duration = parseDuration(value);
  return duration === null ? 0 : Math.max(0, durationMinutes(duration));
};

const linksOf = (event: Event, series: Event): EventDetail["links"] =>
  Object.values(event.virtualLocations ?? series.virtualLocations ?? {}).map(
    (location) => ({
      name: emptyToNull(location.name?.trim()),
      uri: location.uri,
    })
  );

const describe = (
  event: Event,
  series: Event
): Pick<EventDetail, "description" | "descriptionIsHtml"> => {
  const description = event.description ?? series.description ?? "";
  const contentType =
    event.descriptionContentType ?? series.descriptionContentType ?? "";
  return {
    description: description === "" ? null : description,
    descriptionIsHtml: contentType.toLowerCase().startsWith("text/html"),
  };
};

/** The details of one event or occurrence; `base` is its series. */
export const toDetail = (
  event: Event,
  base: Event | undefined,
  context: MapContext
): EventDetail | null => {
  const series = base ?? event;
  const occurrence = toOccurrence(event, series, context);
  if (occurrence === null || typeof event.start !== "string") {
    return null;
  }
  const organizer = addressOf(
    event.organizerCalendarAddress ?? series.organizerCalendarAddress
  );
  return {
    ...describe(event, series),
    accountId: occurrence.accountId,
    alerts: toAlerts(event.alerts === undefined ? series : event),
    allDay: occurrence.allDay,
    baseId: occurrence.baseId,
    calendarId: occurrence.calendarId,
    calendarKey: occurrence.calendarKey,
    color: occurrence.color,
    durationDays: occurrence.allDay
      ? Math.max(
          1,
          Math.round((occurrence.end - occurrence.start) / (1440 * MINUTE_MS))
        )
      : 0,
    durationMinutes: minutesOf(event.duration ?? series.duration),
    end: occurrence.end,
    endDay: occurrence.endDay,
    firstDay: occurrence.firstDay,
    id: event.id,
    isOccurrence: occurrence.recurring && event.id !== occurrence.baseId,
    isOrganizer: occurrence.isOrganizer,
    links: linksOf(event, series),
    localStart: event.start,
    location: occurrence.location,
    myStatus: occurrence.myStatus,
    participants: toParticipants(
      event.participants ?? series.participants,
      context.own,
      organizer
    ),
    recurrenceId: event.recurrenceId ?? null,
    recurring: occurrence.recurring,
    sequence: Math.max(0, Math.trunc(event.sequence ?? series.sequence ?? 0)),
    start: occurrence.start,
    status: occurrence.status,
    timeZone: occurrence.allDay ? null : (series.timeZone ?? null),
    title: occurrence.title,
    uid: series.uid ?? null,
  };
};

export const mapContext = (
  zone: string,
  own: readonly string[],
  calendars: readonly CalendarSummary[],
  accountId: AccountId
): MapContext => ({
  accountId,
  calendars: new Map(calendars.map((calendar) => [calendar.key, calendar])),
  own: ownSet(own),
  zone,
});
