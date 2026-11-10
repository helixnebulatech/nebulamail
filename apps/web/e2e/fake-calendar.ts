// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// JMAP for Calendars in the fake server (fake-jmap.ts), shaped like
// Stalwart answers: calendars, events of today, invitations (.ics parts)
// in the inbox, and every write recorded so a test can check what the app
// asked for. Times are UTC so the tests don't depend on the zone.
//
// Added 2026-10-26: a calendar another user ("b") shares,
// whose event ids repeat ours on purpose (everything must be per
// account); a repeating series whose occurrences only exist expanded
// (like Stalwart's synthetic ids) with an invitation to one occurrence; a
// COUNTER to one of our events; creating, recolouring and deleting
// calendars; and calendar alerts sent on the event source. Since
// 2026-10-28: a task in our calendar; tasks.e2e.ts
// adds more (`taskFixtures`). Like Stalwart: a query without
// `expandRecurrences` lists every stored object page by page (there is no
// filter by type); an expanded one gives each occurrence of a repeating
// task with its own `start`/`utcStart` but the series' `due`, and leaves
// out excluded ones. Like Stalwart 1.0 (2026-10-30): one-off events are
// expanded under their real id with `baseEventId: null`, a result
// reference skips the nulls, and `CalendarEvent/get` refuses
// `recurrenceOverrides` together with `utcStart`/`utcEnd`.

import { z } from "zod";

type Json = Record<string, unknown>;

export const CALENDAR_ID = "cal1";
/** The bridge's limit of scheduling recipients (`maxParticipantsPerEvent`). */
const MAX_RECIPIENTS = 20;
export const INVITATION_BLOB = "b-inv1-cal";
export const INVITATION_UID = "board-1@partner.example";
/** An invitation to one occurrence of WEEKLY_UID. */
export const OCCURRENCE_BLOB = "b-occ1-cal";
export const WEEKLY_UID = "weekly-1@partner.example";
export const SERIES_ID = "ser1";
export const OCCURRENCE_ID = "ser1-occ";
/** A COUNTER from Carol for PLANNING_UID (organized by us). */
export const COUNTER_BLOB = "b-ctr1-cal";
export const PLANNING_UID = "planning-1@example.com";
export const PLANNING_ID = "ev4";
/** Bob's account and the calendar he shares with us. */
export const SHARED_CALENDAR_ACCOUNT = "b";
export const SHARED_CALENDAR_ID = "team";
/** A task due today in our calendar (never shown as an event). */
export const TASK_ID = "task1";
export const TASK_TITLE = "Pay the invoice";
/** A task Carol assigns to Jane by email (iTIP VTODO REQUEST). */
export const TASK_ASSIGNMENT_BLOB = "b-tsk1-cal";
/** Carol proposes another due for a task Jane assigned her (COUNTER). */
export const TASK_COUNTER_BLOB = "b-tsk2-cal";
/** Bob passes Jane a weekly task Carol assigned him (a delegated series). */
export const TASK_SERIES_BLOB = "b-tsk3-cal";
export const TASK_SERIES_UID = "series-1@example.com";
export const TASK_COUNTER_UID = "brochure-print@example.com";
export const TASK_ASSIGNMENT_UID = "brochure-1@partner.example";

/** A method-level error (`["error", {type}]`) from a fake method. */
export class FakeMethodError extends Error {
  readonly type: string;

  constructor(type: string, description: string) {
    super(description);
    this.name = "FakeMethodError";
    this.type = type;
  }
}

interface FakeEvent extends Json {
  readonly id: string;
}

/** Participants other than the organizer. */
const guestsOf = (event: FakeEvent | undefined): number => {
  const participants = event?.participants;
  return typeof participants === "object" && participants !== null
    ? Math.max(0, Object.keys(participants).length - 1)
    : 0;
};

interface FakeCalendarRow {
  readonly id: string;
  name: string;
  color: string | null;
  isVisible: boolean;
  readonly isDefault: boolean;
  myRights: Json;
  /** Who it is shared with (principal id → rights); ours only. */
  shareWith?: Record<string, Json>;
  /** IANA zone (`Calendar.timeZone`); absent: null. */
  timeZone?: string | null;
  defaultAlertsWithTime?: Json;
  defaultAlertsWithoutTime?: Json;
  /** Nextcloud through the bridge: published at a public link. */
  published?: boolean;
  /** Nextcloud through the bridge: a webcal subscription's source. */
  subscription?: string;
}

/** Something in the fake Nextcloud trash bin. */
export interface FakeTrashItem {
  readonly id: string;
  readonly kind: "calendar" | "event";
  readonly name: string;
  readonly calendarId?: string;
}

export interface FakeCalendar {
  /** Our account's events (stored ones). */
  readonly events: FakeEvent[];
  /** Expanded occurrences of our series (synthetic ids, like Stalwart). */
  readonly occurrences: FakeEvent[];
  /** Bob's shared calendar's events (ids repeat ours). */
  readonly sharedEvents: FakeEvent[];
  readonly calendars: FakeCalendarRow[];
  readonly sharedCalendars: FakeCalendarRow[];
  /** Every `CalendarEvent/set` the app sent. */
  readonly sets: Json[];
  /** Every `Calendar/set` the app sent. */
  readonly calendarSets: Json[];
  /** Every `Email/set` create (e.g. iTIP replies). */
  readonly emails: Json[];
  /** Every `EmailSubmission/set`. */
  readonly submissions: Json[];
  /** CalendarAlert objects for the next event-source connection. */
  readonly alerts: Json[];
  /** Refuse every `Calendar.timeZone` (`invalidProperties`). */
  refuseZones?: boolean;
  /**
   * Act as the bridge: the session announces its capability and it sends
   * proposals itself (`CalendarEvent/counter`, `/declineCounter`).
   */
  bridge?: boolean;
  /** Every call to the bridge's proposal methods: [name, args]. */
  readonly proposals: [string, Json][];
  /**
   * Nextcloud through the bridge (with `bridge`): publishing, webcal
   * subscriptions and the trash bin.
   */
  nextcloud?: boolean;
  /** Nextcloud's trash bin. */
  readonly trash: FakeTrashItem[];
  /** Every `CalendarTrash/set`. */
  readonly trashSets: Json[];
}

const BRIDGE = "https://helixnebula.tech/jmap/bridge";
const IS_PUBLISHED = `${BRIDGE}/isPublished`;
const PUBLIC_URL = `${BRIDGE}/publicUrl`;
const SUBSCRIPTION_URL = `${BRIDGE}/subscriptionUrl`;
/** The patch key of `isPublished` (a JSON pointer). */
const IS_PUBLISHED_PATCH = IS_PUBLISHED.replaceAll("/", "~1");
/** A fake public link (never opened). */
export const PUBLIC_LINK =
  "https://cloud.example.com/remote.php/dav/public-calendars/k3y";

const READ_ONLY_RIGHTS = {
  mayDelete: true,
  mayReadFreeBusy: true,
  mayReadItems: true,
  mayRSVP: false,
  mayShare: false,
  mayUpdatePrivate: false,
  mayWriteAll: false,
  mayWriteOwn: false,
};

/** The bridge's Nextcloud properties of one of our calendars. */
const nextcloudProperties = (
  state: FakeCalendar,
  calendar: FakeCalendarRow,
  shared: boolean
): Json => {
  if (state.nextcloud !== true || shared) {
    return {};
  }
  if (calendar.subscription !== undefined) {
    return { [SUBSCRIPTION_URL]: calendar.subscription };
  }
  const published = calendar.published === true;
  return {
    [IS_PUBLISHED]: published,
    [PUBLIC_URL]: published ? PUBLIC_LINK : null,
  };
};

/** "YYYY-MM-DD" of today in UTC. */
export const todayUtc = (): string => new Date().toISOString().slice(0, 10);

/** "YYYY-MM-DD" of tomorrow in UTC. */
export const tomorrowUtc = (): string =>
  new Date(Date.now() + 86_400_000).toISOString().slice(0, 10);

const utc = (day: string, time: string, minutes: number) => {
  const start = Date.parse(`${day}T${time}:00Z`);
  return {
    utcEnd: new Date(start + minutes * 60_000)
      .toISOString()
      .replace(".000", ""),
    utcStart: new Date(start).toISOString().replace(".000", ""),
  };
};

const CAROL = {
  "@type": "Participant",
  calendarAddress: "mailto:carol@partner.example",
  name: "Carol",
  participationStatus: "accepted",
  roles: { attendee: true, owner: true },
};

const JANE_INVITED = {
  "@type": "Participant",
  calendarAddress: "mailto:jane@example.com",
  expectReply: true,
  name: "Jane",
  participationStatus: "needs-action",
};

const events = (): FakeEvent[] => {
  const day = todayUtc();
  return [
    {
      "@type": "Event",
      alerts: {
        a1: {
          "@type": "Alert",
          action: "display",
          trigger: { "@type": "OffsetTrigger", offset: "-PT10M" },
        },
      },
      calendarIds: { [CALENDAR_ID]: true },
      duration: "PT30M",
      id: "ev1",
      start: `${day}T08:00:00`,
      timeZone: "Etc/UTC",
      title: "Standup",
      uid: "ev1@example.com",
      ...utc(day, "08:00", 30),
    },
    {
      "@type": "Event",
      calendarIds: { [CALENDAR_ID]: true },
      // Untrusted HTML: the script and the tracker must never run or load.
      description:
        '<p>Agenda</p><script>parent.postMessage("pwned","*")</script><img src="http://tracker.test/pixel.gif" width="1" height="1"><a href="https://docs.example/review">Notes</a>',
      descriptionContentType: "text/html",
      duration: "PT1H",
      id: "ev2",
      locations: { l1: { "@type": "Location", name: "Room ‮1" } },
      start: `${day}T13:00:00`,
      timeZone: "Etc/UTC",
      title: "Design review",
      uid: "ev2@example.com",
      virtualLocations: {
        v1: {
          "@type": "VirtualLocation",
          name: "Video",
          uri: "https://meet.example/x",
        },
      },
      ...utc(day, "13:00", 60),
    },
    {
      "@type": "Event",
      calendarIds: { [CALENDAR_ID]: true },
      duration: "P1D",
      id: "ev3",
      showWithoutTime: true,
      start: `${day}T00:00:00`,
      title: "Holiday",
      uid: "ev3@example.com",
      ...utc(day, "00:00", 1440),
    },
    {
      // Ours, with Carol invited: she proposes another time (COUNTER).
      "@type": "Event",
      alerts: {
        a1: {
          "@type": "Alert",
          action: "display",
          trigger: { "@type": "OffsetTrigger", offset: "-PT15M" },
        },
      },
      calendarIds: { [CALENDAR_ID]: true },
      duration: "PT1H",
      id: PLANNING_ID,
      organizerCalendarAddress: "mailto:jane@example.com",
      participants: {
        c: {
          "@type": "Participant",
          calendarAddress: "mailto:carol@partner.example",
          name: "Carol",
          participationStatus: "accepted",
          roles: { attendee: true },
        },
        o: {
          "@type": "Participant",
          calendarAddress: "mailto:jane@example.com",
          name: "Jane",
          participationStatus: "accepted",
          roles: { attendee: true, owner: true },
        },
      },
      sequence: 0,
      start: `${tomorrowUtc()}T09:00:00`,
      timeZone: "Etc/UTC",
      title: "Planning",
      uid: PLANNING_UID,
      ...utc(tomorrowUtc(), "09:00", 60),
    },
    {
      // Carol's weekly series, in our calendar (Stalwart added it).
      "@type": "Event",
      calendarIds: { [CALENDAR_ID]: true },
      duration: "PT30M",
      id: SERIES_ID,
      organizerCalendarAddress: "mailto:carol@partner.example",
      participants: { c: CAROL, j: JANE_INVITED },
      recurrenceRule: { "@type": "RecurrenceRule", frequency: "weekly" },
      sequence: 0,
      start: `${day}T10:00:00`,
      timeZone: "Etc/UTC",
      title: "Weekly sync",
      uid: WEEKLY_UID,
    },
    {
      // A task (VTODO from another client) in the same calendar, as
      // Stalwart answers an expanded query: `start` = the due
      // time, and UTC times. The calendar views must not show it.
      "@type": "Task",
      calendarIds: { [CALENDAR_ID]: true },
      due: `${day}T11:00:00`,
      id: TASK_ID,
      progress: "needs-action",
      start: `${day}T11:00:00`,
      timeZone: "Etc/UTC",
      title: TASK_TITLE,
      uid: "task-1@example.com",
      ...utc(day, "11:00", 30),
    },
  ];
};

/** Today's occurrence of the weekly series (an expanded, synthetic id). */
const occurrences = (): FakeEvent[] => {
  const day = todayUtc();
  return [
    {
      baseEventId: SERIES_ID,
      calendarIds: { [CALENDAR_ID]: true },
      duration: "PT30M",
      id: OCCURRENCE_ID,
      participants: { c: CAROL, j: JANE_INVITED },
      recurrenceId: `${day}T10:00:00`,
      start: `${day}T10:00:00`,
      timeZone: "Etc/UTC",
      title: "Weekly sync",
      uid: WEEKLY_UID,
      ...utc(day, "10:00", 30),
    },
  ];
};

const sharedEvents = (): FakeEvent[] => {
  const day = todayUtc();
  return [
    {
      "@type": "Event",
      calendarIds: { [SHARED_CALENDAR_ID]: true },
      duration: "PT1H",
      // Same id as our Standup: per-account caches must keep them apart.
      id: "ev1",
      start: `${day}T15:00:00`,
      timeZone: "Etc/UTC",
      title: "Team review",
      uid: "bob-review@example.com",
      ...utc(day, "15:00", 60),
    },
  ];
};

const ShareWith = z.record(z.string(), z.record(z.string(), z.unknown()));

const OWN_RIGHTS = {
  mayDelete: true,
  mayReadFreeBusy: true,
  mayReadItems: true,
  mayRSVP: true,
  mayShare: true,
  mayUpdatePrivate: true,
  mayWriteAll: true,
  mayWriteOwn: true,
};

/** The task list tasks.e2e.ts adds (a calendar with tasks, like Stalwart). */
export const ERRANDS_ID = "errands";

/** "YYYY-MM-DD" (UTC) `offset` days from today. */
const dayFromToday = (offset: number): string =>
  new Date(Date.now() + offset * 86_400_000).toISOString().slice(0, 10);

const inTaskList = (list: string) => ({ calendarIds: { [list]: true } });

/**
 * Tasks for tasks.e2e.ts, as Stalwart stores them: a list
 * ("Errands"), an overdue high-priority task with notes, a task with two
 * subtasks (one done), a weekly task (start = due, its alerts from the
 * due), one without a date, one done yesterday.
 */
export const addTaskFixtures = (state: FakeCalendar): void => {
  const today = todayUtc();
  const day = dayFromToday;
  state.calendars.push({
    color: "#0E7A4E",
    id: ERRANDS_ID,
    isDefault: false,
    isVisible: true,
    myRights: OWN_RIGHTS,
    name: "Errands",
  });
  const inList = inTaskList;
  state.events.push(
    {
      "@type": "Task",
      ...inList(ERRANDS_ID),
      description: "Semi-skimmed, two bottles.",
      due: `${day(-1)}T00:00:00`,
      id: "buymilk",
      priority: 1,
      progress: "needs-action",
      showWithoutTime: true,
      title: "Buy milk",
      uid: "buy-milk@example.com",
    },
    {
      "@type": "Task",
      ...inList(CALENDAR_ID),
      due: `${day(1)}T00:00:00`,
      id: "plantrip",
      priority: 5,
      progress: "needs-action",
      showWithoutTime: true,
      title: "Plan the trip",
      uid: "plan-trip@example.com",
    },
    {
      "@type": "Task",
      ...inList(CALENDAR_ID),
      id: "flights",
      progress: "needs-action",
      relatedTo: {
        "plan-trip@example.com": {
          "@type": "Relation",
          relation: { parent: true },
        },
      },
      title: "Book flights",
      uid: "flights@example.com",
    },
    {
      "@type": "Task",
      ...inList(CALENDAR_ID),
      iCalendar: {
        name: "vtodo",
        properties: [["completed", {}, "unknown", "20261001T100000Z"]],
      },
      id: "pack",
      progress: "completed",
      relatedTo: {
        "plan-trip@example.com": {
          "@type": "Relation",
          relation: { parent: true },
        },
      },
      title: "Pack the bags",
      uid: "pack@example.com",
    },
    {
      "@type": "Task",
      ...inList(CALENDAR_ID),
      alerts: {
        a1: {
          "@type": "Alert",
          action: "display",
          trigger: {
            "@type": "OffsetTrigger",
            offset: "-PT15M",
            relativeTo: "end",
          },
        },
      },
      due: `${today}T09:00:00`,
      id: "report",
      progress: "needs-action",
      recurrenceRule: { "@type": "RecurrenceRule", frequency: "weekly" },
      start: `${today}T09:00:00`,
      timeZone: "Etc/UTC",
      title: "Weekly report",
      uid: "weekly-report@example.com",
    },
    {
      "@type": "Task",
      ...inList(ERRANDS_ID),
      id: "readbook",
      progress: "needs-action",
      title: "Read a book",
      uid: "read-book@example.com",
    },
    {
      "@type": "Task",
      ...inList(ERRANDS_ID),
      due: `${day(-1)}T00:00:00`,
      iCalendar: {
        name: "vtodo",
        properties: [
          [
            "completed",
            {},
            "unknown",
            `${day(-1).replaceAll("-", "")}T180000Z`,
          ],
        ],
      },
      id: "callmom",
      progress: "completed",
      showWithoutTime: true,
      title: "Call mom",
      uid: "call-mom@example.com",
    }
  );
};

export const fakeCalendar = (): FakeCalendar => ({
  alerts: [],
  calendarSets: [],
  calendars: [
    {
      color: "#2353C9",
      id: CALENDAR_ID,
      isDefault: true,
      isVisible: true,
      myRights: OWN_RIGHTS,
      // Long like Stalwart's default names: the sidebar must cut it.
      name: "Stalwart Calendar (alice@nebulamail.test)",
    },
  ],
  emails: [],
  events: events(),
  occurrences: occurrences(),
  sets: [],
  trash: [],
  trashSets: [],
  sharedCalendars: [
    {
      color: "#0E7A4E",
      id: SHARED_CALENDAR_ID,
      isDefault: false,
      isVisible: true,
      myRights: {
        mayDelete: false,
        mayReadFreeBusy: true,
        mayReadItems: true,
        mayRSVP: true,
        mayShare: false,
        mayWriteAll: true,
        mayWriteOwn: true,
      },
      name: "Team calendar",
    },
  ],
  proposals: [],
  sharedEvents: sharedEvents(),
  submissions: [],
});

/** The invitation inside the fake inbox's "Board meeting" email. */
const invitation = (): Json => {
  const day = todayUtc();
  return {
    "@type": "Event",
    duration: "PT1H",
    method: "request",
    organizerCalendarAddress: "mailto:carol@partner.example",
    participants: { c: CAROL, j: JANE_INVITED },
    sequence: 1,
    start: `${day}T13:30:00`,
    timeZone: "Etc/UTC",
    title: "Board meeting",
    uid: INVITATION_UID,
  };
};

/** An update for today's occurrence of the weekly series only. */
const occurrenceInvitation = (): Json => {
  const day = todayUtc();
  return {
    "@type": "Event",
    duration: "PT30M",
    method: "request",
    organizerCalendarAddress: "mailto:carol@partner.example",
    participants: { c: CAROL, j: JANE_INVITED },
    // Stalwart writes it in the event's zone.
    recurrenceId: `${day}T10:00:00`,
    recurrenceIdTimeZone: "Etc/UTC",
    sequence: 0,
    start: `${day}T10:00:00`,
    timeZone: "Etc/UTC",
    title: "Weekly sync",
    uid: WEEKLY_UID,
  };
};

/** Carol proposes Planning one hour later, with a note. */
const counter = (): Json => ({
  "@type": "Event",
  duration: "PT1H",
  iCalendar: {
    name: "vevent",
    properties: [["comment", {}, "unknown", "Can we start at 10?"]],
  },
  method: "counter",
  organizerCalendarAddress: "mailto:jane@example.com",
  participants: {
    c: {
      "@type": "Participant",
      calendarAddress: "mailto:carol@partner.example",
      name: "Carol",
      participationStatus: "accepted",
    },
    o: {
      "@type": "Participant",
      calendarAddress: "mailto:jane@example.com",
      name: "Jane",
      roles: { owner: true },
    },
  },
  sequence: 0,
  start: `${tomorrowUtc()}T10:00:00`,
  timeZone: "Etc/UTC",
  title: "Planning",
  uid: PLANNING_UID,
});

/** Carol assigns Jane a task. */
const taskAssignment = (): Json => ({
  "@type": "Task",
  due: `${tomorrowUtc()}T00:00:00`,
  method: "request",
  organizerCalendarAddress: "mailto:carol@partner.example",
  participants: { c: CAROL, j: JANE_INVITED },
  progress: "needs-action",
  sequence: 0,
  showWithoutTime: true,
  title: "Translate the brochure",
  uid: TASK_ASSIGNMENT_UID,
});

/**
 * A task Carol assigned to Jane, as the server stores it after Jane
 * accepted (for tasks.e2e.ts).
 */
export const assignedTask = (): FakeEvent => ({
  "@type": "Task",
  calendarIds: { [CALENDAR_ID]: true },
  id: "assigned1",
  organizerCalendarAddress: "mailto:carol@partner.example",
  participants: {
    c: CAROL,
    j: { ...JANE_INVITED, participationStatus: "accepted" },
  },
  progress: "needs-action",
  sequence: 1,
  title: "Review the contract",
  uid: "contract-1@partner.example",
});

/**
 * A weekly task Carol assigned to Jane, in Madrid time, with one Friday
 * taken out: as the app stores a repeating task (`start` = `due`).
 */
export const assignedSeries = (): FakeEvent => ({
  ...assignedTask(),
  due: "2031-03-07T09:00:00",
  id: "assigned2",
  recurrenceOverrides: { "2031-03-14T09:00:00": { excluded: true } },
  recurrenceRule: {
    "@type": "RecurrenceRule",
    byDay: [{ day: "fr" }],
    frequency: "weekly",
  },
  start: "2031-03-07T09:00:00",
  timeZone: "Europe/Madrid",
  title: "Weekly figures",
  uid: "weekly-figures@example.com",
});

/**
 * Bob passes on to Jane the weekly task Carol assigned him, as the server
 * parses the REQUEST the app writes (the series with its occurrences).
 * The sender's alerts must not reach Jane's tasks.
 */
const taskSeries = (): Json => ({
  "@type": "Task",
  alerts: {
    a: {
      "@type": "Alert",
      trigger: { "@type": "OffsetTrigger", offset: "-PT1H", relativeTo: "end" },
    },
  },
  due: "2031-03-07T09:00:00",
  method: "request",
  organizerCalendarAddress: "mailto:carol@partner.example",
  participants: {
    b: {
      "@type": "Participant",
      calendarAddress: "mailto:bob@example.com",
      delegatedTo: { "mailto:jane@example.com": true },
      name: "Bob",
      participationStatus: "delegated",
    },
    c: CAROL,
    j: {
      ...JANE_INVITED,
      delegatedFrom: { "mailto:bob@example.com": true },
    },
  },
  progress: "needs-action",
  recurrenceOverrides: {
    "2031-03-14T09:00:00": { excluded: true },
    "2031-03-21T09:00:00": {
      "alerts/b": { "@type": "Alert" },
      due: "2031-03-22T12:00:00",
      start: "2031-03-22T12:00:00",
    },
  },
  recurrenceRule: {
    "@type": "RecurrenceRule",
    byDay: [{ day: "fr" }],
    frequency: "weekly",
  },
  sequence: 2,
  start: "2031-03-07T09:00:00",
  timeZone: "Europe/Madrid",
  title: "Weekly figures",
  uid: TASK_SERIES_UID,
});

/** A task Jane assigned to Carol, as stored in Jane's calendar. */
export const myAssignedTask = (): FakeEvent => ({
  "@type": "Task",
  calendarIds: { [CALENDAR_ID]: true },
  due: `${tomorrowUtc()}T00:00:00`,
  id: "mine1",
  organizerCalendarAddress: "mailto:jane@example.com",
  participants: {
    c: { ...CAROL, roles: { attendee: true } },
    j: {
      "@type": "Participant",
      calendarAddress: "mailto:jane@example.com",
      name: "Jane",
      participationStatus: "accepted",
      roles: { attendee: true, owner: true },
    },
  },
  progress: "needs-action",
  sequence: 1,
  showWithoutTime: true,
  title: "Print the brochure",
  uid: TASK_COUNTER_UID,
});

/** Carol's COUNTER for `myAssignedTask`: three days later, with a note. */
const taskCounter = (): Json => ({
  "@type": "Task",
  due: "2031-03-14T00:00:00",
  iCalendar: {
    name: "vtodo",
    properties: [["comment", {}, "unknown", "The printer is busy until then."]],
  },
  method: "counter",
  organizerCalendarAddress: "mailto:jane@example.com",
  participants: {
    c: { ...CAROL, roles: { attendee: true } },
    j: {
      "@type": "Participant",
      calendarAddress: "mailto:jane@example.com",
      name: "Jane",
      roles: { owner: true },
    },
  },
  sequence: 1,
  showWithoutTime: true,
  title: "Print the brochure",
  uid: TASK_COUNTER_UID,
});

const PARSED: Readonly<Record<string, () => Json>> = {
  [TASK_ASSIGNMENT_BLOB]: taskAssignment,
  [TASK_COUNTER_BLOB]: taskCounter,
  [TASK_SERIES_BLOB]: taskSeries,
  [COUNTER_BLOB]: counter,
  [INVITATION_BLOB]: invitation,
  [OCCURRENCE_BLOB]: occurrenceInvitation,
};

const Args = z.record(z.string(), z.unknown());

const DEFAULT_ALERTS = [
  "defaultAlertsWithTime",
  "defaultAlertsWithoutTime",
] as const;

const isZone = (zone: string): boolean => {
  try {
    new Intl.DateTimeFormat("en", { timeZone: zone }).format(0);
    return true;
  } catch {
    return false;
  }
};

/**
 * Applies the zone and default alerts of a create or update like Stalwart
 * 1.0 does: whole maps (null clears) or `<map>/<id>`; an unknown zone, or
 * an alert id used twice in the account (both maps, every calendar), is
 * refused (returns the property, nothing changes).
 */
const applyDefaults = (
  row: FakeCalendarRow,
  patch: Json,
  refuseZones: boolean,
  others: readonly FakeCalendarRow[]
): string | null => {
  const next: FakeCalendarRow = { ...row };
  for (const [path, value] of Object.entries(patch)) {
    if (path === "timeZone") {
      if (
        value !== null &&
        (refuseZones || typeof value !== "string" || !isZone(value))
      ) {
        return "timeZone";
      }
      next.timeZone = value;
      continue;
    }
    for (const map of DEFAULT_ALERTS) {
      if (path === map) {
        if (value !== null && typeof value !== "object") {
          return map;
        }
        next[map] = value === null ? {} : Args.parse(value);
      } else if (path.startsWith(`${map}/`)) {
        const entries = { ...next[map] };
        const id = path.slice(map.length + 1);
        if (value === null) {
          // oxlint-disable-next-line typescript/no-dynamic-delete -- a fake server's map.
          delete entries[id];
        } else {
          entries[id] = Args.parse(value);
        }
        next[map] = entries;
      }
    }
  }
  const idsOf = (calendar: FakeCalendarRow): string[] =>
    DEFAULT_ALERTS.flatMap((map) => Object.keys(calendar[map] ?? {}));
  const mine = idsOf(next);
  const taken = new Set(
    others.filter((other) => other.id !== row.id).flatMap(idsOf)
  );
  if (new Set(mine).size !== mine.length || mine.some((id) => taken.has(id))) {
    return DEFAULT_ALERTS[0];
  }
  Object.assign(row, next);
  return null;
};
const Ids = z.array(z.string()).catch([]);
const SetArgs = z.object({
  create: z.record(z.string(), Args).catch({}),
  destroy: z.array(z.string()).catch([]),
  update: z.record(z.string(), Args).catch({}),
});
const UidFilter = z.object({ uid: z.string() });
const WindowFilter = z
  .object({
    after: z.string().optional(),
    before: z.string().optional(),
    inCalendar: z.string().optional(),
  })
  .catch({});
const Paging = z.object({
  limit: z.number().int().positive().optional().catch(undefined),
  position: z.number().int().nonnegative().optional().catch(undefined),
});

const DAY_MS = 86_400_000;

/** "2026-10-29T10:00:00" (UTC in the tests) of epoch ms. */
const localOf = (ms: number): string => new Date(ms).toISOString().slice(0, 19);

const utcOf = (ms: number): string =>
  new Date(ms).toISOString().replace(".000", "");

const Rule = z
  .object({
    count: z.number().int().positive().optional(),
    frequency: z.enum(["daily", "weekly", "monthly", "yearly"]),
    interval: z.number().int().positive().optional(),
  })
  .loose();

const Overrides = z
  .record(z.string(), z.record(z.string(), z.unknown()))
  .catch({});

/**
 * Occurrences of the repeating tasks, like Stalwart's expansion: a
 * synthetic id per occurrence, `start` and `utcStart` of that occurrence,
 * the series' `due` (Stalwart's quirk), the override's `progress`, and no
 * excluded ones. Daily and weekly rules (what the tests write).
 */
export const taskOccurrences = (
  stored: readonly FakeEvent[],
  from: number,
  to: number
): FakeEvent[] =>
  stored.flatMap((series) => {
    const rule = Rule.safeParse(series.recurrenceRule);
    if (series["@type"] !== "Task" || !rule.success) {
      return [];
    }
    const anchor = typeof series.start === "string" ? series.start : series.due;
    if (typeof anchor !== "string") {
      return [];
    }
    const first = Date.parse(`${anchor.slice(0, 19)}Z`);
    const step =
      (rule.data.frequency === "weekly" ? 7 : 1) *
      (rule.data.interval ?? 1) *
      DAY_MS;
    const overrides = Overrides.parse(series.recurrenceOverrides ?? {});
    const out: FakeEvent[] = [];
    const count = rule.data.count ?? 1000;
    for (let index = 0; index < count; index += 1) {
      const at = first + index * step;
      if (at >= to) {
        break;
      }
      const recurrenceId = localOf(at);
      const override = overrides[recurrenceId] ?? {};
      if (at < from || override.excluded === true) {
        continue;
      }
      const {
        recurrenceOverrides: _overrides,
        recurrenceRule: _rule,
        ...rest
      } = series;
      out.push({
        ...rest,
        ...override,
        baseEventId: series.id,
        id: `${series.id}-${recurrenceId.slice(0, 10).replaceAll("-", "")}`,
        recurrenceId,
        start: recurrenceId,
        utcEnd: utcOf(at),
        utcStart: utcOf(at),
      });
    }
    return out;
  });

/** Epoch ms of a filter's LocalDateTime (UTC in the tests). */
const windowMs = (value: string | undefined, fallback: number): number =>
  value === undefined ? fallback : Date.parse(`${value.slice(0, 19)}Z`);

const inCalendar = (event: Json, calendar: string | undefined): boolean =>
  calendar === undefined ||
  Args.safeParse(event.calendarIds).data?.[calendar] === true;

const ACCOUNT = "a";
const STATE = "c1";

const accountOf = (args: Json): string =>
  typeof args.accountId === "string" ? args.accountId : ACCOUNT;

const isShared = (args: Json): boolean =>
  accountOf(args) === SHARED_CALENDAR_ACCOUNT;

/** Applies a JMAP patch: "a/b/c" paths, null removes. */
const applyPatch = (event: FakeEvent, patch: Json): FakeEvent => {
  const next: Json = structuredClone({ ...event });
  for (const [path, value] of Object.entries(patch)) {
    const keys = path.split("/");
    const last = keys.pop();
    if (last === undefined) {
      continue;
    }
    let target: Json = next;
    for (const key of keys) {
      // The parsed copy goes in, and the walk goes on in that same copy
      // (parsing it again would write into a throwaway).
      const child = Args.safeParse(target[key]);
      const inner: Json = child.success ? child.data : {};
      target[key] = inner;
      target = inner;
    }
    if (value === null) {
      Reflect.deleteProperty(target, last);
    } else {
      target[last] = value;
    }
  }
  return { ...next, id: event.id };
};

const calendarsOf = (state: FakeCalendar, args: Json): FakeCalendarRow[] =>
  isShared(args) ? state.sharedCalendars : state.calendars;

const storedOf = (state: FakeCalendar, args: Json): FakeEvent[] =>
  isShared(args) ? state.sharedEvents : state.events;

type Method = (state: FakeCalendar, args: Json) => Json;

export const CALENDAR_METHODS: Readonly<Record<string, Method>> = {
  "Calendar/get": (state, args) => ({
    accountId: accountOf(args),
    list: calendarsOf(state, args).map((calendar) => ({
      color: calendar.color,
      defaultAlertsWithTime: calendar.defaultAlertsWithTime ?? {},
      defaultAlertsWithoutTime: calendar.defaultAlertsWithoutTime ?? {},
      description: null,
      id: calendar.id,
      includeInAvailability: "all",
      isDefault: calendar.isDefault,
      // Like Stalwart: a calendar shared with us isn't subscribed yet.
      isSubscribed: !isShared(args),
      isVisible: calendar.isVisible,
      myRights: calendar.myRights,
      name: calendar.name,
      shareWith: isShared(args) ? null : (calendar.shareWith ?? {}),
      sortOrder: 0,
      timeZone: calendar.timeZone ?? null,
      ...nextcloudProperties(state, calendar, isShared(args)),
    })),
    notFound: [],
    state: STATE,
  }),
  "Calendar/set": (state, args) => {
    state.calendarSets.push(args);
    const { create, destroy, update } = SetArgs.parse(args);
    const rows = calendarsOf(state, args);
    const created: Json = {};
    const notCreated: Json = {};
    const notUpdated: Json = {};
    for (const [key, input] of Object.entries(create)) {
      const id = `cal${String(rows.length + 1)}`;
      const row: FakeCalendarRow = {
        color: typeof input.color === "string" ? input.color : null,
        id,
        isDefault: false,
        isVisible: true,
        myRights: OWN_RIGHTS,
        name: typeof input.name === "string" ? input.name : "",
        ...(typeof input[SUBSCRIPTION_URL] === "string"
          ? {
              myRights: READ_ONLY_RIGHTS,
              subscription: input[SUBSCRIPTION_URL],
            }
          : {}),
        ...(input.shareWith === undefined
          ? {}
          : { shareWith: ShareWith.parse(input.shareWith) }),
      };
      const refused = applyDefaults(
        row,
        input,
        state.refuseZones === true,
        rows
      );
      if (refused === null) {
        rows.push(row);
        created[key] = { id };
      } else {
        notCreated[key] = { properties: [refused], type: "invalidProperties" };
      }
    }
    for (const [id, patch] of Object.entries(update)) {
      const row = rows.find((calendar) => calendar.id === id);
      const refused =
        row === undefined
          ? null
          : applyDefaults(row, patch, state.refuseZones === true, rows);
      if (refused !== null) {
        notUpdated[id] = { properties: [refused], type: "invalidProperties" };
      } else if (row !== undefined) {
        if (typeof patch.isVisible === "boolean") {
          row.isVisible = patch.isVisible;
        }
        if (typeof patch.color === "string") {
          row.color = patch.color;
        }
        if (typeof patch.name === "string") {
          row.name = patch.name;
        }
        if (typeof patch[IS_PUBLISHED_PATCH] === "boolean") {
          row.published = patch[IS_PUBLISHED_PATCH];
        }
        // `shareWith/<principal id>`, like Stalwart: rights or null.
        for (const [path, rights] of Object.entries(patch)) {
          if (path.startsWith("shareWith/")) {
            const shares = { ...row.shareWith };
            const principal = path.slice("shareWith/".length);
            if (rights === null) {
              // oxlint-disable-next-line typescript/no-dynamic-delete -- a fake server's map.
              delete shares[principal];
            } else {
              shares[principal] = Args.parse(rights);
            }
            row.shareWith = shares;
          }
        }
      }
    }
    for (const id of destroy) {
      const index = rows.findIndex((calendar) => calendar.id === id);
      if (index !== -1) {
        rows.splice(index, 1);
      }
    }
    return {
      accountId: accountOf(args),
      created,
      destroyed: destroy,
      newState: STATE,
      notCreated,
      notUpdated,
      oldState: STATE,
      updated: Object.fromEntries(
        Object.keys(update)
          .filter((id) => !(id in notUpdated))
          .map((id) => [id, null])
      ),
    };
  },
  "CalendarEvent/get": (state, args) => {
    const properties = Ids.catch([]).parse(args.properties);
    if (
      properties.includes("recurrenceOverrides") &&
      (properties.includes("utcStart") || properties.includes("utcEnd"))
    ) {
      // Stalwart 1.0 (calendar_event/get.rs).
      throw new FakeMethodError(
        "invalidArguments",
        "recurrenceOverrides cannot be requested together with utcStart or utcEnd"
      );
    }
    const ids = Ids.parse(args.ids);
    const now = Date.now();
    const all = isShared(args)
      ? state.sharedEvents
      : [
          ...state.events,
          ...state.occurrences,
          ...taskOccurrences(
            state.events,
            now - 400 * DAY_MS,
            now + 400 * DAY_MS
          ),
        ];
    return {
      accountId: accountOf(args),
      list: all
        .filter((event) => ids.includes(event.id))
        // Like Stalwart 1.0: a stored object (a one-off event expanded
        // under its real id too) has no base; occurrences name theirs.
        .map((event) => ({ baseEventId: null, ...event })),
      notFound: [],
      state: STATE,
    };
  },
  // The bridge's own methods: it builds and mails the
  // COUNTER / DECLINECOUNTER; here they are only recorded.
  "CalendarEvent/counter": (state, args) => {
    state.proposals.push(["CalendarEvent/counter", args]);
    return { accountId: accountOf(args) };
  },
  "CalendarEvent/declineCounter": (state, args) => {
    state.proposals.push(["CalendarEvent/declineCounter", args]);
    return { accountId: accountOf(args) };
  },
  // Nextcloud's trash bin through the bridge.
  "CalendarTrash/get": (state, args) => ({
    accountId: accountOf(args),
    calendars: state.trash
      .filter((item) => item.kind === "calendar")
      .map((item) => ({
        color: "#C0392B",
        deletedAt: `${todayUtc()}T08:00:00Z`,
        id: item.id,
        name: item.name,
      })),
    events: state.trash
      .filter((item) => item.kind === "event")
      .map((item) => ({
        calendarId: item.calendarId ?? null,
        deletedAt: `${todayUtc()}T09:00:00Z`,
        id: item.id,
        title: item.name,
        utcStart: `${todayUtc()}T10:00:00Z`,
      })),
    retentionSeconds: 2_592_000,
  }),
  "CalendarTrash/set": (state, args) => {
    state.trashSets.push(args);
    const restore = Ids.catch([]).parse(args.restore);
    const destroy = Ids.catch([]).parse(args.destroy);
    const done = (ids: readonly string[]): string[] =>
      ids.filter((id) => state.trash.some((item) => item.id === id));
    const restored = done(restore);
    const destroyed = done(destroy);
    for (const id of [...restored, ...destroyed]) {
      const index = state.trash.findIndex((item) => item.id === id);
      const [item] = state.trash.splice(index, 1);
      if (item?.kind === "calendar" && restored.includes(id)) {
        state.calendars.push({
          color: "#C0392B",
          id: `restored-${id}`,
          isDefault: false,
          isVisible: true,
          myRights: OWN_RIGHTS,
          name: item.name,
        });
      }
    }
    return {
      accountId: accountOf(args),
      destroyed,
      notDestroyed: {},
      notRestored: {},
      restored,
    };
  },
  "CalendarEvent/parse": (_state, args) => ({
    accountId: accountOf(args),
    parsed: Object.fromEntries(
      Ids.parse(args.blobIds).flatMap((id) => {
        const make = PARSED[id];
        return make === undefined ? [] : [[id, [make()]]];
      })
    ),
  }),
  "CalendarEvent/query": (state, args) => {
    const uid = UidFilter.safeParse(args.filter);
    const stored = storedOf(state, args);
    if (uid.success) {
      return {
        accountId: accountOf(args),
        canCalculateChanges: false,
        ids: stored
          .filter((event) => event.uid === uid.data.uid)
          .map((event) => event.id),
        position: 0,
        queryState: STATE,
      };
    }
    if (args.expandRecurrences !== true) {
      // Every stored object (events and tasks), page by page.
      const { limit, position = 0 } = Paging.parse(args);
      const only = WindowFilter.parse(args.filter).inCalendar;
      const all = stored
        .filter((event) => inCalendar(event, only))
        .map((event) => event.id);
      return {
        accountId: accountOf(args),
        canCalculateChanges: false,
        ids: all.slice(
          position,
          limit === undefined ? undefined : position + limit
        ),
        position,
        queryState: STATE,
        ...(args.calculateTotal === true ? { total: all.length } : {}),
      };
    }
    // Expanded: occurrences instead of series.
    const window = WindowFilter.parse(args.filter);
    const tasks = isShared(args)
      ? []
      : taskOccurrences(
          stored,
          windowMs(window.after, -Infinity),
          windowMs(window.before, Infinity)
        );
    const ids = [
      ...stored.filter((event) => event.recurrenceRule === undefined),
      ...(isShared(args) ? [] : state.occurrences),
      ...tasks,
    ]
      .filter((event) => inCalendar(event, window.inCalendar))
      .map((event) => event.id);
    return {
      accountId: accountOf(args),
      canCalculateChanges: false,
      ids,
      position: 0,
      queryState: STATE,
    };
  },
  "CalendarEvent/set": (state, args) => {
    state.sets.push(args);
    const { create, destroy, update } = SetArgs.parse(args);
    const stored = storedOf(state, args);
    const created: Json = {};
    for (const [key, event] of Object.entries(create)) {
      const id = `new${String(stored.length + 1)}`;
      stored.push({ ...event, id });
      created[key] = { id };
    }
    for (const [id, patch] of Object.entries(update)) {
      for (const list of [stored, state.occurrences]) {
        const index = list.findIndex((event) => event.id === id);
        const event = list[index];
        if (event !== undefined) {
          list[index] = applyPatch(event, patch);
        }
      }
    }
    // Like the bridge: no cancellation to more than MAX_RECIPIENTS (20)
    // guests; the event stays.
    const notDestroyed: Json = {};
    const destroyed: string[] = [];
    for (const id of destroy) {
      const index = stored.findIndex((event) => event.id === id);
      if (
        args.sendSchedulingMessages === true &&
        guestsOf(stored[index]) > MAX_RECIPIENTS
      ) {
        notDestroyed[id] = {
          description:
            "too many participants to notify; destroy without sendSchedulingMessages",
          type: "forbidden",
        };
        continue;
      }
      if (index !== -1) {
        stored.splice(index, 1);
      }
      destroyed.push(id);
    }
    return {
      accountId: accountOf(args),
      created,
      destroyed,
      newState: STATE,
      notDestroyed,
      oldState: STATE,
      updated: Object.fromEntries(Object.keys(update).map((id) => [id, null])),
    };
  },
  "EmailSubmission/set": (state, args) => {
    state.submissions.push(args);
    return {
      accountId: ACCOUNT,
      created: {
        send: {
          id: "sub1",
          sendAt: new Date().toISOString(),
          undoStatus: "final",
        },
      },
      newState: "2",
      oldState: "1",
    };
  },
  "ParticipantIdentity/get": () => ({
    accountId: ACCOUNT,
    list: [
      {
        calendarAddress: "mailto:jane@example.com",
        id: "pi1",
        isDefault: true,
        name: "Jane",
      },
    ],
    notFound: [],
    state: "1",
  }),
  "Principal/getAvailability": () => ({ list: [] }),
  "Principal/query": () => ({
    accountId: ACCOUNT,
    canCalculateChanges: false,
    ids: [],
    position: 0,
    queryState: STATE,
  }),
  // No browser push service in the tests; the app only reads these.
  "PushSubscription/get": () => ({ list: [], notFound: [] }),
  "PushSubscription/set": () => ({ created: {}, destroyed: [], updated: {} }),
};

/** Session capabilities for calendars (merged into fake-jmap's session). */
export const CALENDAR_CAPABILITIES = {
  "urn:ietf:params:jmap:calendars": {},
  "urn:ietf:params:jmap:calendars:parse": {},
  "urn:ietf:params:jmap:principals": {},
  "urn:ietf:params:jmap:principals:availability": {},
} as const;

/** Bob's account has calendars too (his shared calendar). */
export const SHARED_CALENDAR_CAPABILITIES = {
  "urn:ietf:params:jmap:calendars": {},
} as const;

/**
 * What the event source sends on this connection: the pending calendar
 * alerts (once each), else nothing.
 */
export const eventSourceBody = (state: FakeCalendar, types: string): string => {
  const wanted = types === "*" || types.split(",").includes("CalendarAlert");
  const alerts = wanted ? state.alerts.splice(0) : [];
  return alerts.length === 0
    ? ": nothing\n\n"
    : alerts
        .map(
          (alert) => `event: calendarAlert\ndata: ${JSON.stringify(alert)}\n\n`
        )
        .join("");
};
