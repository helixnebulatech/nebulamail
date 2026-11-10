// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Calendar domain functions: the UI calls these, never JMAP.
// JMAP for Calendars as Stalwart implements it:
// occurrences come from the server (`expandRecurrences`), so no recurrence
// engine runs here, and scheduling email is sent by the server only when
// the user organizes the event (`sendSchedulingMessages`).

import type { CalendarDate } from "@internationalized/date";
import type {
  AccountId,
  CalendarEvent,
  CalendarEventId,
  CalendarId,
  CalendarPatch,
  CalendarRights,
  JmapClient,
  Lenient,
  PrincipalId,
} from "@nebulamail/jmap-client";
import {
  AccountId as AccountIdSchema,
  CAPABILITIES,
  CalendarId as CalendarIdSchema,
  hasAccountCapability,
  hasCapability,
} from "@nebulamail/jmap-client";

import { getClient } from "../mail/client";
import { calendarColor, isSafeColor } from "./colors";
import type { SettingsPatch } from "./defaults";
import { newCalendarSettings, readDefaultAlerts } from "./defaults";
import type { Me } from "./editing";
import { addressOf } from "./itip";
import { isEventObject, mapContext, toDetail, toOccurrence } from "./mapping";
import type { CalendarSummary, EventDetail, Occurrence } from "./model";
import { BRIDGE_COMPONENTS, calendarKey } from "./model";
import { publishingOf, subscriptionOf } from "./nextcloud";
import { expandWindows, maxExpandDays, nextPosition } from "./paging";
import { fromRule } from "./recurrence";
import type { Recurrence } from "./recurrence";
import { browserTimeZone } from "./time";

type Event = Lenient<CalendarEvent> & { readonly id: CalendarEventId };

/** A calendar or event the server refused to save (its SetError). */
export class EventSaveError extends Error {
  readonly type: string;
  readonly properties: readonly string[];
  constructor(
    type: string,
    description: string | null,
    properties: readonly string[] = []
  ) {
    super(description ?? type);
    this.name = "EventSaveError";
    this.type = type;
    this.properties = properties;
  }
}

/**
 * The bridge's refusal to delete with notice an event with more guests
 * than it mails a cancellation to (`MAX_RECIPIENTS`, 20). Deleting
 * without notice (`sendSchedulingMessages: false`) still works.
 */
export const isTooManyToNotify = (error: unknown): boolean =>
  error instanceof EventSaveError &&
  error.type === "forbidden" &&
  error.message.startsWith("too many participants to notify");

/**
 * Runs with the calendars account: `accountId`, or the user's own
 * (primary) one when null. Calendars shared by others live in their
 * owners' accounts.
 */
const withCalendars = async <T>(
  run: (client: JmapClient, accountId: AccountId) => Promise<T>,
  accountId: AccountId | null = null
): Promise<T> => {
  const client = await getClient();
  return await run(
    client,
    accountId ?? client.primaryAccountId(CAPABILITIES.calendars)
  );
};

/** Whether the server has calendars (the app rail hides the app if not). */
export const calendarsAvailable = async (): Promise<boolean> => {
  const client = await getClient();
  return hasCapability(client.session, CAPABILITIES.calendars);
};

/** The user's own calendars account. */
export const ownCalendarAccount = async (): Promise<AccountId> => {
  const client = await getClient();
  return client.primaryAccountId(CAPABILITIES.calendars);
};

/** What a calendar holds by the bridge's word (null: it says nothing). */
const componentsOf = (calendar: object): string[] | null => {
  const value: unknown = Reflect.get(calendar, BRIDGE_COMPONENTS);
  if (!Array.isArray(value)) {
    return null;
  }
  return value
    .filter((item): item is string => typeof item === "string")
    .map((item) => item.toUpperCase());
};

/** Every account in the session with calendars, the user's own first. */
const calendarAccounts = (client: JmapClient): AccountId[] => {
  const own = client.primaryAccountId(CAPABILITIES.calendars);
  const others = Object.keys(client.session.accounts)
    .map((id) => AccountIdSchema.parse(id))
    .filter(
      (id) =>
        id !== own &&
        hasAccountCapability(client.session, CAPABILITIES.calendars, id)
    );
  return [own, ...others];
};

/**
 * Calendars of every account: the user's own and those other people share
 * (Stalwart lists their accounts in the session even before sharing
 * anything; those answer `forbidden` and are skipped). `listCalendars`
 * with a given connection (Tasks, live tests).
 */
export const calendarsOf = async (
  client: JmapClient
): Promise<CalendarSummary[]> => {
  const own = client.primaryAccountId(CAPABILITIES.calendars);
  const accounts = calendarAccounts(client);
  // The bridge says what each calendar holds (BRIDGE_COMPONENTS) only
  // with its capability in `using` (RFC 8620 §1.8).
  const bridge = hasCapability(client.session, CAPABILITIES.bridge);
  const results = await client.requestSettled((b) => {
    if (bridge) {
      b.using(CAPABILITIES.bridge);
    }
    return accounts.map((accountId) => b.call("Calendar/get", { accountId }));
  });
  const summaries = accounts.flatMap((accountId, index) => {
    const result = results[index];
    if (result?.ok !== true) {
      if (accountId === own) {
        throw result?.error ?? new Error("No calendars");
      }
      return [];
    }
    const owner =
      accountId === own
        ? null
        : (client.session.accounts[accountId]?.name ?? accountId);
    return result.value.list.flatMap((calendar) => {
      const id = CalendarIdSchema.safeParse(calendar.id);
      if (!id.success) {
        return [];
      }
      return [
        {
          accountId,
          color: calendarColor(calendar.color, `${accountId}/${id.data}`),
          components: componentsOf(calendar),
          defaultAlertsWithTime: readDefaultAlerts(
            calendar.defaultAlertsWithTime
          ),
          defaultAlertsWithoutTime: readDefaultAlerts(
            calendar.defaultAlertsWithoutTime
          ),
          id: id.data,
          isDefault: owner === null && calendar.isDefault === true,
          key: calendarKey(accountId, id.data),
          mayDelete:
            calendar.isDefault !== true &&
            (owner === null || calendar.myRights?.mayDelete === true),
          mayWrite:
            calendar.myRights?.mayWriteAll === true ||
            calendar.myRights?.mayWriteOwn === true,
          mayShare: calendar.myRights?.mayShare === true,
          name: calendar.name ?? "",
          owner,
          publishing: publishingOf(calendar),
          subscription: subscriptionOf(calendar),
          sortOrder: calendar.sortOrder ?? 0,
          timeZone: calendar.timeZone ?? null,
          visible: calendar.isVisible !== false,
        },
      ];
    });
  });
  return summaries.toSorted(
    (a, b) =>
      Number(a.owner !== null) - Number(b.owner !== null) ||
      (a.owner ?? "").localeCompare(b.owner ?? "") ||
      Number(b.isDefault) - Number(a.isDefault) ||
      a.sortOrder - b.sortOrder ||
      a.name.localeCompare(b.name)
  );
};

/** Calendars of every account (`calendarsOf` this tab's connection). */
export const listCalendars = async (): Promise<CalendarSummary[]> =>
  await calendarsOf(await getClient());

/**
 * Shows or hides a calendar, renames it, changes its colour, time zone or
 * default alerts (stored on the server, so on every device). Only hex
 * colours are sent.
 */
export const updateCalendar = async (
  calendar: Pick<CalendarSummary, "id" | "accountId">,
  change: {
    readonly visible?: boolean;
    readonly color?: string;
    readonly name?: string;
    /** From `settingsPatch` (calendar/defaults.ts). */
    readonly settings?: SettingsPatch;
  }
): Promise<void> => {
  await withCalendars(async (client, accountId) => {
    const name = change.name?.trim();
    const patch: CalendarPatch = {
      ...change.settings,
      ...(change.visible === undefined ? {} : { isVisible: change.visible }),
      ...(change.color !== undefined && isSafeColor(change.color)
        ? { color: change.color }
        : {}),
      ...(name === undefined || name === "" ? {} : { name }),
    };
    if (Object.keys(patch).length === 0) {
      return;
    }
    const { id } = calendar;
    const [result] = await client.request((b) => [
      b.call("Calendar/set", { accountId, update: { [id]: patch } }),
    ]);
    const error = result.notUpdated[id];
    if (error !== undefined) {
      throw new EventSaveError(
        error.type,
        error.description ?? null,
        error.properties ?? []
      );
    }
  }, calendar.accountId);
};

/** Creates a calendar in the user's own account; returns its id. */
export const createCalendar = async (input: {
  readonly name: string;
  readonly color: string;
  /**
   * A task list only (["VTODO"]): only the bridge can say it (fixed once
   * created); other servers make an ordinary calendar.
   */
  readonly components?: readonly string[];
  /** Time zone and default alerts (`settingsPatch`). */
  readonly settings?: SettingsPatch;
  /**
   * Who gets it from the start (`shareWith` of the create; Stalwart and
   * the bridge take it). If the server refuses, nothing is
   * created.
   */
  readonly shareWith?: Readonly<Record<PrincipalId, CalendarRights>>;
}): Promise<CalendarId> =>
  await withCalendars(async (client, accountId) => {
    const name = input.name.trim();
    if (name === "") {
      throw new EventSaveError("invalidProperties", "A calendar needs a name");
    }
    const bridge =
      input.components !== undefined &&
      hasCapability(client.session, CAPABILITIES.bridge);
    const shareWith = input.shareWith ?? {};
    const shared = Object.keys(shareWith).length > 0;
    const [result] = await client.request((b) => {
      if (bridge) {
        b.using(CAPABILITIES.bridge);
      }
      if (shared) {
        b.using(CAPABILITIES.principals);
      }
      return [
        b.call("Calendar/set", {
          accountId,
          create: {
            calendar: {
              ...(bridge ? { [BRIDGE_COMPONENTS]: input.components } : {}),
              ...(shared ? { shareWith } : {}),
              ...newCalendarSettings(input.settings, browserTimeZone()),
              ...(isSafeColor(input.color) ? { color: input.color } : {}),
              isSubscribed: true,
              isVisible: true,
              name,
            },
          },
        }),
      ] as const;
    });
    const created = result.created.calendar;
    if (created === undefined) {
      const error = result.notCreated.calendar;
      throw new EventSaveError(
        error?.type ?? "unknown",
        error?.description ?? null,
        error?.properties ?? []
      );
    }
    return created.id;
  });

/** What a calendar holds besides its events (see `calendarContents`). */
export interface CalendarContents {
  /** Tasks (shown in Tasks, not in the calendar views). */
  readonly tasks: number;
  /**
   * Objects of other apps that Nebula Mail never shows: a journal entry
   * (`VJOURNAL`, RFC 5545 §3.6.3) has no JSCalendar type, so Stalwart and
   * the bridge give it as a `Group` (checked 2026-11-08).
   */
  readonly others: number;
  /** false: more than `MAX_CONTENTS` objects, counted up to there. */
  readonly complete: boolean;
}

/** Objects looked at when counting what a calendar holds. */
export const MAX_CONTENTS = 20_000;
const CONTENTS_PAGE = 500;

/**
 * Counts the tasks and the objects of other apps in a calendar, which
 * deleting it deletes too, so the dialog can say so. There is no filter by
 * `@type` (Stalwart: unsupportedFilter): the objects are listed with only
 * `@type`, page by page.
 */
export const calendarContents = async (
  calendar: Pick<CalendarSummary, "id" | "accountId">
): Promise<CalendarContents> =>
  await withCalendars(async (client, accountId) => {
    const page = Math.min(client.session.core.maxObjectsInGet, CONTENTS_PAGE);
    let tasks = 0;
    let others = 0;
    let position: number | null = 0;
    while (position !== null && position < MAX_CONTENTS) {
      const from: number = position;
      // Pages depend on the total of the first one: one at a time.
      // oxlint-disable-next-line no-await-in-loop
      const [query, objects] = await client.request((b) => {
        const q = b.call("CalendarEvent/query", {
          accountId,
          calculateTotal: true,
          filter: { inCalendar: calendar.id },
          limit: page,
          position: from,
        });
        const g = b.call("CalendarEvent/get", {
          "#ids": q.ref("/ids"),
          accountId,
          properties: ["@type"],
        });
        return [q, g] as const;
      });
      for (const item of objects.list) {
        if (item["@type"] === "Task") {
          tasks += 1;
        } else if (!isEventObject(item)) {
          others += 1;
        }
      }
      // By the server's own page (it may cap `limit`, RFC 8620 §5.5).
      position = nextPosition(query, page);
    }
    return { complete: position === null, others, tasks };
  }, calendar.accountId);

/**
 * Deletes a calendar and its events. The UI asks first; the user's
 * default calendar isn't offered (servers keep one).
 */
export const deleteCalendar = async (
  calendar: Pick<CalendarSummary, "id" | "accountId">
): Promise<void> => {
  await withCalendars(async (client, accountId) => {
    const { id } = calendar;
    const [result] = await client.request((b) => [
      b.call("Calendar/set", {
        accountId,
        destroy: [id],
        onDestroyRemoveEvents: true,
      }),
    ]);
    const error = result.notDestroyed[id];
    if (error !== undefined) {
      throw new EventSaveError(error.type, error.description ?? null);
    }
  }, calendar.accountId);
};

/**
 * The user's own calendar addresses: participant identities (Stalwart
 * makes one per address) plus the mail identities and login.
 */
export const listOwnAddresses = async (): Promise<string[]> => {
  const client = await getClient();
  const own = new Set<string>([client.session.username.toLowerCase()]);
  const [identities, mailIdentities] = await client.requestSettled(
    (b) =>
      [
        b.call("ParticipantIdentity/get", {
          accountId: client.primaryAccountId(CAPABILITIES.calendars),
        }),
        b.call("Identity/get", {
          accountId: client.primaryAccountId(),
          properties: ["email"],
        }),
      ] as const
  );
  const addresses = [
    ...(identities.ok
      ? identities.value.list.map((item) => item.calendarAddress)
      : []),
    ...(mailIdentities.ok
      ? mailIdentities.value.list.map((item) => item.email)
      : []),
  ];
  for (const raw of addresses) {
    const address = addressOf(raw);
    if (address !== null) {
      own.add(address);
    }
  }
  return [...own];
};

/** Who the user is as an organizer (default participant identity). */
export const me = async (): Promise<Me> =>
  await withCalendars(async (client, accountId) => {
    const [result] = await client.request((b) => [
      b.call("ParticipantIdentity/get", { accountId }),
    ]);
    const chosen =
      result.list.find((identity) => identity.isDefault === true) ??
      result.list[0];
    const email =
      addressOf(chosen?.calendarAddress) ??
      client.session.username.toLowerCase();
    return { email, name: chosen?.name ?? null };
  });

const OCCURRENCE_PROPERTIES = [
  // Tasks share calendars and queries with events: `toOccurrence` skips them.
  "@type",
  "calendarIds",
  "title",
  "start",
  "duration",
  // Stalwart 1.0 expands an event that doesn't repeat under its real id with
  // `baseEventId: null`, and the result reference below skips nulls: the
  // occurrence is then its own series, so it needs the series' fields too.
  "showWithoutTime",
  "timeZone",
  "recurrenceRule",
  "utcStart",
  "utcEnd",
  "baseEventId",
  "recurrenceId",
  "status",
  "participants",
  "organizerCalendarAddress",
  "locations",
  "isOrigin",
] as const;

const SERIES_PROPERTIES = [
  "@type",
  "calendarIds",
  "title",
  "showWithoutTime",
  "timeZone",
  "recurrenceRule",
  "organizerCalendarAddress",
  "participants",
  "status",
  "isOrigin",
] as const;

const MAX_GET = 500;
/**
 * Most occurrences one range may have: Stalwart and the bridge refuse more
 * (`cannotCalculateOccurrences`).
 */
const MAX_OCCURRENCES = 3000;

export interface OccurrenceQuery {
  readonly start: CalendarDate;
  readonly end: CalendarDate;
  readonly zone: string;
  readonly own: readonly string[];
  readonly calendars: readonly CalendarSummary[];
}

/**
 * One page of the occurrences in [start, end): the query and both gets in
 * a single request, chained by result references (RFC 8620 §3.7): the
 * ids of the page, then the series of those that repeat. The server pages
 * the expanded ids like any query (RFC 8620 §5.5, `position`/`limit`;
 * Stalwart and the bridge both do, checked 2026-11-08).
 */
const occurrencePage = async (
  client: JmapClient,
  accountId: AccountId,
  query: OccurrenceQuery,
  position: number,
  limit: number
) => {
  const [found, events, series] = await client.request((b) => {
    const q = b.call("CalendarEvent/query", {
      accountId,
      calculateTotal: true,
      expandRecurrences: true,
      filter: {
        after: `${query.start.toString()}T00:00:00`,
        before: `${query.end.toString()}T00:00:00`,
      },
      limit,
      position,
      sort: [{ isAscending: true, property: "start" }],
      timeZone: query.zone,
    });
    const get = b.call("CalendarEvent/get", {
      "#ids": q.ref("/ids"),
      accountId,
      properties: [...OCCURRENCE_PROPERTIES],
      timeZone: query.zone,
    });
    const bases = b.call("CalendarEvent/get", {
      "#ids": get.ref("/list/*/baseEventId"),
      accountId,
      properties: [...SERIES_PROPERTIES],
    });
    return [q, get, bases] as const;
  });
  return { events: events.list, found, series: series.list };
};

type OccurrencePage = Awaited<ReturnType<typeof occurrencePage>>;

/** The pages after the first one, if it wasn't all. */
const otherPages = async (
  client: JmapClient,
  accountId: AccountId,
  query: OccurrenceQuery,
  first: OccurrencePage,
  limit: number
): Promise<OccurrencePage[]> => {
  // The rest at once, if the first page wasn't all (the server may give
  // fewer than asked: its pages are as long as the first one). The
  // client queues them under `maxConcurrentRequests`.
  const step = first.found.ids.length;
  const { total } = first.found;
  const rest: OccurrencePage[] = [];
  // Only a full page may have more after it: as many as asked, or as
  // the server's own `limit` says it gave (RFC 8620 §5.5). A server that
  // gave more than asked gave them all.
  const full = step > 0 && step === (first.found.limit ?? limit);
  if (full && total === undefined) {
    // No total (RFC 8620 §5.5 allows it): page after page until a short
    // one, or one that repeats ids (a server that ignores `position`).
    const listed = new Set(first.found.ids);
    let position = step;
    let last = first;
    while (last.found.ids.length === step && position < MAX_OCCURRENCES) {
      // oxlint-disable-next-line no-await-in-loop -- each page says whether there is another.
      last = await occurrencePage(client, accountId, query, position, step);
      if (last.found.ids.some((id) => listed.has(id))) {
        break;
      }
      for (const id of last.found.ids) {
        listed.add(id);
      }
      rest.push(last);
      position += step;
    }
  } else if (total !== undefined && step > 0) {
    // The total says what is left, whatever the length of the pages.
    const positions: number[] = [];
    for (
      let position = step;
      position < Math.min(total, MAX_OCCURRENCES);
      position += step
    ) {
      positions.push(position);
    }
    rest.push(
      ...(await Promise.all(
        positions.map(
          async (position) =>
            await occurrencePage(client, accountId, query, position, step)
        )
      ))
    );
  }
  return rest;
};

const listAccountOccurrences = async (
  query: OccurrenceQuery,
  account: AccountId
): Promise<Occurrence[]> =>
  await withCalendars(async (client, accountId) => {
    // No window longer than the server expands (`maxExpandedQueryDuration`,
    // else `expandDurationTooLarge`, draft-ietf-jmap-calendars §5.11): a
    // longer range goes as several, each listed alone; an occurrence that
    // spans two of them once.
    const windows = expandWindows(
      query.start,
      query.end,
      maxExpandDays(client, accountId)
    );
    if (windows.length > 1) {
      const lists = await Promise.all(
        windows.map(
          async ([start, end]) =>
            await listAccountOccurrences({ ...query, end, start }, accountId)
        )
      );
      const once = new Map<string, Occurrence>();
      for (const occurrence of lists.flat()) {
        once.set(occurrence.id, occurrence);
      }
      return [...once.values()];
    }
    const limit = Math.min(MAX_GET, client.session.core.maxObjectsInGet);
    const first = await occurrencePage(client, accountId, query, 0, limit);
    const rest = await otherPages(client, accountId, query, first, limit);
    const context = mapContext(
      query.zone,
      query.own,
      query.calendars,
      accountId
    );
    const byId = new Map<string, Event>();
    for (const page of [first, ...rest]) {
      for (const event of page.series) {
        byId.set(event.id, event);
      }
    }
    const occurrences: Occurrence[] = [];
    // An occurrence on two pages (something changed between them) once.
    const seen = new Set<string>();
    for (const page of [first, ...rest]) {
      for (const event of page.events) {
        if (seen.has(event.id)) {
          continue;
        }
        seen.add(event.id);
        const mapped = toOccurrence(
          event,
          byId.get(event.baseEventId ?? event.id),
          context
        );
        if (mapped !== null) {
          occurrences.push(mapped);
        }
      }
    }
    return occurrences;
  }, account);

/**
 * Every occurrence in [start, end) of the user's zone, in all calendars of
 * every account in `query.calendars` (own and shared). An account that
 * fails (sharing just revoked) is left out; the user's own must answer.
 */
export const listOccurrences = async (
  query: OccurrenceQuery
): Promise<Occurrence[]> => {
  const own = await ownCalendarAccount();
  const accounts = [
    own,
    ...new Set(
      query.calendars
        .map((calendar) => calendar.accountId)
        .filter((accountId) => accountId !== own)
    ),
  ];
  const lists = await Promise.all(
    accounts.map(async (accountId) => {
      try {
        return await listAccountOccurrences(query, accountId);
      } catch (error) {
        if (accountId === own) {
          throw error;
        }
        return [];
      }
    })
  );
  return lists.flat().toSorted((a, b) => a.start - b.start);
};

const DETAIL_PROPERTIES = [
  ...OCCURRENCE_PROPERTIES,
  "uid",
  "description",
  "descriptionContentType",
  "virtualLocations",
  "alerts",
  "sequence",
  "showWithoutTime",
  "timeZone",
  "recurrenceRule",
] as const;

export interface EventWithSeries {
  readonly detail: EventDetail;
  readonly recurrence: Recurrence;
  /** The series' participants as stored (to keep keys when editing). */
  readonly participants: Event["participants"];
  /**
   * This occurrence's participants (with answers given only to it), else
   * the series'. A change to one occurrence stores them in its override:
   * Stalwart only applies a REPLY to an occurrence whose override has its
   * own participants.
   */
  readonly occurrenceParticipants: Event["participants"];
  /** The series' own local start (to move a whole series). */
  readonly seriesStart: string | null;
  /**
   * Alerts and locations as stored (this occurrence's, else the series',
   * like the detail shows them): saving keeps what the form can't show.
   */
  readonly alerts: Event["alerts"];
  readonly locations: Event["locations"];
}

/** One occurrence (or event) with its series; null if it's gone. */
export const getEvent = async (
  id: CalendarEventId,
  baseId: CalendarEventId,
  zone: string,
  own: readonly string[],
  calendars: readonly CalendarSummary[],
  account: AccountId | null = null
): Promise<EventWithSeries | null> =>
  await withCalendars(async (client, accountId) => {
    const ids = id === baseId ? [id] : [id, baseId];
    const [result] = await client.request((b) => [
      b.call("CalendarEvent/get", {
        accountId,
        ids,
        properties: [...DETAIL_PROPERTIES],
        timeZone: zone,
      }),
    ]);
    const base = result.list.find((event) => event.id === baseId);
    const event = result.list.find((item) => item.id === id) ?? base;
    if (event === undefined) {
      return null;
    }
    const detail = toDetail(
      event,
      base,
      mapContext(zone, own, calendars, accountId)
    );
    return detail === null
      ? null
      : {
          alerts: event.alerts === undefined ? base?.alerts : event.alerts,
          detail,
          locations:
            event.locations === undefined ? base?.locations : event.locations,
          occurrenceParticipants: event.participants ?? base?.participants,
          participants: base?.participants ?? event.participants,
          recurrence: fromRule(
            base?.recurrenceRule ?? null,
            (base ?? event).showWithoutTime === true
          ),
          seriesStart: base?.start ?? null,
        };
  }, account);

/** Creates an event (in `account`, default the user's own); returns its id. */
export const createEvent = async (
  properties: Record<string, unknown>,
  notify: boolean,
  account: AccountId | null = null
): Promise<CalendarEventId> =>
  await withCalendars(async (client, accountId) => {
    const { calendarIds } = properties;
    if (typeof calendarIds !== "object" || calendarIds === null) {
      throw new Error("An event needs a calendar");
    }
    const [result] = await client.request((b) => [
      b.call("CalendarEvent/set", {
        accountId,
        create: {
          event: {
            ...properties,
            // Checked just above; the editor always sets exactly one.
            // oxlint-disable-next-line typescript/no-unsafe-type-assertion
            calendarIds: calendarIds as Record<CalendarId, true>,
          },
        },
        sendSchedulingMessages: notify,
      }),
    ]);
    const created = result.created.event;
    if (created === undefined) {
      const error = result.notCreated.event;
      throw new EventSaveError(
        error?.type ?? "unknown",
        error?.description ?? null,
        error?.properties ?? []
      );
    }
    return created.id;
  }, account);

/** Updates an event or one occurrence (`id`) with a patch. */
export const updateEvent = async (
  id: CalendarEventId,
  patch: Record<string, unknown>,
  notify: boolean,
  account: AccountId | null = null
): Promise<void> => {
  await withCalendars(async (client, accountId) => {
    const [result] = await client.request((b) => [
      b.call("CalendarEvent/set", {
        accountId,
        sendSchedulingMessages: notify,
        update: { [id]: patch },
      }),
    ]);
    const error = result.notUpdated[id];
    if (error !== undefined) {
      throw new EventSaveError(
        error.type,
        error.description ?? null,
        error.properties ?? []
      );
    }
  }, account);
};

/** Deletes an event, or one occurrence of a series (`id` of it). */
export const deleteEvent = async (
  id: CalendarEventId,
  notify: boolean,
  account: AccountId | null = null
): Promise<void> => {
  await withCalendars(async (client, accountId) => {
    const [result] = await client.request((b) => [
      b.call("CalendarEvent/set", {
        accountId,
        destroy: [id],
        sendSchedulingMessages: notify,
      }),
    ]);
    const error = result.notDestroyed[id];
    if (error !== undefined) {
      throw new EventSaveError(
        error.type,
        error.description ?? null,
        error.properties ?? []
      );
    }
  }, account);
};

/** Stored events with this UID (an invitation already in the calendar). */
export const findByUid = async (
  uid: string
): Promise<
  {
    readonly id: CalendarEventId;
    readonly participants: Event["participants"];
    readonly organizerCalendarAddress: string | null;
    readonly status: string | null;
    readonly sequence: number;
    readonly start: string | null;
    readonly duration: string | null;
    readonly timeZone: string | null;
    readonly showWithoutTime: boolean;
  }[]
> =>
  await withCalendars(async (client, accountId) => {
    const [, result] = await client.request((b) => {
      const query = b.call("CalendarEvent/query", {
        accountId,
        filter: { uid },
      });
      const get = b.call("CalendarEvent/get", {
        "#ids": query.ref("/ids"),
        accountId,
        properties: [
          "@type",
          "uid",
          "participants",
          "organizerCalendarAddress",
          "status",
          "sequence",
          "start",
          "duration",
          "timeZone",
          "showWithoutTime",
        ],
      });
      return [query, get] as const;
    });
    // `uid` matches words for some servers: check it exactly. A task with
    // the same UID is not the invitation's event.
    return result.list
      .filter((event) => event.uid === uid && isEventObject(event))
      .map((event) => ({
        id: event.id,
        organizerCalendarAddress: event.organizerCalendarAddress ?? null,
        participants: event.participants,
        sequence: event.sequence ?? 0,
        duration: event.duration ?? null,
        start: event.start ?? null,
        showWithoutTime: event.showWithoutTime === true,
        status: event.status ?? null,
        timeZone: event.timeZone ?? null,
      }));
  });

export interface Busy {
  readonly start: number;
  readonly end: number;
  readonly tentative: boolean;
}

/**
 * Busy times of people on this server, by address. null for someone who
 * isn't a user here; an empty list may also mean they don't share their
 * availability with the user (Stalwart answers the same).
 */
export const getAvailability = async (
  emails: readonly string[],
  from: number,
  to: number
): Promise<ReadonlyMap<string, readonly Busy[] | null>> => {
  const client = await getClient();
  const result = new Map<string, readonly Busy[] | null>();
  if (
    !hasCapability(client.session, CAPABILITIES.principals) ||
    !hasCapability(
      client.session,
      "urn:ietf:params:jmap:principals:availability"
    )
  ) {
    return result;
  }
  const accountId = client.primaryAccountId(CAPABILITIES.principals);
  const lookups = await client.requestSettled((b) =>
    emails.map((email) =>
      b.call("Principal/query", { accountId, filter: { email }, limit: 1 })
    )
  );
  const principals = emails.map((email, index) => {
    const lookup = lookups[index];
    return {
      email,
      id: lookup?.ok === true ? lookup.value.ids[0] : undefined,
    };
  });
  const known = principals.filter(
    (
      principal
    ): principal is { email: string; id: NonNullable<typeof principal.id> } =>
      principal.id !== undefined
  );
  for (const principal of principals) {
    if (principal.id === undefined) {
      result.set(principal.email, null);
    }
  }
  if (known.length === 0) {
    return result;
  }
  const utcStart = new Date(from).toISOString().replace(/\.\d{3}Z$/u, "Z");
  const utcEnd = new Date(to).toISOString().replace(/\.\d{3}Z$/u, "Z");
  const answers = await client.requestSettled((b) =>
    known.map((principal) =>
      b.call("Principal/getAvailability", {
        accountId,
        id: principal.id,
        utcEnd,
        utcStart,
      })
    )
  );
  for (const [index, principal] of known.entries()) {
    const answer = answers[index];
    result.set(
      principal.email,
      answer?.ok === true
        ? answer.value.list.flatMap((period) => {
            const start = Date.parse(period.utcStart);
            const end = Date.parse(period.utcEnd);
            return Number.isFinite(start) && Number.isFinite(end)
              ? [{ end, start, tentative: period.busyStatus === "tentative" }]
              : [];
          })
        : null
    );
  }
  return result;
};

/** A local start one hour from the next half hour, for "new event". */
export const nextSlot = (now: number, zone: string): string => {
  const date = new Date(now + 30 * 60_000);
  const parts = new Intl.DateTimeFormat("en-CA", {
    day: "2-digit",
    hour: "2-digit",
    hourCycle: "h23",
    minute: "2-digit",
    month: "2-digit",
    timeZone: zone,
    year: "numeric",
  }).formatToParts(date);
  const value = (type: string): string =>
    parts.find((part) => part.type === type)?.value ?? "00";
  const minute = Number(value("minute")) < 30 ? "00" : "30";
  return `${value("year")}-${value("month")}-${value("day")}T${value("hour")}:${minute}:00`;
};
