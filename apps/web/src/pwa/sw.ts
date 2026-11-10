// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The service worker of the installable app. Small on
// purpose, so it can be audited in a minute:
//
// - Caches ONLY this build's static files: the hashed JS/CSS/fonts listed
//   at build time (an exact allowlist) and the offline page.
//   Never API calls, mail, attachments, HTML pages, images from the proxy
//   or anything from another origin: those requests aren't even touched.
// - Pages: always from the network (they carry the user's data and a fresh
//   CSP nonce). Only when the network fails, the static offline page.
// - A new build is a new worker (the list changes): it takes over at once
//   and deletes the old caches. Pages never come from the cache, so there
//   is no old app to keep alive; the open tab gets a "reload" prompt.
//
// Built by pwa/plugin.ts into /sw.js (TypeScript removed, the two
// constants below filled in). No imports: it runs as a classic script.

/** Filled in at build time: the hashed files of this build, e.g. "/assets/app-1a2b.css". */
declare const SW_ASSETS: readonly string[];
/** Filled in at build time: changes with every build. */
declare const SW_VERSION: string;
/** Filled in at build time: where the app lives, "/" or "/nebulamail/". */
declare const SW_BASE: string;
/**
 * Filled in at build time: where the BFF hands out short tokens, or null in
 * the static build: its session is in the tabs' session
 * worker, out of reach here, so no JMAP.
 */
declare const SW_TOKEN_ENDPOINT: string | null;
/**
 * Filled in at build time: the JMAP session of the full BFF mode's proxy
 * (JMAP_PROXY=on, lib/jmap-proxy.ts), used when the token endpoint says
 * so; null in the static build.
 */
declare const SW_PROXY_SESSION: string | null;

// The parts of the service worker scope used here (the WebWorker lib
// can't be loaded next to the DOM lib of the app).
interface SwExtendableEvent extends Event {
  waitUntil: (promise: Promise<unknown>) => void;
}
interface SwFetchEvent extends SwExtendableEvent {
  readonly request: Request;
  readonly preloadResponse: Promise<Response | undefined>;
  respondWith: (response: Promise<Response>) => void;
}
interface SwScope {
  readonly location: Location;
  readonly registration: {
    readonly navigationPreload?: { enable: () => Promise<void> };
  };
  readonly clients: { claim: () => Promise<void> };
  skipWaiting: () => Promise<void>;
  addEventListener: ((
    type: "install" | "activate",
    listener: (event: SwExtendableEvent) => void
  ) => void) &
    ((type: "fetch", listener: (event: SwFetchEvent) => void) => void);
}

// oxlint-disable-next-line typescript/no-unsafe-type-assertion -- this file only runs as the service worker, whose global is a ServiceWorkerGlobalScope; the DOM lib types it as Window.
const scope = globalThis as unknown as SwScope;

const CACHE = `nebula-static-${SW_VERSION}`;
const OFFLINE_PAGE = `${SW_BASE}offline.html`;
/** Precached when the worker installs, so the offline page always works. */
const OFFLINE_FILES = [
  OFFLINE_PAGE,
  `${SW_BASE}offline.css`,
  `${SW_BASE}favicon.svg`,
];
const CACHEABLE = new Set([...SW_ASSETS, ...OFFLINE_FILES]);

scope.addEventListener("install", (event) => {
  event.waitUntil(
    (async () => {
      const cache = await caches.open(CACHE);
      // Fixed names (not hashed): skip the HTTP cache, which may hold a
      // copy from an older build (Stalwart serves them `immutable`).
      await cache.addAll(
        OFFLINE_FILES.map(
          (url) =>
            new Request(new URL(url, scope.location.href), { cache: "reload" })
        )
      );
      await scope.skipWaiting();
    })()
  );
});

scope.addEventListener("activate", (event) => {
  event.waitUntil(
    (async () => {
      const names = await caches.keys();
      await Promise.all(
        names
          .filter((name) => name !== CACHE)
          .map(async (name) => await caches.delete(name))
      );
      // Starts the page request while the worker boots (pages only).
      await scope.registration.navigationPreload?.enable();
      await scope.clients.claim();
    })()
  );
});

/** Static file of this build: cache first, then the network (and keep it). */
const staticFile = async (request: Request): Promise<Response> => {
  const cache = await caches.open(CACHE);
  const cached = await cache.match(request);
  if (cached !== undefined) {
    return cached;
  }
  const response = await fetch(request);
  // Only plain same-origin 200s: no redirects, errors or opaque answers.
  if (response.status === 200 && response.type === "basic") {
    await cache.put(request, response.clone());
  }
  return response;
};

/** A page: network only; the offline page when there is no network. */
const page = async (event: SwFetchEvent): Promise<Response> => {
  try {
    return (await event.preloadResponse) ?? (await fetch(event.request));
  } catch {
    const offline = await caches.match(OFFLINE_PAGE);
    return offline ?? Response.error();
  }
};

scope.addEventListener("fetch", (event) => {
  const { request } = event;
  const url = new URL(request.url);
  if (request.method !== "GET" || url.origin !== scope.location.origin) {
    // JMAP, the image proxy, uploads, POSTs: straight to the network.
    return;
  }
  if (request.mode === "navigate") {
    event.respondWith(page(event));
  } else if (CACHEABLE.has(url.pathname) && url.search === "") {
    event.respondWith(staticFile(request));
  }
  // Everything else (server functions, /auth/*, /api/*): not touched.
});

// ---------------------------------------------------------------- Web Push
//
// Pushes from the JMAP server (lib/web-push.ts), for
// tabs that are closed or hidden:
//
// - PushVerification: passed to the open tabs, which send the code back;
//   with none open, the worker sends it itself (when it can reach JMAP).
// - CalendarAlert: an event reminder. If a tab is visible it shows the
//   reminder itself (it gets the same alert over the event source);
//   otherwise a notification: the event's title and time, read with a
//   short token from the BFF (same origin, the session cookie), as the
//   tabs do; if that fails, a generic "Event reminder". Snooze and Dismiss
//   buttons write to the event the same way. Clicking opens the event.
//   A task's reminder says when it is due and offers "Mark
//   done" (written like the tabs do; a repeating all-day task opens Tasks
//   instead: completing one of its days needs the app) and Snooze;
//   clicking opens the task in Tasks.
// - StateChange with "EmailDelivery": new mail (see "New mail" below).
// - EmailPush (Stalwart 1.0): new mail with the emails inside (see "New
//   mail inside the push" below).
//
// The notification never includes descriptions, guests or mail bodies
//text from the server is stripped of control and invisible
// characters.

/** Filled in at build time: the worker's texts, by language. */
declare const SW_MESSAGES: Readonly<
  Record<string, Readonly<Record<string, string>>>
>;

interface SwClient {
  readonly url: string;
  readonly visibilityState?: string;
  readonly focused?: boolean;
  postMessage: (message: unknown) => void;
  focus?: () => Promise<SwClient>;
  navigate?: (url: string) => Promise<SwClient | null>;
}
interface SwNotification {
  readonly data: unknown;
  close: () => void;
}
interface SwPushEvent extends SwExtendableEvent {
  readonly data: { json: () => unknown } | null;
}
interface SwNotificationEvent extends SwExtendableEvent {
  readonly action: string;
  readonly notification: SwNotification;
}
interface SwPushScope {
  readonly registration: {
    showNotification: (
      title: string,
      options: Record<string, unknown>
    ) => Promise<void>;
  };
  readonly clients: {
    matchAll: (options: Record<string, unknown>) => Promise<SwClient[]>;
    openWindow: (url: string) => Promise<SwClient | null>;
  };
  addEventListener: ((
    type: "push",
    listener: (event: SwPushEvent) => void
  ) => void) &
    ((
      type: "notificationclick",
      listener: (event: SwNotificationEvent) => void
    ) => void);
}

// oxlint-disable-next-line typescript/no-unsafe-type-assertion -- same global as `scope`, with the push parts.
const pushScope = globalThis as unknown as SwPushScope;

/** Keep in sync with lib/web-push.ts. */
const PUSH_MESSAGE = "nebula-push";
const SNOOZE_MINUTES = 10;

/** "2026-10-26T20:49:00Z" (UTCDateTime, no milliseconds). */
const utcDateTime = (ms: number): string =>
  new Date(ms).toISOString().replace(/\.\d{3}Z$/u, "Z");

const text = (key: string): string => {
  const language = (globalThis.navigator.language || "en").toLowerCase();
  const table = SW_MESSAGES[language.split("-")[0] ?? "en"] ?? SW_MESSAGES.en;
  return table?.[key] ?? SW_MESSAGES.en?.[key] ?? key;
};

/** Untrusted text for a notification: no controls or invisible characters. */
const clean = (value: unknown, max: number): string =>
  typeof value === "string"
    ? value
        .replaceAll(/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/gu, " ")
        .replaceAll(/\s+/gu, " ")
        .trim()
        .slice(0, max)
    : "";

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null;

const ID = /^[\w-]{1,255}$/u;

interface Alert {
  readonly accountId: string;
  readonly calendarEventId: string;
  readonly alertId: string;
  readonly recurrenceId: string | null;
}

const readAlert = (value: unknown): Alert | null => {
  if (!isObject(value) || value["@type"] !== "CalendarAlert") {
    return null;
  }
  const { accountId, alertId, calendarEventId, recurrenceId } = value;
  return typeof accountId === "string" &&
    ID.test(accountId) &&
    typeof calendarEventId === "string" &&
    ID.test(calendarEventId) &&
    typeof alertId === "string" &&
    ID.test(alertId)
    ? {
        accountId,
        alertId,
        calendarEventId,
        recurrenceId: typeof recurrenceId === "string" ? recurrenceId : null,
      }
    : null;
};

/** A tab of this app (under SW_BASE), not some other page of the origin. */
const inScope = (tab: SwClient): boolean => {
  try {
    return new URL(tab.url).pathname.startsWith(SW_BASE);
  } catch {
    return false;
  }
};

const tabs = async (): Promise<SwClient[]> => {
  const all = await pushScope.clients.matchAll({
    includeUncontrolled: true,
    type: "window",
  });
  return all.filter(inScope);
};

/** JSON of a response, or null. */
const jsonOf = async (response: Response): Promise<unknown> =>
  response.ok ? ((await response.json()) as unknown) : null;

interface Connection {
  /** The session object (untrusted shape: read with care). */
  readonly session: Record<string, unknown>;
  readonly apiOrigin: string;
  /** One JMAP request; null on any failure. */
  readonly post: (
    calls: readonly unknown[],
    using: readonly string[]
  ) => Promise<unknown[] | null>;
}

/**
 * Full BFF mode (JMAP_PROXY=on): the token endpoint answers 404 with
 * this error, and JMAP goes through the proxy on this origin with the
 * session cookie and this header (lib/jmap-proxy.ts; no imports here).
 * `background`: a push is not the user using the app.
 */
const PROXY_TOKEN_ERROR = "jmap-proxy";
const PROXY_HEADERS: Readonly<Record<string, string>> = {
  "x-nebula-proxy": "background",
};

interface JmapAuth {
  readonly sessionUrl: string;
  readonly headers: Readonly<Record<string, string>>;
  readonly credentials: RequestCredentials;
}

/** How to reach JMAP: a short token, or the proxy; null on any failure. */
const jmapAuth = async (endpoint: string): Promise<JmapAuth | null> => {
  const tokenResponse = await fetch(endpoint, {
    credentials: "same-origin",
    method: "POST",
  });
  if (tokenResponse.status === 404) {
    const answer: unknown = await tokenResponse.json().catch(() => null);
    return SW_PROXY_SESSION !== null &&
      isObject(answer) &&
      answer.error === PROXY_TOKEN_ERROR
      ? {
          credentials: "same-origin",
          headers: PROXY_HEADERS,
          sessionUrl: new URL(SW_PROXY_SESSION, scope.location.origin).href,
        }
      : null;
  }
  const grant = await jsonOf(tokenResponse);
  if (
    !isObject(grant) ||
    typeof grant.accessToken !== "string" ||
    typeof grant.sessionUrl !== "string"
  ) {
    return null;
  }
  return {
    credentials: "omit",
    headers: { authorization: `Bearer ${grant.accessToken}` },
    sessionUrl: grant.sessionUrl,
  };
};

/** The session with a short token from the BFF (or its proxy); null on any failure. */
const connect = async (): Promise<Connection | null> => {
  if (SW_TOKEN_ENDPOINT === null) {
    return null;
  }
  try {
    const grant = await jmapAuth(SW_TOKEN_ENDPOINT);
    if (grant === null) {
      return null;
    }
    const { credentials, headers } = grant;
    const sessionResponse = await fetch(grant.sessionUrl, {
      credentials,
      headers,
    });
    const session = await jsonOf(sessionResponse);
    if (!isObject(session) || typeof session.apiUrl !== "string") {
      return null;
    }
    // The API must be on the session's origin (like the tabs' client).
    const sessionUrl = new URL(grant.sessionUrl);
    const apiUrl = new URL(session.apiUrl, sessionUrl);
    if (apiUrl.origin !== sessionUrl.origin) {
      return null;
    }
    const post = async (
      calls: readonly unknown[],
      using: readonly string[]
    ): Promise<unknown[] | null> => {
      try {
        const apiResponse = await fetch(apiUrl, {
          body: JSON.stringify({
            methodCalls: calls,
            using: ["urn:ietf:params:jmap:core", ...using],
          }),
          credentials,
          headers: { ...headers, "content-type": "application/json" },
          method: "POST",
          redirect: "error",
        });
        const response = await jsonOf(apiResponse);
        return isObject(response) && Array.isArray(response.methodResponses)
          ? response.methodResponses
          : null;
      } catch {
        return null;
      }
    };
    return { apiOrigin: apiUrl.origin, post, session };
  } catch {
    return null;
  }
};

/** A JMAP call with the session's short token; null on any failure. */
const jmap = async (
  calls: readonly unknown[],
  using: readonly string[]
): Promise<unknown[] | null> => {
  const connection = await connect();
  return connection === null ? null : await connection.post(calls, using);
};

const CALENDARS = ["urn:ietf:params:jmap:calendars"];

/** Epoch ms of a wall time in an IANA zone (NaN if the zone is unknown). */
const zonedToEpoch = (
  fields: readonly [number, number, number, number, number, number],
  zone: string
): number => {
  const [year, month, day, hour, minute, second] = fields;
  const wall = Date.UTC(year, month - 1, day, hour, minute, second);
  try {
    const format = new Intl.DateTimeFormat("en-US", {
      day: "numeric",
      hour: "numeric",
      hourCycle: "h23",
      minute: "numeric",
      month: "numeric",
      second: "numeric",
      timeZone: zone,
      year: "numeric",
    });
    // The zone's offset at an instant: its wall time there minus the instant.
    const offset = (at: number): number => {
      const part = (type: string): number =>
        Number(
          format.formatToParts(at).find((item) => item.type === type)?.value
        );
      return (
        Date.UTC(
          part("year"),
          part("month") - 1,
          part("day"),
          part("hour"),
          part("minute"),
          part("second")
        ) - at
      );
    };
    // Twice: the offset at the first guess can differ near a change of time.
    const guess = wall - offset(wall);
    return wall - offset(guess);
  } catch {
    return Number.NaN;
  }
};

/**
 * When the alert's occurrence starts. Its `recurrenceId` is a
 * LocalDateTime in the event's zone (Stalwart and the bridge)
 *  or, from older servers, a UTC time with `Z`; floating
 * and all-day events are in this device's zone. Same as `occurrenceStart`
 * in calendar/alerts.ts, without imports.
 */
const occurrenceStart = (recurrenceId: string, zone: string | null): number => {
  if (/(?:z|[+-]\d{2}:?\d{2})$/iu.test(recurrenceId)) {
    return Date.parse(recurrenceId);
  }
  const parts =
    /^(?<y>\d{4})-(?<mo>\d{2})-(?<d>\d{2})(?:T(?<h>\d{2}):(?<mi>\d{2})(?::(?<s>\d{2}))?)?$/u.exec(
      recurrenceId
    )?.groups;
  if (parts === undefined) {
    return Number.NaN;
  }
  const field = (name: string): number => Number(parts[name] ?? "0");
  const fields = [
    field("y"),
    field("mo"),
    field("d"),
    field("h"),
    field("mi"),
    field("s"),
  ] as const;
  if (zone === null) {
    return new Date(
      fields[0],
      fields[1] - 1,
      fields[2],
      fields[3],
      fields[4],
      fields[5]
    ).getTime();
  }
  return zonedToEpoch(fields, zone);
};

/**
 * When the alert's event starts, or (a task without a repetition) when
 * the task is due: wall times in their zone, all-day ones in this device's.
 */
const alertStart = (
  alert: Alert,
  event: Record<string, unknown>,
  task: boolean
): number => {
  const zone =
    typeof event.timeZone === "string" && event.showWithoutTime !== true
      ? event.timeZone
      : null;
  if (task && alert.recurrenceId === null && typeof event.due === "string") {
    return occurrenceStart(event.due, zone);
  }
  if (alert.recurrenceId !== null) {
    return occurrenceStart(alert.recurrenceId, zone);
  }
  return Date.parse(typeof event.utcStart === "string" ? event.utcStart : "");
};

/** The day a task without a time is due ("6 Nov 2026"); "" if unknown. */
const dueDayText = (alert: Alert, event: Record<string, unknown>): string => {
  const value = alert.recurrenceId ?? event.due;
  const day = typeof value === "string" ? value.slice(0, 10) : "";
  const ms = /^\d{4}-\d{2}-\d{2}$/u.test(day)
    ? Date.parse(`${day}T00:00:00Z`)
    : Number.NaN;
  return Number.isFinite(ms)
    ? new Intl.DateTimeFormat(globalThis.navigator.language, {
        dateStyle: "medium",
        timeZone: "UTC",
      }).format(ms)
    : "";
};

/** Title and "10:00" of the alert's event, if the worker can read them. */
const describeAlert = async (
  alert: Alert
): Promise<{
  readonly title: string;
  readonly body: string;
  readonly task: boolean;
}> => {
  const fallback = { body: "", task: false, title: text("cal_alert_generic") };
  const responses = await jmap(
    [
      [
        "CalendarEvent/get",
        {
          accountId: alert.accountId,
          ids: [alert.calendarEventId],
          properties: [
            "@type",
            "title",
            "utcStart",
            "due",
            "timeZone",
            "showWithoutTime",
          ],
        },
        "0",
      ],
    ],
    CALENDARS
  );
  const answer = responses?.[0];
  const list =
    Array.isArray(answer) && isObject(answer[1]) ? answer[1].list : null;
  const event: unknown = Array.isArray(list) ? list[0] : null;
  if (!isObject(event)) {
    return fallback;
  }
  const task = event["@type"] === "Task";
  const title =
    clean(event.title, 120) ||
    text(task ? "tasks_alert_generic" : "cal_alert_generic");
  const start = alertStart(alert, event, task);
  let body = "";
  if (event.showWithoutTime !== true && Number.isFinite(start)) {
    body = new Intl.DateTimeFormat(globalThis.navigator.language, {
      dateStyle: "medium",
      timeStyle: "short",
    }).format(start);
  } else if (task) {
    // A task due on a day: that day.
    body = dueDayText(alert, event);
  }
  return {
    body:
      task && body !== ""
        ? text("tasks_reminder_due").replace("{when}", body)
        : body,
    task,
    title,
  };
};

const alertTag = (alert: Alert): string =>
  [
    "wm-alert",
    alert.accountId,
    alert.calendarEventId,
    alert.alertId,
    alert.recurrenceId ?? "",
  ].join("|");

const eventUrl = (alert: Alert): string => {
  const search = new URLSearchParams({
    account: alert.accountId,
    event: alert.calendarEventId,
  });
  return `${SW_BASE}calendar?${search.toString()}`;
};

/** Tasks, with this task open (its account always named). */
const taskUrl = (alert: Alert): string => {
  const search = new URLSearchParams({
    task: `${alert.accountId}.${alert.calendarEventId}`,
  });
  return `${SW_BASE}tasks?${search.toString()}`;
};

/** The alert's buttons. Static build: none (no token here to write with). */
const alertActions = (
  task: boolean
): { readonly action: string; readonly title: string }[] => {
  if (SW_TOKEN_ENDPOINT === null) {
    return [];
  }
  return task
    ? [
        { action: "done", title: text("tasks_alert_done") },
        { action: "snooze", title: text("cal_alert_snooze_10") },
      ]
    : [
        { action: "snooze", title: text("cal_alert_snooze_10") },
        { action: "dismiss", title: text("cal_alert_dismiss") },
      ];
};

const showAlert = async (alert: Alert): Promise<void> => {
  const open = await tabs();
  if (open.some((tab) => tab.visibilityState === "visible")) {
    // A visible tab shows it itself (it gets it over the event source).
    return;
  }
  const { body, task, title } = await describeAlert(alert);
  await pushScope.registration.showNotification(title, {
    actions: alertActions(task),
    body,
    data: { ...alert, task },
    icon: `${SW_BASE}icons/icon-192.png`,
    requireInteraction: true,
    tag: alertTag(alert),
  });
};

// ------------------------------------------------- New mail (StateChange)
//
// A StateChange with "EmailDelivery" (RFC 8621 §1.5: changes only when
// mail is delivered) for the session's primary mail account. If this
// device wants new-mail notifications (the "on" flag the tabs write), ask
// `Email/changes` from the last state stored here, keep what notify.ts
// keeps (unread inbox mail, not junk, from the last 15 minutes) and show
// the sender and the subject, never the body. The stored state moves on
// only if it is still the one we started from (two pushes at once), and
// each email is claimed once (tabs claim too), so nothing shows twice.
// A visible tab: nothing (it shows new mail itself); the state still
// moves on. No stored state yet: learn the current one, announce nothing.
// The static build has no token here: one generic "New
// mail" instead (no sender, no subject), replaced by the next one.
//
// The IndexedDB lines below mirror mail/push-store.ts: keep them in sync.

const MAIL = ["urn:ietf:params:jmap:mail"];
const PUSH_DB = "nebula-mail-push";
const MAIL_MAX_AGE_MS = 15 * 60_000;
const MAIL_MAX_SINGLE = 3;
const MAIL_PAGE = 50;
const MAIL_PAGES = 4;

const idbSettle = async <T>(request: IDBRequest<T>): Promise<T> => {
  const { promise, reject, resolve } = Promise.withResolvers<T>();
  request.addEventListener("success", () => {
    resolve(request.result);
  });
  request.addEventListener("error", () => {
    reject(request.error ?? new Error("IndexedDB request failed"));
  });
  return await promise;
};

const idbDone = async (transaction: IDBTransaction): Promise<void> => {
  const { promise, reject, resolve } = Promise.withResolvers<undefined>();
  transaction.addEventListener("complete", () => {
    resolve(undefined);
  });
  transaction.addEventListener("abort", () => {
    reject(transaction.error ?? new Error("IndexedDB transaction aborted"));
  });
  await promise;
};

const withPushDb = async <T>(
  run: (db: IDBDatabase) => Promise<T>
): Promise<T> => {
  const request = indexedDB.open(PUSH_DB, 1);
  request.addEventListener("upgradeneeded", () => {
    for (const name of ["kv", "notified"]) {
      if (!request.result.objectStoreNames.contains(name)) {
        request.result.createObjectStore(name, { keyPath: "id" });
      }
    }
  });
  const db = await idbSettle(request);
  try {
    return await run(db);
  } finally {
    db.close();
  }
};

const kvValue = async (id: string): Promise<unknown> =>
  await withPushDb(async (db) => {
    const found: unknown = await idbSettle(
      db.transaction("kv").objectStore("kv").get(id)
    );
    return isObject(found) ? found.value : undefined;
  });

/** Stores `to` if the state is still `from`; whether it did. */
const advanceState = async (
  account: string,
  from: string | null,
  to: string
): Promise<boolean> =>
  await withPushDb(async (db) => {
    const transaction = db.transaction("kv", "readwrite");
    const store = transaction.objectStore("kv");
    const id = `state ${account}`;
    const found: unknown = await idbSettle(store.get(id));
    const current =
      isObject(found) && typeof found.value === "string" ? found.value : null;
    const moved = current === from;
    if (moved) {
      store.put({ id, value: to });
    }
    await idbDone(transaction);
    return moved;
  });

/** True the first time this email is announced on this device. */
const claimNotice = async (
  account: string,
  emailId: string
): Promise<boolean> =>
  await withPushDb(async (db) => {
    const transaction = db.transaction("notified", "readwrite");
    const store = transaction.objectStore("notified");
    const id = `${account} ${emailId}`;
    const found: unknown = await idbSettle(store.get(id));
    if (found === undefined) {
      store.put({ at: Date.now(), id });
    }
    await idbDone(transaction);
    return found === undefined;
  });

/** Accounts whose delivered mail changed, from a StateChange. */
const deliveries = (value: unknown): string[] => {
  if (
    !isObject(value) ||
    value["@type"] !== "StateChange" ||
    !isObject(value.changed)
  ) {
    return [];
  }
  return Object.entries(value.changed).flatMap(([accountId, types]) =>
    ID.test(accountId) &&
    isObject(types) &&
    typeof types.EmailDelivery === "string"
      ? [accountId]
      : []
  );
};

/** The response of one method call (by its call id), or null. */
const answerOf = (
  responses: readonly unknown[] | null,
  callId: string
): { readonly name: string; readonly args: Record<string, unknown> } | null => {
  for (const response of responses ?? []) {
    if (
      Array.isArray(response) &&
      response[2] === callId &&
      typeof response[0] === "string" &&
      isObject(response[1])
    ) {
      return { args: response[1], name: response[0] };
    }
  }
  return null;
};

const stringIds = (value: unknown): string[] =>
  Array.isArray(value)
    ? value.filter((id): id is string => typeof id === "string" && ID.test(id))
    : [];

interface ArrivedMail {
  readonly id: string;
  readonly title: string;
  readonly body: string;
}

/** What notify.ts keeps: unread inbox mail, not junk, recent. */
/**
 * A notification's text: the subject. Stalwart leaves a property out when
 * the push would be too big (a subject of thousands of characters), and
 * that's not "(no subject)".
 */
const mailBody = (email: Record<string, unknown>, title: string): string => {
  if ("subject" in email) {
    return clean(email.subject, 200) || text("row_no_subject");
  }
  return title === text("sw_mail_new") ? "" : text("sw_mail_new");
};

const worthAnnouncing = (
  list: unknown,
  inboxId: string,
  now: number
): ArrivedMail[] =>
  (Array.isArray(list) ? list : []).flatMap((email: unknown) => {
    if (
      !isObject(email) ||
      typeof email.id !== "string" ||
      !ID.test(email.id)
    ) {
      return [];
    }
    const keywords = isObject(email.keywords) ? email.keywords : {};
    const boxes = isObject(email.mailboxIds) ? email.mailboxIds : {};
    const received = Date.parse(
      typeof email.receivedAt === "string" ? email.receivedAt : ""
    );
    if (
      boxes[inboxId] !== true ||
      keywords.$seen === true ||
      keywords.$junk === true ||
      !Number.isFinite(received) ||
      now - received >= MAIL_MAX_AGE_MS
    ) {
      return [];
    }
    const { from } = email;
    const sender: unknown = Array.isArray(from) ? from.at(0) : null;
    const name = isObject(sender) ? clean(sender.name, 120) : "";
    const address = isObject(sender) ? clean(sender.email, 254) : "";
    const title = name || address || text("sw_mail_new");
    return [
      {
        body: mailBody(email, title),
        id: email.id,
        // No sender (left out of a full push): still a title.
        title,
      },
    ];
  });

/** The account's current Email state (to start from), or null. */
const currentState = async (
  connection: Connection,
  accountId: string
): Promise<string | null> => {
  const answer = answerOf(
    await connection.post([["Email/get", { accountId, ids: [] }, "s"]], MAIL),
    "s"
  );
  return typeof answer?.args.state === "string" ? answer.args.state : null;
};

/** Emails created since `since` and the inbox id; null: start over. */
const createdSince = async (
  connection: Connection,
  accountId: string,
  since: string
): Promise<{
  readonly state: string;
  readonly created: string[];
  readonly inboxId: string | null;
} | null> => {
  let state = since;
  let inboxId: string | null = null;
  const created: string[] = [];
  for (let round = 0; round < MAIL_PAGES; round += 1) {
    // oxlint-disable-next-line no-await-in-loop -- each page starts where the last ended.
    const responses = await connection.post(
      [
        ...(round === 0
          ? [["Mailbox/query", { accountId, filter: { role: "inbox" } }, "m"]]
          : []),
        [
          "Email/changes",
          { accountId, maxChanges: MAIL_PAGE, sinceState: state },
          "c",
        ],
      ],
      MAIL
    );
    if (round === 0) {
      [inboxId = null] = stringIds(answerOf(responses, "m")?.args.ids);
    }
    const changes = answerOf(responses, "c");
    if (
      changes?.name !== "Email/changes" ||
      typeof changes.args.newState !== "string"
    ) {
      // cannotCalculateChanges, an error or no answer.
      return null;
    }
    created.push(...stringIds(changes.args.created));
    state = changes.args.newState;
    if (changes.args.hasMoreChanges !== true) {
      break;
    }
  }
  return { created, inboxId, state };
};

const showMail = async (
  account: string,
  arrived: readonly ArrivedMail[]
): Promise<void> => {
  const claimed = await Promise.all(
    arrived.map(async (email) => await claimNotice(account, email.id))
  );
  const fresh = arrived.filter((_email, index) => claimed[index] === true);
  if (fresh.length > MAIL_MAX_SINGLE) {
    await pushScope.registration.showNotification(
      text("sw_mail_many").replace("{count}", String(fresh.length)),
      {
        data: { "@type": "MailNotice", emailId: null },
        icon: `${SW_BASE}icons/icon-192.png`,
        tag: "wm-new-mail",
      }
    );
    return;
  }
  for (const email of fresh) {
    // oxlint-disable-next-line no-await-in-loop -- at most 3, in order.
    await pushScope.registration.showNotification(email.title, {
      body: email.body,
      data: { "@type": "MailNotice", emailId: email.id },
      icon: `${SW_BASE}icons/icon-192.png`,
      // The same tag as the tabs' notification of this email (notify.ts).
      tag: `wm-${email.id}`,
    });
  }
};

/** Static build: "New mail", unless a tab is visible (it shows it). */
const announceMailGenerically = async (): Promise<void> => {
  const open = await tabs();
  if (open.some((tab) => tab.visibilityState === "visible")) {
    return;
  }
  await pushScope.registration.showNotification(text("sw_mail_new"), {
    data: { "@type": "MailNotice", emailId: null },
    icon: `${SW_BASE}icons/icon-192.png`,
    tag: "wm-new-mail",
  });
};

/** New mail for these accounts, and this device wants to hear of it. */
const wantsMailNotices = async (
  accounts: readonly string[]
): Promise<boolean> =>
  accounts.length > 0 &&
  typeof Notification !== "undefined" &&
  Notification.permission === "granted" &&
  (await kvValue("on").catch(() => false)) === true;

const announceMail = async (accounts: readonly string[]): Promise<void> => {
  const connection = await connect();
  if (connection === null) {
    return;
  }
  const { primaryAccounts } = connection.session;
  const primary = isObject(primaryAccounts)
    ? primaryAccounts[MAIL[0] ?? ""]
    : null;
  if (typeof primary !== "string" || !accounts.includes(primary)) {
    return;
  }
  const username =
    typeof connection.session.username === "string"
      ? connection.session.username
      : "";
  // Same key as mail/push-store.ts `pushAccountKey`.
  const account = `${connection.apiOrigin} ${username} ${primary}`;
  const stored = await kvValue(`state ${account}`);
  const since = typeof stored === "string" ? stored : null;
  const found =
    since === null ? null : await createdSince(connection, primary, since);
  if (found === null) {
    // No state yet, or too old to compare: start from now, quietly.
    const state = await currentState(connection, primary);
    if (state !== null) {
      await advanceState(account, since, state);
    }
    return;
  }
  const moved = await advanceState(account, since, found.state);
  const open = await tabs();
  if (
    !moved ||
    found.inboxId === null ||
    found.created.length === 0 ||
    open.some((tab) => tab.visibilityState === "visible")
  ) {
    return;
  }
  const emails = answerOf(
    await connection.post(
      [
        [
          "Email/get",
          {
            accountId: primary,
            ids: found.created.slice(-MAIL_PAGE),
            properties: [
              "from",
              "subject",
              "receivedAt",
              "keywords",
              "mailboxIds",
            ],
          },
          "g",
        ],
      ],
      MAIL
    ),
    "g"
  );
  await showMail(
    account,
    worthAnnouncing(emails?.args.list, found.inboxId, Date.now())
  );
};

// ------------------------------------------ New mail inside the push
//
// Stalwart 1.0 `emailPush` (mail/email-push.ts): instead of the StateChange
// of a delivery, an EmailPush with a few properties of each new email of
// the account. Shown with no token and no request (also in the static
// build), by the same rules: this device wants it, no tab visible, and
// what notify.ts keeps, with the inbox the tabs stored. Its content is as
// untrusted as any server text: checked, cleaned and shown as plain text.
// An account the tabs didn't set up here: nothing.

interface PushedMail {
  readonly accountId: string;
  readonly emails: readonly unknown[];
}

const readEmailPush = (value: unknown): PushedMail | null =>
  isObject(value) &&
  value["@type"] === "EmailPush" &&
  typeof value.accountId === "string" &&
  ID.test(value.accountId) &&
  Array.isArray(value.emails)
    ? { accountId: value.accountId, emails: value.emails.slice(0, MAIL_PAGE) }
    : null;

/** Whose mail comes inside the push (mail/push-store.ts), or null. */
const emailPushTarget = async (): Promise<{
  readonly account: string;
  readonly accountId: string;
  readonly inboxId: string;
} | null> => {
  const value = await kvValue("email push");
  return isObject(value) &&
    typeof value.account === "string" &&
    typeof value.accountId === "string" &&
    typeof value.inboxId === "string" &&
    ID.test(value.inboxId)
    ? {
        account: value.account,
        accountId: value.accountId,
        inboxId: value.inboxId,
      }
    : null;
};

const announcePushedMail = async (pushed: PushedMail): Promise<void> => {
  const target = await emailPushTarget();
  if (target === null || target.accountId !== pushed.accountId) {
    return;
  }
  const open = await tabs();
  if (open.some((tab) => tab.visibilityState === "visible")) {
    return;
  }
  await showMail(
    target.account,
    worthAnnouncing(pushed.emails, target.inboxId, Date.now())
  );
};

/** A verification code as servers write them (Stalwart: base64url). */
const VERIFICATION_CODE = /^[!-~]{1,255}$/u;

/**
 * Sends a PushVerification's code back (RFC 8620 §7.2.2): what a tab does
 * (lib/web-push.ts `verify`), for when none is open. True if the server
 * took it.
 */
const sendVerification = async (
  verification: Record<string, unknown>
): Promise<boolean> => {
  const { pushSubscriptionId: id, verificationCode: code } = verification;
  if (
    SW_TOKEN_ENDPOINT === null ||
    typeof id !== "string" ||
    !ID.test(id) ||
    typeof code !== "string" ||
    !VERIFICATION_CODE.test(code)
  ) {
    return false;
  }
  const responses = await jmap(
    [
      [
        "PushSubscription/set",
        { update: { [id]: { verificationCode: code } } },
        "0",
      ],
    ],
    []
  );
  const [first] = responses ?? [];
  const answer: unknown = Array.isArray(first) ? first[1] : null;
  return (
    Array.isArray(first) &&
    first[0] === "PushSubscription/set" &&
    isObject(answer) &&
    isObject(answer.updated) &&
    id in answer.updated
  );
};

const mailUrl = (data: unknown): string | null => {
  if (!isObject(data) || data["@type"] !== "MailNotice") {
    return null;
  }
  return typeof data.emailId === "string" && ID.test(data.emailId)
    ? `${SW_BASE}mail/inbox/${encodeURIComponent(data.emailId)}`
    : `${SW_BASE}mail/inbox`;
};

pushScope.addEventListener("push", (event) => {
  let object: unknown = null;
  try {
    object = event.data?.json() ?? null;
  } catch {
    object = null;
  }
  event.waitUntil(
    (async () => {
      const alert = readAlert(object);
      if (alert !== null) {
        await showAlert(alert);
        return;
      }
      if (isObject(object) && object["@type"] === "PushVerification") {
        const open = await tabs();
        // No tab to send it back: the worker does it itself when it can
        // reach JMAP, or the server would never push anything else
        // (RFC 8620 §7.2.2). Without, the next start re-subscribes
        // (lib/web-push.ts `renew`).
        if (open.length === 0) {
          await sendVerification(object).catch(() => false);
        }
        for (const tab of open) {
          // oxlint-disable-next-line unicorn/require-post-message-target-origin -- Client.postMessage (worker → tab) has no target origin.
          tab.postMessage({ object, type: PUSH_MESSAGE });
        }
        return;
      }
      const pushed = readEmailPush(object);
      if (pushed !== null) {
        if (await wantsMailNotices([pushed.accountId])) {
          await announcePushedMail(pushed).catch(() => null);
        }
        return;
      }
      const accounts = deliveries(object);
      if (await wantsMailNotices(accounts)) {
        await (
          SW_TOKEN_ENDPOINT === null
            ? announceMailGenerically()
            : announceMail(accounts)
        ).catch(() => null);
      }
    })()
  );
});

/** Snooze or dismiss from the notification (same patches as the tabs). */
const answerAlert = async (
  alert: Alert,
  action: "snooze" | "dismiss"
): Promise<void> => {
  const now = Date.now();
  const snoozed = alert.alertId.startsWith("snz");
  const patch: Record<string, unknown> = snoozed
    ? { [`alerts/${alert.alertId}`]: null }
    : { [`alerts/${alert.alertId}/acknowledged`]: utcDateTime(now) };
  if (action === "snooze") {
    const key = `snz${crypto.randomUUID().replaceAll("-", "").slice(0, 10)}`;
    patch[`alerts/${key}`] = {
      "@type": "Alert",
      action: "display",
      // A snooze of a snooze hangs from the same alert as before; the
      // worker doesn't look it up, so it hangs from this one.
      ...(snoozed
        ? {}
        : {
            relatedTo: {
              [alert.alertId]: {
                "@type": "Relation",
                relation: { parent: true },
              },
            },
          }),
      trigger: {
        "@type": "AbsoluteTrigger",
        when: utcDateTime(now + SNOOZE_MINUTES * 60_000),
      },
    };
  }
  await jmap(
    [
      [
        "CalendarEvent/set",
        {
          accountId: alert.accountId,
          update: { [alert.calendarEventId]: patch },
        },
        "0",
      ],
    ],
    CALENDARS
  );
};

/** The COMPLETED property for now (iCalendar form, as the tabs write). */
const completedNow = (): unknown[] => [
  "completed",
  {},
  "date-time",
  utcDateTime(Date.now()).replaceAll("-", "").replaceAll(":", ""),
];

/**
 * "Mark done" from a task's notification: the same patch the tabs write
 * (tasks/editing.ts `completeChange`), for a task or one occurrence of a
 * repeating task (all-day too since Stalwart 1.0). False when it can't be
 * read, or the task is assigned: the app does it, as only a tab can tell
 * the others (the REPLY to whoever assigned it, or the assignees' REQUEST).
 */
const completeTask = async (alert: Alert): Promise<boolean> => {
  const responses = await jmap(
    [
      [
        "CalendarEvent/get",
        {
          accountId: alert.accountId,
          ids: [alert.calendarEventId],
          properties: [
            "iCalendar",
            "participants",
            "percentComplete",
            "recurrenceOverrides",
            "recurrenceRule",
          ],
        },
        "0",
      ],
    ],
    CALENDARS
  );
  const answer = responses?.[0];
  const list =
    Array.isArray(answer) && isObject(answer[1]) ? answer[1].list : null;
  const task: unknown = Array.isArray(list) ? list[0] : null;
  if (
    !isObject(task) ||
    (isObject(task.participants) && Object.keys(task.participants).length > 0)
  ) {
    return false;
  }
  const patch: Record<string, unknown> = {
    [`alerts/${alert.alertId}/acknowledged`]: utcDateTime(Date.now()),
  };
  const repeating = isObject(task.recurrenceRule);
  if (repeating && alert.recurrenceId !== null) {
    const overrides = isObject(task.recurrenceOverrides)
      ? task.recurrenceOverrides
      : {};
    const current = overrides[alert.recurrenceId];
    patch.recurrenceOverrides = {
      ...overrides,
      // With its start: see `withStart` in tasks/editing.ts.
      [alert.recurrenceId]: {
        start: alert.recurrenceId,
        ...(isObject(current) ? current : {}),
        progress: "completed",
        // RFC 8984 §5.2.6, in the occurrence's override (tasks/editing.ts).
        progressUpdated: utcDateTime(Date.now()),
      },
    };
  } else {
    patch.progress = "completed";
    patch.progressUpdated = utcDateTime(Date.now());
    if (typeof task.percentComplete === "number") {
      patch.percentComplete = 100;
    }
    const component = isObject(task.iCalendar) ? task.iCalendar : null;
    const properties: unknown = component?.properties;
    const current: readonly unknown[] = Array.isArray(properties)
      ? properties
      : [];
    const others = current.filter(
      (entry: unknown) =>
        !Array.isArray(entry) ||
        typeof entry[0] !== "string" ||
        entry[0].toLowerCase() !== "completed"
    );
    if (component === null) {
      patch.iCalendar = { name: "vtodo", properties: [completedNow()] };
    } else {
      patch["iCalendar/properties"] = [...others, completedNow()];
    }
  }
  await jmap(
    [
      [
        "CalendarEvent/set",
        {
          accountId: alert.accountId,
          update: { [alert.calendarEventId]: patch },
        },
        "0",
      ],
    ],
    CALENDARS
  );
  return true;
};

/**
 * Focuses a tab of the app on `url`, or opens one. Only tabs this worker
 * controls: `navigate` rejects on the others. Focus first, while the
 * click's activation lasts, and on its own: if it fails, the tab still
 * navigates and no second window opens. A new window only when there is no
 * such tab or `navigate` fails.
 */
const openApp = async (url: string): Promise<void> => {
  const controlled = await pushScope.clients.matchAll({ type: "window" });
  const tab = controlled.find(inScope);
  if (tab?.navigate !== undefined) {
    try {
      await tab.focus?.();
    } catch {
      // No activation left (or not allowed): navigating is what matters.
    }
    try {
      await tab.navigate(url);
      return;
    } catch {
      // Not navigable after all: open a window instead.
    }
  }
  await pushScope.clients.openWindow(url);
};

pushScope.addEventListener("notificationclick", (event) => {
  const mail = mailUrl(event.notification.data);
  if (mail !== null) {
    event.notification.close();
    event.waitUntil(openApp(mail));
    return;
  }
  const alert = readAlert(
    isObject(event.notification.data)
      ? { "@type": "CalendarAlert", ...event.notification.data }
      : null
  );
  event.notification.close();
  if (alert === null) {
    return;
  }
  const isTask =
    isObject(event.notification.data) && event.notification.data.task === true;
  event.waitUntil(
    (async () => {
      if (event.action === "snooze" || event.action === "dismiss") {
        await answerAlert(alert, event.action);
        return;
      }
      if (event.action === "done" && (await completeTask(alert))) {
        return;
      }
      await openApp(isTask ? taskUrl(alert) : eventUrl(alert));
    })()
  );
});
