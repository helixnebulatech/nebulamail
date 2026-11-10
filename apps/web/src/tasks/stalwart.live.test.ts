// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Tasks against a real Stalwart. Runs the app's own adapter and patches (adapter.ts, editing.ts)
// and reads back over JMAP and over Stalwart's CalDAV, to check what is
// stored as VTODO. It also pins down the Stalwart quirks the code works
// around: when one of those tests fails, Stalwart changed and the
// workaround can go. Skipped unless these are set (password in
// docker/.env); the same variables as calendar/occurrence.live.test.ts:
//
//   CAL_LIVE_URL=http://localhost:8080 \
//   CAL_LIVE_USER=alice@nebulamail.test CAL_LIVE_PASSWORD=… bun run test
//
// Everything it creates (one calendar and its tasks) is deleted again.

import { setTimeout as sleep } from "node:timers/promises";

import type {
  AccountId,
  CalendarEventId,
  FetchLike,
  JmapClient,
} from "@nebulamail/jmap-client";
import { CalendarId, JmapClient as Client } from "@nebulamail/jmap-client";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

import type { CalendarSummary } from "../calendar/model";
import { calendarKey } from "../calendar/model";
import { dateInZone } from "../calendar/time";
import type { TaskSnapshot } from "./adapter";
import { calendarTaskAdapter, storedKey } from "./adapter";
import type { TaskChange } from "./editing";
import {
  completeChange,
  estimatePatch,
  newTaskProperties,
  sortOrderPatch,
  titlePatch,
  undoOf,
} from "./editing";
import type { RawTask, Task } from "./model";

const env = {
  password: process.env.CAL_LIVE_PASSWORD ?? "",
  url: process.env.CAL_LIVE_URL ?? "",
  user: process.env.CAL_LIVE_USER ?? "",
};
const configured = Object.values(env).every((value) => value !== "");

const authorization = (): string =>
  `Basic ${btoa(`${env.user}:${env.password}`)}`;

/** Stalwart takes basic auth too; swap the client's Bearer header for it. */
const basicFetch: FetchLike = async (input, init) => {
  const headers = new Headers(init?.headers);
  headers.set("Authorization", authorization());
  return await fetch(input, { ...init, headers });
};

const ZONE = "Europe/Madrid";
const MARKER = `nebulatasks${Date.now().toString(36)}`;
const DAY_MS = 86_400_000;

let client: JmapClient;
let accountId: AccountId;
let calendarId: CalendarId;
/** The calendar's CalDAV collection ("/dav/cal/<user>/<name>/"). */
let davPath = "";

const day = (offset: number): string =>
  dateInZone(Date.now() + offset * DAY_MS, ZONE).toString();

const summary = (): CalendarSummary => ({
  accountId,
  color: "#2353C9",
  id: calendarId,
  isDefault: false,
  key: calendarKey(accountId, calendarId),
  mayDelete: true,
  mayWrite: true,
  name: MARKER,
  owner: null,
  sortOrder: 0,
  visible: true,
});

const adapter = calendarTaskAdapter({
  calendars: async () => await Promise.resolve([summary()]),
  client: async () => await Promise.resolve(client),
  savedLists: () => [],
});

const load = async (): Promise<TaskSnapshot> => {
  // Stalwart indexes a moment after a write.
  await sleep(300);
  return await adapter.load({ display: ZONE, now: Date.now() });
};

const byTitle = (snapshot: TaskSnapshot, title: string): Task[] =>
  snapshot.tasks.filter((task) => task.title === title);

const one = (snapshot: TaskSnapshot, title: string): Task => {
  const [task] = byTitle(snapshot, title);
  if (task === undefined) {
    throw new Error(`No task "${title}"`);
  }
  return task;
};

const storedOf = (snapshot: TaskSnapshot, task: Task): RawTask => {
  const raw = snapshot.stored.get(storedKey(task.accountId, task.baseId));
  if (raw === undefined) {
    throw new Error(`Not stored: ${task.title}`);
  }
  return raw;
};

/** Runs a change with the snapshot it was built from; answers its undo. */
const run = async (
  change: TaskChange,
  snapshot: TaskSnapshot
): Promise<TaskChange> => {
  const before = new Map<CalendarEventId, RawTask>(
    [...snapshot.stored.values()].map((raw) => [raw.id, raw])
  );
  const created = await adapter.apply(change);
  return undoOf(change, before, created);
};

const dav = async (
  method: string,
  path: string,
  body: string | null = null,
  headers: Record<string, string> = {}
): Promise<{ status: number; text: string }> => {
  const response = await fetch(`${env.url}${path}`, {
    body,
    headers: {
      Authorization: authorization(),
      "Content-Type": "application/xml; charset=utf-8",
      ...headers,
    },
    method,
  });
  return { status: response.status, text: await response.text() };
};

/** Every VTODO of the test's calendar over CalDAV, unfolded. */
const davTodos = async (): Promise<string[]> => {
  const report = await dav(
    "REPORT",
    davPath,
    '<?xml version="1.0"?><c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><c:calendar-data/></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VTODO"/></c:comp-filter></c:filter></c:calendar-query>',
    { Depth: "1" }
  );
  return [
    ...report.text.matchAll(/BEGIN:VCALENDAR[\s\S]*?END:VCALENDAR/gu),
  ].map((match) =>
    match[0]
      .replaceAll("&#13;", "")
      .replaceAll("\r\n ", "")
      .replaceAll("\r", "")
  );
};

const davTodo = async (uid: string): Promise<string> => {
  const todos = await davTodos();
  const found = todos.find((ics) => ics.includes(`UID:${uid}\n`));
  if (found === undefined) {
    throw new Error(`No VTODO ${uid} over CalDAV`);
  }
  return found;
};

const create = async (
  key: string,
  properties: Record<string, unknown>
): Promise<CalendarEventId> => {
  const created = await adapter.apply({
    accountId,
    create: { [key]: properties },
  });
  const id = created[key];
  if (id === undefined) {
    throw new Error(`Not created: ${key}`);
  }
  return id;
};

const uid = (name: string): string => `${MARKER}-${name}`;

/** "YYYY-MM-DDTHH:MM:SS" of `ms` in `zone`. */
const wallTime = (ms: number, zone: string): string =>
  new Intl.DateTimeFormat("sv-SE", {
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    month: "2-digit",
    second: "2-digit",
    timeZone: zone,
    year: "numeric",
  })
    .format(ms)
    .replace(" ", "T");

/** Hours `zone` is ahead of UTC at noon of `date` (1 in Madrid's winter). */
const offsetHours = (date: string, zone = ZONE): number => {
  const noon = Date.parse(`${date}T12:00:00Z`);
  const wall = Date.parse(`${wallTime(noon, zone)}Z`);
  return Math.round((wall - noon) / 3_600_000);
};

const newTask = async (
  name: string,
  input: Omit<Parameters<typeof newTaskProperties>[0], "calendarId" | "title">,
  extra: Record<string, unknown> = {}
): Promise<CalendarEventId> =>
  await create(name, {
    ...newTaskProperties(
      { ...input, calendarId, title: name },
      { display: ZONE, uid: uid(name) }
    ),
    ...extra,
  });

const setZone = async (timeZone: string | null): Promise<void> => {
  await client.request((b) => [
    b.call("Calendar/set", {
      accountId,
      update: { [calendarId]: { timeZone } },
    }),
  ]);
};

/**
 * A daily series due on `ring`'s day, with an offset that rings at `ring`
 * if counted from that day's midnight in `zone`.
 */
const daySeries =
  (ring: number) =>
  async (name: string, zone: string): Promise<CalendarEventId> => {
    const today = wallTime(ring, zone).slice(0, 10);
    const midnight =
      Date.parse(`${today}T00:00:00Z`) - offsetHours(today, zone) * 3_600_000;
    const seconds = Math.round((ring - midnight) / 1000);
    return await create(name, {
      "@type": "Task",
      alerts: {
        a: {
          "@type": "Alert",
          action: "display",
          trigger: {
            "@type": "OffsetTrigger",
            offset: `PT${String(seconds)}S`,
            relativeTo: "end",
          },
        },
      },
      calendarIds: { [calendarId]: true },
      due: `${today}T00:00:00`,
      progress: "needs-action",
      recurrenceRule: {
        "@type": "RecurrenceRule",
        count: 2,
        frequency: "daily",
      },
      showWithoutTime: true,
      start: `${today}T00:00:00`,
      title: name,
      uid: uid(name),
    });
  };

/**
 * The tasks of `ids` whose CalendarAlert arrives over the event source
 * within `ms` (stops early once all have).
 */
const alertsFired = async (
  ids: readonly CalendarEventId[],
  ms: number
): Promise<Set<string>> => {
  const url = client.session.eventSourceUrl
    .replace("{types}", "CalendarAlert")
    .replace("{closeafter}", "no")
    .replace("{ping}", "30");
  const controller = new AbortController();
  const timer = setTimeout(() => {
    controller.abort();
  }, ms);
  const fired = new Set<string>();
  try {
    const response = await fetch(url, {
      headers: {
        Accept: "text/event-stream",
        Authorization: authorization(),
      },
      signal: controller.signal,
    });
    const reader = response.body?.getReader();
    if (reader === undefined) {
      throw new Error("No event stream");
    }
    const decoder = new TextDecoder();
    while (fired.size < ids.length) {
      // oxlint-disable-next-line no-await-in-loop -- reading a stream.
      const { done, value } = await reader.read();
      if (done) {
        break;
      }
      const text = decoder.decode(value);
      for (const id of ids) {
        if (text.includes(`"calendarEventId":"${id}"`)) {
          fired.add(id);
        }
      }
    }
  } catch {
    // Aborted: what fired so far counts.
  } finally {
    clearTimeout(timer);
    controller.abort();
  }
  return fired;
};

describe.skipIf(!configured)("live Stalwart tasks", () => {
  beforeAll(async () => {
    client = await Client.connect({
      fetch: basicFetch,
      getAccessToken: async () => await Promise.resolve("unused"),
      sessionUrl: `${env.url}/.well-known/jmap`,
    });
    accountId = client.primaryAccountId("urn:ietf:params:jmap:calendars");
    const [created] = await client.request((b) => [
      b.call("Calendar/set", { accountId, create: { list: { name: MARKER } } }),
    ]);
    calendarId = CalendarId.parse(created.created.list?.id);
    // The CalDAV collection of the calendar, by its name.
    const home = `/dav/cal/${encodeURIComponent(env.user)}/`;
    const listing = await dav(
      "PROPFIND",
      home,
      '<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop><d:displayname/></d:prop></d:propfind>',
      { Depth: "1" }
    );
    const href = new RegExp(
      `<D:href>([^<]+)</D:href><D:propstat><D:prop><D:displayname>${MARKER}<`,
      "u"
    ).exec(listing.text)?.[1];
    davPath = href ?? "";
  });

  afterAll(async () => {
    if (client === undefined || calendarId === undefined) {
      return;
    }
    await client.request((b) => [
      b.call("Calendar/set", {
        accountId,
        destroy: [calendarId],
        onDestroyRemoveEvents: true,
      }),
    ]);
  });

  it("finds its calendar over CalDAV, taking VTODO", () => {
    expect(davPath).toMatch(/^\/dav\/cal\//u);
  });

  it("stores tasks with and without dates, subtasks, priorities", async () => {
    await newTask("undated", { priority: "low" });
    await newTask("day", { due: { date: day(3), kind: "date" } });
    await newTask("timed", {
      alerts: [{ minutesBefore: 30 }],
      due: { kind: "datetime", local: `${day(4)}T17:30:00`, timeZone: ZONE },
      priority: "high",
    });
    await newTask("parent", {});
    await newTask("child", { parentUid: uid("parent") });
    // Keywords and an unknown iCalendar property, as other clients write.
    await newTask(
      "extras",
      {
        links: [
          { href: "mid:abc@example.com" },
          { href: "https://app.example/t" },
        ],
      },
      {
        iCalendar: {
          name: "vtodo",
          properties: [["x-apple-sort-order", {}, "integer", 12_345]],
        },
        keywords: { work: true },
      }
    );
    const snapshot = await load();
    expect(snapshot.lists.map((list) => list.key)).toStrictEqual([
      calendarKey(accountId, calendarId),
    ]);
    const timed = one(snapshot, "timed");
    expect({
      child: one(snapshot, "child").parentUid,
      day: one(snapshot, "day").due,
      extras: [
        one(snapshot, "extras").keywords,
        one(snapshot, "extras").sortOrder,
      ],
      timed: [timed.priority, timed.alerts.map((alert) => alert.minutesBefore)],
      undated: [
        one(snapshot, "undated").dueAt,
        one(snapshot, "undated").priority,
      ],
    }).toStrictEqual({
      child: uid("parent"),
      day: { date: day(3), kind: "date" },
      extras: [["work"], 12_345],
      timed: ["high", [30]],
      undated: [null, "low"],
    });
  });

  it("stores them as VTODO, as other CalDAV clients read them", async () => {
    const dayIcs = await davTodo(uid("day"));
    const timedIcs = await davTodo(uid("timed"));
    const childIcs = await davTodo(uid("child"));
    const extras = await davTodo(uid("extras"));
    const lines = [
      [dayIcs, `DUE;VALUE=DATE:${day(3).replaceAll("-", "")}`],
      [timedIcs, `DUE;TZID=${ZONE}:${day(4).replaceAll("-", "")}T173000`],
      [timedIcs, "PRIORITY:1"],
      [timedIcs, "BEGIN:VALARM\nACTION:DISPLAY\nTRIGGER;VALUE=DATE-TIME:"],
      [childIcs, `RELATED-TO;RELTYPE=PARENT:${uid("parent")}`],
      [extras, "X-APPLE-SORT-ORDER"],
      [extras, "CATEGORIES:work"],
    ] as const;
    expect(lines.filter(([ics, line]) => !ics.includes(line))).toStrictEqual(
      []
    );
    // One link as URL, the other as LINK (RFC 9253).
    expect(extras).toMatch(/\n(?:URL|LINK)[^:\n]*:mid:abc@example\.com\n/u);
  });

  it("completes a task with COMPLETED, and undoes it", async () => {
    let snapshot = await load();
    const task = one(snapshot, "undated");
    const undo = await run(
      completeChange(task, storedOf(snapshot, task), true, {
        now: Date.now(),
      }),
      snapshot
    );
    snapshot = await load();
    const done = one(snapshot, "undated");
    expect([
      done.completed,
      (done.completedAt ?? 0) > Date.now() - 120_000,
    ]).toStrictEqual([true, true]);
    const ics = await davTodo(uid("undated"));
    expect(ics).toMatch(/STATUS:COMPLETED[\s\S]*COMPLETED:\d{8}T\d{6}Z/u);
    await adapter.apply(undo);
    snapshot = await load();
    expect(one(snapshot, "undated").completed).toBeFalsy();
    await expect(davTodo(uid("undated"))).resolves.not.toContain("COMPLETED:");
  });

  it("completes one occurrence of a repeating task, the series stays", async () => {
    await newTask("weekly", {
      alerts: [{ minutesBefore: 10 }],
      due: { kind: "datetime", local: `${day(-7)}T09:00:00`, timeZone: ZONE },
      recurrenceRule: {
        "@type": "RecurrenceRule",
        count: 6,
        frequency: "weekly",
      },
    });
    let snapshot = await load();
    const occurrences = byTitle(snapshot, "weekly").toSorted(
      (a, b) => (a.dueAt ?? 0) - (b.dueAt ?? 0)
    );
    // Overdue, today and the next weeks; each with its own due (the same
    // wall time, also across a change of time).
    expect(occurrences.length).toBeGreaterThanOrEqual(5);
    expect(occurrences[1]?.due).toStrictEqual({
      kind: "datetime",
      local: `${day(0)}T09:00:00`,
      timeZone: ZONE,
    });
    const [first] = occurrences;
    if (first === undefined) {
      throw new Error("No occurrence");
    }
    expect(first.alerts.map((alert) => alert.minutesBefore)).toStrictEqual([
      10,
    ]);
    await run(
      completeChange(first, storedOf(snapshot, first), true, {
        now: Date.now(),
      }),
      snapshot
    );
    snapshot = await load();
    const after = byTitle(snapshot, "weekly");
    expect({
      done: after
        .filter((task) => task.completed)
        .map((task) => task.recurrenceId),
      open: after.filter((task) => !task.completed).length,
    }).toStrictEqual({
      done: [first.recurrenceId],
      open: occurrences.length - 1,
    });
    // CalDAV: the series and the completed instance.
    const ics = await davTodo(uid("weekly"));
    const parts = ics.split("BEGIN:VTODO").slice(1);
    expect({
      instances: parts
        .filter((part) => part.includes("RECURRENCE-ID;TZID=Europe/Madrid:"))
        .map((part) => part.includes("STATUS:COMPLETED")),
      series: parts.filter((part) => part.includes("RRULE:FREQ=WEEKLY;COUNT=6"))
        .length,
    }).toStrictEqual({ instances: [true], series: 1 });
  });

  it("[Stalwart quirk] keeps estimatedDuration and progressUpdated, but hides the task's own progressUpdated from a get that names it", async () => {
    // RFC 8984 §5.2.3 and §5.2.6.
    await newTask("estimate", {});
    let snapshot = await load();
    let task = one(snapshot, "estimate");
    await run(
      { accountId, update: { [task.baseId]: estimatePatch(90) } },
      snapshot
    );
    snapshot = await load();
    task = one(snapshot, "estimate");
    const minutes = task.estimatedMinutes;
    const now = Date.now();
    const stamp = new Date(now).toISOString().replace(/\.\d{3}Z$/u, "Z");
    await run(
      completeChange(task, storedOf(snapshot, task), true, { now }),
      snapshot
    );
    const ics = await davTodo(uid("estimate"));
    expect([
      minutes,
      ics.includes("ESTIMATED-DURATION:PT1H30M"),
      ics.includes(`JSPROP;JSPTR="progressUpdated":"${stamp}"`),
    ]).toStrictEqual([90, true, true]);
    snapshot = await load();
    task = one(snapshot, "estimate");
    // Asked by name (as the app does) it isn't there; a get of everything
    // has it. When this fails, Stalwart returns it and `inversePatch` may
    // restore it like any other property.
    expect(storedOf(snapshot, task).progressUpdated).toBeUndefined();
    const [all] = await client.request((b) => [
      b.call("CalendarEvent/get", { accountId, ids: [task.baseId] }),
    ]);
    expect(
      (all.list as readonly Record<string, unknown>[])[0]?.progressUpdated
    ).toBe(stamp);
    // Clearing the estimate (a null patch) removes ESTIMATED-DURATION.
    await run(
      { accountId, update: { [task.baseId]: estimatePatch(null) } },
      snapshot
    );
    snapshot = await load();
    expect(one(snapshot, "estimate").estimatedMinutes).toBeNull();
    await expect(davTodo(uid("estimate"))).resolves.not.toContain(
      "ESTIMATED-DURATION"
    );
  });

  it("dates the progress of one occurrence in its override", async () => {
    await newTask("dated-weekly", {
      due: { kind: "datetime", local: `${day(1)}T09:00:00`, timeZone: ZONE },
      recurrenceRule: {
        "@type": "RecurrenceRule",
        count: 3,
        frequency: "weekly",
      },
    });
    let snapshot = await load();
    const [first] = byTitle(snapshot, "dated-weekly").toSorted(
      (a, b) => (a.dueAt ?? 0) - (b.dueAt ?? 0)
    );
    if (first?.recurrenceId === null || first?.recurrenceId === undefined) {
      throw new Error("No occurrence");
    }
    const now = Date.now();
    const stamp = new Date(now).toISOString().replace(/\.\d{3}Z$/u, "Z");
    await run(
      completeChange(first, storedOf(snapshot, first), true, { now }),
      snapshot
    );
    snapshot = await load();
    const series = storedOf(snapshot, first);
    expect(
      series.recurrenceOverrides?.[first.recurrenceId]?.progressUpdated
    ).toBe(stamp);
    expect(
      byTitle(snapshot, "dated-weekly").filter((task) => task.completed)
    ).toHaveLength(1);
  });

  it("completes an all-day occurrence with an override, the series stays", async () => {
    await newTask("daily", {
      due: { date: day(-1), kind: "date" },
      recurrenceRule: {
        "@type": "RecurrenceRule",
        count: 4,
        frequency: "daily",
      },
    });
    let snapshot = await load();
    const occurrences = byTitle(snapshot, "daily").toSorted(
      (a, b) => (a.dueAt ?? 0) - (b.dueAt ?? 0)
    );
    expect(occurrences).toHaveLength(4);
    const [first] = occurrences;
    if (first === undefined) {
      throw new Error("No occurrence");
    }
    const undo = await run(
      completeChange(first, storedOf(snapshot, first), true, {
        now: Date.now(),
      }),
      snapshot
    );
    snapshot = await load();
    const after = byTitle(snapshot, "daily");
    expect({
      done: after
        .filter((task) => task.completed)
        .map((task) => task.recurrenceId),
      recurring: after.every((task) => task.recurring),
      total: after.length,
    }).toStrictEqual({
      done: [first.recurrenceId],
      recurring: true,
      total: 4,
    });
    const [stored] = await client.request((b) => [
      b.call("CalendarEvent/get", {
        accountId,
        ids: [first.baseId],
        properties: ["recurrenceRule"],
      }),
    ]);
    expect(stored.list[0]?.recurrenceRule).toMatchObject({
      count: 4,
      frequency: "daily",
    });
    // CalDAV: the series and the completed day, with its DTSTART (#2007).
    const ics = await davTodo(uid("daily"));
    const parts = ics.split("BEGIN:VTODO").slice(1);
    expect({
      instances: parts
        .filter((part) => part.includes("RECURRENCE-ID;VALUE=DATE:"))
        .map((part) => [
          part.includes("STATUS:COMPLETED"),
          part.includes("DTSTART;VALUE=DATE:"),
        ]),
      series: parts.filter((part) => part.includes("RRULE:FREQ=DAILY;COUNT=4"))
        .length,
    }).toStrictEqual({ instances: [[true, true]], series: 1 });
    await adapter.apply(undo);
    snapshot = await load();
    expect(
      byTitle(snapshot, "daily").filter((task) => task.completed)
    ).toHaveLength(0);
  });

  it("[Stalwart quirk] occurrences keep the series' due; a path into a missing map fails", async () => {
    const snapshot = await load();
    const [, weekly] = byTitle(snapshot, "weekly");
    if (weekly === undefined) {
      throw new Error("No occurrence");
    }
    const [result] = await client.request((b) => [
      b.call("CalendarEvent/get", {
        accountId,
        ids: [weekly.id],
        properties: ["due", "start"],
      }),
    ]);
    const [occurrence] = result.list;
    expect([
      occurrence?.due,
      occurrence?.start === occurrence?.due,
    ]).toStrictEqual([`${day(-7)}T09:00:00`, false]);
    const plain = one(snapshot, "parent");
    const [set] = await client.request((b) => [
      b.call("CalendarEvent/set", {
        accountId,
        update: { [plain.baseId]: { "iCalendar/properties": [] } },
      }),
    ]);
    // `invalidPatch`; `mapPatch` writes the whole map then.
    expect(set.notUpdated[plain.baseId]?.type).toBe("invalidPatch");
  });

  it("keeps what another client wrote when the app edits a task", async () => {
    const name = "caldav";
    const ics = [
      "BEGIN:VCALENDAR",
      "VERSION:2.0",
      "PRODID:-//Nebula Mail//live test//EN",
      "BEGIN:VTODO",
      `UID:${uid(name)}`,
      "DTSTAMP:20261027T100000Z",
      `SUMMARY:${name}`,
      "STATUS:COMPLETED",
      "COMPLETED:20261026T090000Z",
      "PRIORITY:5",
      `DUE;VALUE=DATE:${day(1).replaceAll("-", "")}`,
      "X-APPLE-SORT-ORDER:778899",
      "CLASS:PRIVATE",
      "URL:https://example.com/x",
      "BEGIN:VALARM",
      "ACTION:DISPLAY",
      "DESCRIPTION:Reminder",
      "TRIGGER;VALUE=DATE-TIME:20261028T080000Z",
      "END:VALARM",
      "END:VTODO",
      "END:VCALENDAR",
      "",
    ].join("\r\n");
    const put = await dav("PUT", `${davPath}${uid(name)}.ics`, ics, {
      "Content-Type": "text/calendar; charset=utf-8",
    });
    expect(put.status).toBe(201);
    let snapshot = await load();
    const task = one(snapshot, name);
    expect(task).toMatchObject({
      completed: true,
      completedAt: Date.UTC(2026, 9, 26, 9),
      priority: "medium",
      sortOrder: 778_899,
    });
    await run(
      { accountId, update: { [task.baseId]: titlePatch("caldav edited") } },
      snapshot
    );
    snapshot = await load();
    await run(
      completeChange(
        one(snapshot, "caldav edited"),
        storedOf(snapshot, one(snapshot, "caldav edited")),
        false,
        { now: Date.now() }
      ),
      snapshot
    );
    const after = await davTodo(uid(name));
    expect(
      [
        "SUMMARY:caldav edited",
        "STATUS:NEEDS-ACTION",
        "X-APPLE-SORT-ORDER:778899",
        "CLASS:PRIVATE",
        "URL:https://example.com/x",
        "DESCRIPTION:Reminder",
        "PRIORITY:5",
      ].filter((line) => !after.includes(line))
    ).toStrictEqual([]);
    expect(after).not.toContain("COMPLETED:");
  });

  it(
    "fires CalendarAlert for a task's alerts (absolute, and an offset from the due)",
    { timeout: 150_000 },
    async () => {
      const due = new Date(Date.now() + 75_000);
      const local = wallTime(due.getTime(), ZONE);
      const single = await newTask("alert-single", {
        alerts: [{ minutesBefore: 0 }],
        due: { kind: "datetime", local, timeZone: ZONE },
      });
      const repeating = await newTask("alert-repeating", {
        alerts: [{ minutesBefore: 0 }],
        due: { kind: "datetime", local, timeZone: ZONE },
        recurrenceRule: {
          "@type": "RecurrenceRule",
          count: 2,
          frequency: "daily",
        },
      });
      const fired = await alertsFired([single, repeating], 130_000);
      expect([...fired].toSorted()).toStrictEqual(
        [single, repeating].toSorted()
      );
    }
  );

  it("stores a day task's reminders: a time for one, offsets for a series", async () => {
    // "1 day before at 9:00" and "on the day at 9:00".
    const due = day(5);
    await newTask("day-reminder", {
      alerts: [{ minutesBefore: 900 }],
      due: { date: due, kind: "date" },
    });
    await newTask("day-reminder-series", {
      alerts: [{ minutesBefore: 900 }, { minutesBefore: -540 }],
      due: { date: due, kind: "date" },
      recurrenceRule: { "@type": "RecurrenceRule", frequency: "weekly" },
    });
    const snapshot = await load();
    const minutes = (title: string) =>
      byTitle(snapshot, title)[0]?.alerts.map((alert) => alert.minutesBefore);
    expect({
      series: minutes("day-reminder-series"),
      single: minutes("day-reminder"),
    }).toStrictEqual({ series: [900, -540], single: [900] });
    // Over CalDAV: what other clients read (9:00 in Madrid, the day before).
    const at =
      Date.parse(`${day(4)}T09:00:00Z`) - 3_600_000 * offsetHours(day(4));
    const single = await davTodo(uid("day-reminder"));
    expect(single).toContain(`DUE;VALUE=DATE:${due.replaceAll("-", "")}`);
    expect(single).toContain(
      `TRIGGER;VALUE=DATE-TIME:${new Date(at).toISOString().replaceAll(/[-:]|\.000/gu, "")}`
    );
    const series = await davTodo(uid("day-reminder-series"));
    expect(
      ["TRIGGER;RELATED=END:-PT15H", "TRIGGER;RELATED=END:PT9H"].filter(
        (line) => !series.includes(line)
      )
    ).toStrictEqual([]);
  });

  it("keeps the manual order (X-APPLE-SORT-ORDER) the app writes", async () => {
    await newTask("order-a", {});
    await newTask(
      "order-b",
      {},
      {
        iCalendar: {
          name: "vtodo",
          properties: [["x-apple-sort-order", {}, "integer", 5]],
        },
      }
    );
    let snapshot = await load();
    const a = one(snapshot, "order-a");
    const b = one(snapshot, "order-b");
    // Without `iCalendar` (the whole map) and with one (by path).
    const undo = await run(
      {
        accountId,
        update: {
          [a.baseId]: sortOrderPatch(storedOf(snapshot, a), 1024),
          [b.baseId]: sortOrderPatch(storedOf(snapshot, b), 2048),
        },
      },
      snapshot
    );
    snapshot = await load();
    expect([
      one(snapshot, "order-a").sortOrder,
      one(snapshot, "order-b").sortOrder,
    ]).toStrictEqual([1024, 2048]);
    // Stalwart adds the type: "X-APPLE-SORT-ORDER;VALUE=INTEGER:1024".
    await expect(davTodo(uid("order-a"))).resolves.toMatch(
      /\nX-APPLE-SORT-ORDER(?:;VALUE=INTEGER)?:1024\n/u
    );
    await expect(davTodo(uid("order-b"))).resolves.toMatch(
      /\nX-APPLE-SORT-ORDER(?:;VALUE=INTEGER)?:2048\n/u
    );
    await run(undo, snapshot);
    snapshot = await load();
    expect([
      one(snapshot, "order-a").sortOrder,
      one(snapshot, "order-b").sortOrder,
    ]).toStrictEqual([null, 5]);
  });

  it(
    "[Stalwart quirk] rings a day series' offsets from midnight UTC",
    { timeout: 150_000 },
    async () => {
      // Stalwart reads a floating time (a day's midnight) in the
      // calendar's zone for alarms, and UTC in a calendar without one (like this one;
      // the next test has one). Two daily series due today with an offset
      // that rings in ~75 s: counted from UTC midnight, and from Madrid's.
      const ring = Date.now() + 75_000;
      const series = daySeries(ring);
      const utc = await series("floating-utc", "UTC");
      const madrid = await series("floating-madrid", ZONE);
      const fired = await alertsFired([utc, madrid], 120_000);
      expect([...fired]).toStrictEqual([utc]);
    }
  );

  it(
    "rings a day series' offsets from midnight in the calendar's zone",
    { timeout: 150_000 },
    async () => {
      // Why the calendars and lists we create get the browser's zone.
      await setZone(ZONE);
      try {
        const ring = Date.now() + 75_000;
        const series = daySeries(ring);
        const utc = await series("zoned-utc", "UTC");
        const madrid = await series("zoned-madrid", ZONE);
        const fired = await alertsFired([utc, madrid], 120_000);
        expect([...fired]).toStrictEqual([madrid]);
      } finally {
        await setZone(null);
      }
    }
  );
});
