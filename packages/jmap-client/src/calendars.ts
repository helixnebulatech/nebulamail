// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// JMAP for Calendars (draft-ietf-jmap-calendars) with JSCalendar events.
// Servers use the JSCalendar "bis" shape (RFC 8984 update): `start` +
// `duration` + `timeZone`, one `recurrenceRule`, participants with
// `calendarAddress`. Only what the app uses is typed; everything else
// passes through (loose objects). Checked against Stalwart:
//
// - `CalendarEvent/get` leaves out properties with default values
//   (`showWithoutTime: false`, empty maps), even when asked for them, so
//   event properties are all optional here.
// - `CalendarEvent/parse` maps each blob to an array of events, each with
//   the iTIP `method` of its VCALENDAR ("request", "reply", "cancel"…).
// - `Principal/getAvailability` answers `{ list }` only.

import { z } from "zod";

import { CAPABILITIES } from "./capabilities";
import { parseOrThrow } from "./errors";
import {
  AccountId,
  BlobId,
  CalendarEventId,
  CalendarId,
  PrincipalId,
} from "./ids";
import type {
  GetArgs,
  GetResponse,
  Lenient,
  QueryArgs,
  QueryResponse,
  SetArgs,
  SetResponse,
} from "./standard";
import {
  SetErrorSchema,
  getResponseSchema,
  queryResponseSchema,
  setResponseSchema,
} from "./standard";

/** Capabilities beyond `CAPABILITIES` that calendar methods need. */
export const CALENDAR_CAPABILITIES = {
  calendars: CAPABILITIES.calendars,
  parse: "urn:ietf:params:jmap:calendars:parse",
  principals: CAPABILITIES.principals,
  availability: "urn:ietf:params:jmap:principals:availability",
} as const;

const flags = z.record(z.string(), z.boolean());

// ---------------------------------------------------------------- Calendar

const CalendarRightsSchema = z.looseObject({
  mayDelete: z.boolean().optional(),
  mayReadFreeBusy: z.boolean().optional(),
  mayReadItems: z.boolean().optional(),
  mayRSVP: z.boolean().optional(),
  mayShare: z.boolean().optional(),
  mayUpdatePrivate: z.boolean().optional(),
  mayWriteAll: z.boolean().optional(),
  mayWriteOwn: z.boolean().optional(),
});
export type CalendarRights = z.infer<typeof CalendarRightsSchema>;

export const CalendarSchema = z.looseObject({
  color: z.string().nullable(),
  /**
   * Alerts for new events with a time / all-day ones (draft §4.1), by id.
   * Kept loose: each value is checked where it is used, so one odd alert
   * from another client doesn't hide the calendar.
   */
  defaultAlertsWithTime: z
    .record(z.string(), z.unknown())
    .nullish()
    .catch(null),
  defaultAlertsWithoutTime: z
    .record(z.string(), z.unknown())
    .nullish()
    .catch(null),
  description: z.string().nullable(),
  id: CalendarId,
  /** "all" | "attending" | "none" */
  includeInAvailability: z.string().nullable(),
  isDefault: z.boolean(),
  isSubscribed: z.boolean(),
  isVisible: z.boolean(),
  myRights: CalendarRightsSchema.nullable(),
  name: z.string(),
  /** Who else has access, by principal id (null: not shareable here). */
  shareWith: z.partialRecord(PrincipalId, CalendarRightsSchema).nullish(),
  sortOrder: z.number(),
  /** IANA time zone, or null. */
  timeZone: z.string().nullable(),
});
export type Calendar = z.infer<typeof CalendarSchema>;
export type CalendarProperty = keyof typeof CalendarSchema.shape;

export type CalendarGetArgs = GetArgs<CalendarId, CalendarProperty>;

/** Only the properties a client may set (`isDefault`, `myRights` are not). */
export interface CalendarCreate {
  readonly name: string;
  readonly color?: string | null;
  readonly description?: string | null;
  readonly isSubscribed?: boolean;
  readonly isVisible?: boolean;
  readonly sortOrder?: number;
  readonly timeZone?: string | null;
  /**
   * Whole maps (`{}` clears). Stalwart and the bridge take only
   * OffsetTrigger alerts.
   */
  readonly defaultAlertsWithTime?: Readonly<Record<string, Alert>>;
  readonly defaultAlertsWithoutTime?: Readonly<Record<string, Alert>>;
  /** The bridge's (Nextcloud): subscribe to this URL instead. */
  readonly "https://helixnebula.tech/jmap/bridge/subscriptionUrl"?: string;
  /** The bridge's (Nextcloud): publish it at once. */
  readonly "https://helixnebula.tech/jmap/bridge/isPublished"?: boolean;
}

/**
 * Also `shareWith` (whole) or `shareWith/<principal id>` (that principal's
 * rights; null removes). Only in an update: the bridge refuses it on create.
 */
export type CalendarPatch = Partial<
  Omit<CalendarCreate, "https://helixnebula.tech/jmap/bridge/subscriptionUrl">
> &
  Readonly<{
    shareWith?: Readonly<Partial<Record<PrincipalId, CalendarRights>>> | null;
    /** The bridge's `isPublished`, as a patch key (a JSON pointer). */
    "https:~1~1helixnebula.tech~1jmap~1bridge~1isPublished"?: boolean;
  }> &
  Readonly<Record<`shareWith/${string}`, CalendarRights | null>>;

export interface CalendarSetArgs extends SetArgs<
  CalendarId,
  CalendarCreate,
  CalendarPatch
> {
  /** Without it, destroying a calendar with events fails. */
  readonly onDestroyRemoveEvents?: boolean;
  readonly onSuccessSetIsDefault?: CalendarId | `#${string}` | null;
}

// ---------------------------------------------------------------- JSCalendar

/** JSCalendar RecurrenceRule (RFC 8984 §4.3.3). */
export const RecurrenceRuleSchema = z.looseObject({
  byDay: z
    .array(
      z.looseObject({ day: z.string(), nthOfPeriod: z.number().optional() })
    )
    .optional(),
  byHour: z.array(z.number()).optional(),
  byMinute: z.array(z.number()).optional(),
  byMonth: z.array(z.string()).optional(),
  byMonthDay: z.array(z.number()).optional(),
  bySecond: z.array(z.number()).optional(),
  bySetPosition: z.array(z.number()).optional(),
  byWeekNo: z.array(z.number()).optional(),
  byYearDay: z.array(z.number()).optional(),
  count: z.number().optional(),
  /** "yearly" | "monthly" | "weekly" | "daily" | "hourly" | "minutely" | "secondly" */
  frequency: z.string(),
  firstDayOfWeek: z.string().optional(),
  interval: z.number().optional(),
  rscale: z.string().optional(),
  skip: z.string().optional(),
  /** LocalDateTime in the event's time zone. */
  until: z.string().optional(),
});
export type RecurrenceRule = z.infer<typeof RecurrenceRuleSchema>;

export const ParticipantSchema = z.looseObject({
  /** `mailto:` URI the server schedules with (JSCalendar bis). */
  calendarAddress: z.string().nullish(),
  /** Who passed the task or invitation on to this one (`mailto:` keys). */
  delegatedFrom: flags.nullish(),
  /** Who this one passed it on to (PARTSTAT=DELEGATED, `mailto:` keys). */
  delegatedTo: flags.nullish(),
  description: z.string().nullish(),
  email: z.string().nullish(),
  expectReply: z.boolean().nullish(),
  /** "individual" | "group" | "resource" | "location" | "unknown" */
  kind: z.string().nullish(),
  name: z.string().nullish(),
  /** "needs-action" | "accepted" | "declined" | "tentative" | "delegated" */
  participationStatus: z.string().nullish(),
  /**
   * A task's assignee (RFC 8984 §4.4.6): "needs-action" | "in-process" |
   * "completed" | "failed" (VTODO PARTSTAT IN-PROCESS, COMPLETED…).
   */
  progress: z.string().nullish(),
  /** When `progress` was last set (UTCDateTime, RFC 8984 §4.4.6, §5.2.6). */
  progressUpdated: z.string().nullish(),
  /** "owner" | "attendee" | "optional" | "informational" | "chair" … */
  roles: flags.nullish(),
  /** SEQUENCE of the participant's last iTIP response (RFC 8984 §4.4.6). */
  scheduleSequence: z.number().nullish(),
  /** DTSTAMP (`updated`) of that response, UTCDateTime (RFC 8984 §4.4.6). */
  scheduleUpdated: z.string().nullish(),
});
export type Participant = z.infer<typeof ParticipantSchema>;

export const AlertSchema = z.looseObject({
  /** "display" | "email" */
  action: z.string().nullish(),
  trigger: z.looseObject({
    "@type": z.string().optional(),
    /** SignedDuration, for OffsetTrigger (e.g. "-PT15M"). */
    offset: z.string().optional(),
    /** "start" | "end" */
    relativeTo: z.string().optional(),
    /** UTCDateTime, for AbsoluteTrigger. */
    when: z.string().optional(),
  }),
});
export type Alert = z.infer<typeof AlertSchema>;

export const LocationSchema = z.looseObject({
  description: z.string().nullish(),
  name: z.string().nullish(),
});
export type Location = z.infer<typeof LocationSchema>;

export const VirtualLocationSchema = z.looseObject({
  name: z.string().nullish(),
  uri: z.string(),
});
export type VirtualLocation = z.infer<typeof VirtualLocationSchema>;

export const LinkSchema = z.looseObject({
  href: z.string(),
  rel: z.string().nullish(),
  title: z.string().nullish(),
});
export type Link = z.infer<typeof LinkSchema>;

/** RFC 8984 §1.4.10 Relation (`relatedTo` values, keyed by UID). */
export const RelationSchema = z.looseObject({
  /** "first" | "next" | "child" | "parent" … */
  relation: flags.nullish(),
});
export type Relation = z.infer<typeof RelationSchema>;

/**
 * One iCalendar property in jCal form (RFC 7265): name, parameters, value
 * type, value (more values may follow).
 */
export const ICalPropertySchema = z
  .tuple([z.string(), z.record(z.string(), z.unknown()), z.string()])
  .rest(z.unknown());
export type ICalProperty = z.infer<typeof ICalPropertySchema>;

/**
 * What the server could not convert to JSCalendar (RFC 9555 §4.3): extra
 * properties to keep as they are when editing. Entries of `properties` are
 * not checked here (one odd entry must not hide the others, which an edit
 * writes back): read them with `icalProperties` (tasks.ts).
 */
export const ICalComponentSchema = z.looseObject({
  components: z.array(z.unknown()).nullish(),
  convertedProperties: z.record(z.string(), z.unknown()).nullish(),
  name: z.string().nullish(),
  properties: z.array(z.unknown()).nullish(),
});
export type ICalComponent = z.infer<typeof ICalComponentSchema>;

/**
 * Properties only a JSCalendar Task has (RFC 8984 §5.2): a calendar object
 * may be a task (`@type: "Task"`, VTODO), so `CalendarEvent/*` carries
 * them too (tasks.ts).
 */
export const taskOnlyShape = {
  /** LocalDateTime in `timeZone` (a date at midnight if `showWithoutTime`). */
  due: z.string().nullish(),
  /** Duration, e.g. "PT2H". */
  estimatedDuration: z.string().nullish(),
  percentComplete: z.number().nullish(),
  /** 0 = none, 1 = highest … 9 = lowest (RFC 5545 PRIORITY). */
  priority: z.number().nullish(),
  /** "needs-action" | "in-process" | "completed" | "failed" | "cancelled" */
  progress: z.string().nullish(),
  /** UTCDateTime `progress` was last set (RFC 8984 §5.2.6). */
  progressUpdated: z.string().nullish(),
};

/** The JSCalendar Event properties shared by stored and parsed events. */
const jsCalendarShape = {
  ...taskOnlyShape,
  categories: flags.nullish(),
  created: z.string().nullish(),
  /**
   * What the server couldn't convert (only returned when asked for). Not
   * an object: read as none rather than failing the whole response.
   */
  iCalendar: ICalComponentSchema.nullish().catch(null),
  relatedTo: z.record(z.string(), RelationSchema).nullish().catch(null),
  "@type": z.string().nullish(),
  alerts: z.record(z.string(), AlertSchema).nullish(),
  color: z.string().nullish(),
  description: z.string().nullish(),
  /** "text/plain" (default) or "text/html". */
  descriptionContentType: z.string().nullish(),
  /** Duration, e.g. "PT1H" or "P1D". */
  duration: z.string().nullish(),
  /** "free" | "busy" */
  freeBusyStatus: z.string().nullish(),
  keywords: flags.nullish(),
  links: z.record(z.string(), LinkSchema).nullish(),
  locations: z.record(z.string(), LocationSchema).nullish(),
  /** iTIP method of a parsed or scheduling object ("request", "reply"…). */
  method: z.string().nullish(),
  organizerCalendarAddress: z.string().nullish(),
  participants: z.record(z.string(), ParticipantSchema).nullish(),
  /** "public" | "private" | "secret" */
  privacy: z.string().nullish(),
  recurrenceId: z.string().nullish(),
  recurrenceIdTimeZone: z.string().nullish(),
  recurrenceOverrides: z
    .record(z.string(), z.record(z.string(), z.unknown()))
    .nullish(),
  recurrenceRule: RecurrenceRuleSchema.nullish(),
  sequence: z.number().nullish(),
  /** True for all-day events (dates without a time). */
  showWithoutTime: z.boolean().nullish(),
  /** LocalDateTime, e.g. "2026-10-26T10:00:00" (no zone, no offset). */
  start: z.string().nullish(),
  /** "confirmed" | "cancelled" | "tentative" */
  status: z.string().nullish(),
  /** IANA name; null or missing means floating time. */
  timeZone: z.string().nullish(),
  title: z.string().nullish(),
  uid: z.string().nullish(),
  updated: z.string().nullish(),
  virtualLocations: z.record(z.string(), VirtualLocationSchema).nullish(),
};

export const CalendarEventSchema = z.looseObject({
  ...jsCalendarShape,
  /** The recurring event an occurrence belongs to (expanded ids only). */
  baseEventId: CalendarEventId.nullish(),
  calendarIds: z.record(z.string(), z.boolean()).nullish(),
  id: CalendarEventId,
  isDraft: z.boolean().nullish(),
  /** True if the user is the organizer (or there is none). */
  isOrigin: z.boolean().nullish(),
  /** UTCDateTime ("…Z"); only returned when asked for. */
  utcEnd: z.string().nullish(),
  utcStart: z.string().nullish(),
});
export type CalendarEvent = z.infer<typeof CalendarEventSchema>;
export type CalendarEventProperty = keyof typeof CalendarEventSchema.shape;

/** An event from `CalendarEvent/parse` (no JMAP-level properties). */
export const ParsedEventSchema = z.looseObject(jsCalendarShape);
export type ParsedEvent = z.infer<typeof ParsedEventSchema>;

export interface CalendarEventGetArgs extends GetArgs<
  CalendarEventId,
  CalendarEventProperty
> {
  /** Time zone for `utcStart`/`utcEnd` of floating events. */
  readonly timeZone?: string;
  readonly reduceParticipants?: boolean;
  readonly recurrenceOverridesAfter?: string;
  readonly recurrenceOverridesBefore?: string;
}

/** Draft §5.8. Stalwart has `inCalendar` (one id, not `inCalendars`). */
export interface CalendarEventFilterCondition {
  readonly inCalendar?: CalendarId;
  /** LocalDateTime, read in the query's `timeZone` (default UTC). */
  readonly after?: string;
  readonly before?: string;
  readonly text?: string;
  readonly title?: string;
  readonly description?: string;
  readonly location?: string;
  readonly owner?: string;
  readonly attendee?: string;
  readonly uid?: string;
}

export type CalendarEventSortProperty = "start" | "uid" | "recurrenceId";

export interface CalendarEventQueryArgs extends QueryArgs<
  CalendarEventFilterCondition,
  {
    readonly property: CalendarEventSortProperty;
    readonly isAscending?: boolean;
  }
> {
  /** One id per occurrence; needs both `after` and `before`. */
  readonly expandRecurrences?: boolean;
  readonly timeZone?: string;
}

/** What a client sends to create an event (a JSCalendar Event). */
export interface CalendarEventCreate {
  readonly [property: string]: unknown;
  readonly calendarIds: Readonly<Partial<Record<CalendarId, true>>>;
  readonly title?: string;
  readonly start?: string;
  readonly duration?: string;
  readonly timeZone?: string | null;
  readonly showWithoutTime?: boolean;
}

export interface CalendarEventSetArgs extends SetArgs<
  CalendarEventId,
  CalendarEventCreate
> {
  /**
   * Let the server send iTIP messages (iMIP email) to participants:
   * invitations, updates and cancellations as organizer, replies as
   * attendee. Default false.
   */
  readonly sendSchedulingMessages?: boolean;
}

export interface CalendarEventParseArgs {
  readonly accountId: AccountId;
  readonly blobIds: readonly BlobId[];
  readonly properties?: readonly string[];
}

export interface CalendarEventParseResponse {
  readonly accountId: AccountId;
  /** Every event found in each blob (several for recurrence overrides). */
  readonly parsed: Partial<Record<string, Lenient<ParsedEvent>[]>>;
  readonly notParsable: BlobId[];
  readonly notFound: BlobId[];
}

// ---------------------------------------------------------------- Principals

// Principal/get and Principal/query: principals.ts.

export interface PrincipalGetAvailabilityArgs {
  readonly accountId: AccountId;
  readonly id: PrincipalId;
  /** UTCDateTime range. */
  readonly utcStart: string;
  readonly utcEnd: string;
  readonly showDetails?: boolean;
}

/**
 * `CalendarEvent/counter` (the Nebula Mail bridge's own method, with its
 * capability in `using`): the user proposes another time to the organizer
 * of a stored event (`eventId`, maybe an occurrence id) or of the
 * invitation in an email (`blobId`, its text/calendar part). The bridge
 * builds and mails the iTIP COUNTER.
 */
export type CalendarEventCounterArgs = {
  readonly accountId: AccountId;
  /** UTCDateTime range of the proposal. */
  readonly utcStart: string;
  readonly utcEnd: string;
  readonly comment?: string;
} & (
  | { readonly eventId: CalendarEventId; readonly blobId?: never }
  | { readonly blobId: BlobId; readonly eventId?: never }
);

/**
 * `CalendarEvent/declineCounter` (the bridge's own): the organizer of
 * `eventId` keeps the time; `blobId` is the COUNTER part of the guest's
 * email. The bridge mails the DECLINECOUNTER.
 */
export interface CalendarEventDeclineCounterArgs {
  readonly accountId: AccountId;
  readonly eventId: CalendarEventId;
  readonly blobId: BlobId;
  readonly comment?: string;
}

/** Both answer with the account once the email went out. */
export interface CalendarItipSentResponse {
  readonly accountId: AccountId;
}

/**
 * The Nebula Mail bridge's calendar extras on Nextcloud
 * vendor Calendar properties, shown only with its
 * capability in `using`. A patch key is a JSON pointer: use
 * `bridgePatchKey` for these (their slashes become `~1`).
 */
export const BRIDGE_CALENDAR = {
  /** Published at a public link (only on calendars that can be). */
  isPublished: "https://helixnebula.tech/jmap/bridge/isPublished",
  /** The public link (server-set; null while not published). */
  publicUrl: "https://helixnebula.tech/jmap/bridge/publicUrl",
  /** A webcal subscription's source; set on create to subscribe. */
  subscriptionUrl: "https://helixnebula.tech/jmap/bridge/subscriptionUrl",
} as const;

/** A patch key for a property whose name has slashes (RFC 6901). */
export const bridgePatchKey = (property: string): string =>
  property.replaceAll("~", "~0").replaceAll("/", "~1");

/** `CalendarTrash/get` (the bridge's own: Nextcloud's trash bin). */
export interface CalendarTrashGetArgs {
  readonly accountId: AccountId;
}

const TrashedCalendarSchema = z.looseObject({
  color: z.string().nullish(),
  deletedAt: z.string().nullish(),
  id: z.string(),
  name: z.string().nullish(),
});
export type TrashedCalendar = z.infer<typeof TrashedCalendarSchema>;

const TrashedEventSchema = z.looseObject({
  calendarId: z.string().nullish(),
  deletedAt: z.string().nullish(),
  id: z.string(),
  title: z.string().nullish(),
  utcStart: z.string().nullish(),
});
export type TrashedEvent = z.infer<typeof TrashedEventSchema>;

const TrashGetSchema = z.looseObject({
  accountId: AccountId,
  calendars: z.array(TrashedCalendarSchema),
  events: z.array(TrashedEventSchema),
  retentionSeconds: z.number().nullish(),
});
export type CalendarTrashGetResponse = z.infer<typeof TrashGetSchema>;

/** `CalendarTrash/set`: restore some items, delete others for good. */
export interface CalendarTrashSetArgs {
  readonly accountId: AccountId;
  readonly restore?: readonly string[];
  readonly destroy?: readonly string[];
}

const trashIds = z
  .array(z.string())
  .nullish()
  .transform((ids) => ids ?? []);
const trashErrors = z
  .record(z.string(), SetErrorSchema)
  .nullish()
  .transform((record) => record ?? {});

const TrashSetSchema = z.looseObject({
  accountId: AccountId,
  destroyed: trashIds,
  notDestroyed: trashErrors,
  notRestored: trashErrors,
  restored: trashIds,
});
export type CalendarTrashSetResponse = z.infer<typeof TrashSetSchema>;

export const BusyPeriodSchema = z.looseObject({
  /** "confirmed" | "tentative" | "unavailable" */
  busyStatus: z.string().nullish(),
  utcEnd: z.string(),
  utcStart: z.string(),
});
export type BusyPeriod = z.infer<typeof BusyPeriodSchema>;

export interface PrincipalGetAvailabilityResponse {
  readonly list: BusyPeriod[];
}

export const ParticipantIdentitySchema = z.looseObject({
  calendarAddress: z.string(),
  id: z.string(),
  isDefault: z.boolean().nullish(),
  name: z.string().nullish(),
});
export type ParticipantIdentity = z.infer<typeof ParticipantIdentitySchema>;
export type ParticipantIdentityProperty =
  keyof typeof ParticipantIdentitySchema.shape;

// ---------------------------------------------------------------- methods

interface Spec<A, R> {
  readonly args: A;
  readonly response: R;
}

/** Merged into `MethodMap` (methods.ts). */
export interface CalendarMethodMap {
  "Calendar/get": Spec<
    CalendarGetArgs,
    GetResponse<Lenient<Calendar>, CalendarId>
  >;
  "Calendar/set": Spec<CalendarSetArgs, SetResponse<Calendar, CalendarId>>;
  "CalendarEvent/get": Spec<
    CalendarEventGetArgs,
    GetResponse<
      Lenient<CalendarEvent> & { id: CalendarEventId },
      CalendarEventId
    >
  >;
  "CalendarEvent/parse": Spec<
    CalendarEventParseArgs,
    CalendarEventParseResponse
  >;
  "CalendarEvent/counter": Spec<
    CalendarEventCounterArgs,
    CalendarItipSentResponse
  >;
  "CalendarEvent/declineCounter": Spec<
    CalendarEventDeclineCounterArgs,
    CalendarItipSentResponse
  >;
  "CalendarTrash/get": Spec<CalendarTrashGetArgs, CalendarTrashGetResponse>;
  "CalendarTrash/set": Spec<CalendarTrashSetArgs, CalendarTrashSetResponse>;
  "CalendarEvent/query": Spec<
    CalendarEventQueryArgs,
    QueryResponse<CalendarEventId>
  >;
  "CalendarEvent/set": Spec<
    CalendarEventSetArgs,
    SetResponse<CalendarEvent, CalendarEventId>
  >;
  "ParticipantIdentity/get": Spec<
    GetArgs<string, ParticipantIdentityProperty>,
    GetResponse<ParticipantIdentity, string>
  >;
  "Principal/getAvailability": Spec<
    PrincipalGetAvailabilityArgs,
    PrincipalGetAvailabilityResponse
  >;
}

const parser =
  <T>(schema: z.ZodType<T>) =>
  (_args: unknown, raw: Record<string, unknown>): T =>
    parseOrThrow(schema, raw);

const setSchema = <Shape extends z.ZodRawShape, I extends z.ZodType>(
  item: z.ZodObject<Shape, z.core.$loose>,
  id: I
) =>
  setResponseSchema(item.partial().extend({ id }), id).extend({
    updated: z
      .record(z.string(), item.partial().nullable())
      .nullish()
      .transform((record) => record ?? {}),
  });

const blobIds = z
  .array(BlobId)
  .nullish()
  .transform((ids) => ids ?? []);

const ParseResponseSchema = z.looseObject({
  accountId: AccountId,
  notFound: blobIds,
  notParsable: blobIds,
  // The draft has one object per blob; Stalwart sends an array.
  parsed: z
    .record(
      z.string(),
      z
        .union([
          z.array(ParsedEventSchema.partial()),
          ParsedEventSchema.partial(),
        ])
        .transform((value) => (Array.isArray(value) ? value : [value]))
    )
    .nullish()
    .transform((record) => record ?? {}),
});

const ParticipantIdentityGetSchema = getResponseSchema(
  ParticipantIdentitySchema,
  z.string()
);

const ItipSentSchema = z.looseObject({ accountId: AccountId });

const AvailabilitySchema = z.looseObject({
  list: z.array(BusyPeriodSchema),
});

const { availability, calendars, parse, principals } = CALENDAR_CAPABILITIES;

/** Merged into `METHODS` (methods.ts). */
export const CALENDAR_METHODS: {
  readonly [M in keyof CalendarMethodMap]: {
    readonly using: readonly string[];
    readonly parse: (
      args: Readonly<Record<string, unknown>>,
      raw: Record<string, unknown>
    ) => CalendarMethodMap[M]["response"];
  };
} = {
  "Calendar/get": {
    parse: parser(getResponseSchema(CalendarSchema.partial(), CalendarId)),
    using: [calendars],
  },
  "Calendar/set": {
    parse: parser(setSchema(CalendarSchema, CalendarId)),
    using: [calendars],
  },
  "CalendarEvent/get": {
    parse: parser(
      getResponseSchema(
        CalendarEventSchema.partial().extend({ id: CalendarEventId }),
        CalendarEventId
      )
    ),
    using: [calendars],
  },
  "CalendarEvent/parse": {
    parse: parser(ParseResponseSchema),
    using: [calendars, parse],
  },
  "CalendarEvent/counter": {
    parse: parser(ItipSentSchema),
    using: [calendars, CAPABILITIES.bridge],
  },
  "CalendarEvent/declineCounter": {
    parse: parser(ItipSentSchema),
    using: [calendars, CAPABILITIES.bridge],
  },
  "CalendarTrash/get": {
    parse: parser(TrashGetSchema),
    using: [calendars, CAPABILITIES.bridge],
  },
  "CalendarTrash/set": {
    parse: parser(TrashSetSchema),
    using: [calendars, CAPABILITIES.bridge],
  },
  "CalendarEvent/query": {
    parse: parser(queryResponseSchema(CalendarEventId)),
    using: [calendars],
  },
  "CalendarEvent/set": {
    parse: parser(setSchema(CalendarEventSchema, CalendarEventId)),
    using: [calendars],
  },
  "ParticipantIdentity/get": {
    parse: parser(ParticipantIdentityGetSchema),
    using: [calendars],
  },
  "Principal/getAvailability": {
    parse: parser(AvailabilitySchema),
    using: [principals, availability, calendars],
  },
};
