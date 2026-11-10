// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Web Push through the real bridge (extension of
// 2026-10-28), with every tab of the app closed: the app subscribes with
// the bridge's VAPID key and `PushSubscription/set`; the bridge sends a
// `PushVerification` to the push service (a fake one on loopback,
// push-service.ts, that checks the VAPID token and decrypts with the
// browser's key), the worker passes it to the tab and the tab sends the
// code back. Then:
//
// - new mail arrives, the bridge pushes a `StateChange` with
//   `EmailDelivery`, and the worker asks the bridge what is new and shows
//   the sender and the subject;
// - a reminder is due, the bridge (with no event source open) pushes the
//   `CalendarAlert`, and the worker shows the event's title;
// - mail another client puts in the inbox (IMAP APPEND, as a delivery):
//   the bridge puts it inside the push (`emailPush`, as Stalwart 1.0) and
//   the worker shows it without a single request (2026-11-01).
//
// The browser's PushManager is replaced (init script) so its endpoint is
// the fake service and its keys are known; what the service decrypts is
// handed to the worker with Chromium's DevTools, as a push service would.

import { CAPABILITIES, EmailId, JmapClient } from "@nebulamail/jmap-client";
import type {
  BrowserContext,
  CDPSession,
  Page,
  Worker,
} from "@playwright/test";
import { expect, test } from "@playwright/test";

import { DOVECOT, WEB_ENV } from "./env";
import { imapAppend } from "./imap-append";
import type { PushService } from "./push-service";
import { BROWSER_KEYS, startPushService } from "./push-service";
import { signIn } from "./session";

// The headless shell has no notifications: Chromium's full browser.
test.use({ channel: "chromium" });

const bytes = (text: string): number[] => [
  ...Buffer.from(text, "base64url").values(),
];

/** Runs in the page before the app: a PushManager with known keys. */
const fakePushManager = (fake: {
  endpoint: string;
  p256dh: number[];
  auth: number[];
}): void => {
  const STORE = "e2e-fake-push";
  const make = (key: number[]) => ({
    endpoint: fake.endpoint,
    expirationTime: null,
    getKey: (name: string) =>
      new Uint8Array(name === "p256dh" ? fake.p256dh : fake.auth).buffer,
    options: {
      applicationServerKey: new Uint8Array(key).buffer,
      userVisibleOnly: true,
    },
    unsubscribe: async () => {
      localStorage.removeItem(STORE);
      return await Promise.resolve(true);
    },
  });
  Object.defineProperty(PushManager.prototype, "getSubscription", {
    value: async () => {
      const stored = localStorage.getItem(STORE);
      const parsed: unknown = stored === null ? null : JSON.parse(stored);
      return await Promise.resolve(
        Array.isArray(parsed)
          ? make(parsed.filter((n): n is number => typeof n === "number"))
          : null
      );
    },
  });
  Object.defineProperty(PushManager.prototype, "subscribe", {
    value: async (options: { applicationServerKey: Uint8Array }) => {
      const key = [...new Uint8Array(options.applicationServerKey)];
      localStorage.setItem(STORE, JSON.stringify(key));
      return await Promise.resolve(make(key));
    },
  });
};

const registrationOf = async (cdp: CDPSession): Promise<string> => {
  const { promise, resolve } = Promise.withResolvers<string>();
  cdp.on("ServiceWorker.workerRegistrationUpdated", (event) => {
    const found = event.registrations.find((item) => !item.isDeleted);
    if (found !== undefined) {
      resolve(found.registrationId);
    }
  });
  await cdp.send("ServiceWorker.enable");
  return await promise;
};

const shown = async (worker: Worker) =>
  await worker.evaluate(async () => {
    const registration: unknown = Reflect.get(globalThis, "registration");
    if (!(registration instanceof ServiceWorkerRegistration)) {
      throw new TypeError("Not the service worker");
    }
    const list = await registration.getNotifications();
    return list.map((item) => ({ body: item.body, title: item.title }));
  });

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null;

interface Setup {
  readonly service: PushService;
  readonly worker: Worker;
  readonly client: JmapClient;
  readonly deliver: (object: unknown) => Promise<void>;
}

/**
 * Signed in as alice with the fake PushManager, the worker ready and a
 * JMAP client with one token (the page leaves the app later); `setting`
 * (in Settings › General) turned on, which subscribes; the verification
 * done as the app does it.
 */
const subscribed = async (
  context: BrowserContext,
  page: Page,
  setting: string
): Promise<Setup> => {
  const service = await startPushService();
  await context.addInitScript(fakePushManager, {
    auth: bytes(BROWSER_KEYS.auth),
    endpoint: service.endpoint,
    p256dh: bytes(BROWSER_KEYS.p256dh),
  });
  await context.grantPermissions(["notifications"], {
    origin: WEB_ENV.APP_URL,
  });
  await signIn(page, "alice", DOVECOT.password);
  await expect(page).toHaveURL(/\/mail\/inbox/u);
  await page.evaluate(async () => {
    const registration = await navigator.serviceWorker.ready;
    return registration.scope;
  });
  const worker =
    context.serviceWorkers()[0] ??
    (await context.waitForEvent("serviceworker"));
  const cdp = await context.newCDPSession(page);
  const registrationId = await registrationOf(cdp);
  const deliver = async (object: unknown): Promise<void> => {
    await cdp.send("ServiceWorker.deliverPushMessage", {
      data: JSON.stringify(object),
      origin: new URL(WEB_ENV.APP_URL).origin,
      registrationId,
    });
  };
  const grant: unknown = await page.evaluate(async () => {
    const response = await fetch("/auth/token", {
      credentials: "same-origin",
      method: "POST",
    });
    return (await response.json()) as unknown;
  });
  if (
    !isObject(grant) ||
    typeof grant.accessToken !== "string" ||
    typeof grant.sessionUrl !== "string"
  ) {
    throw new Error("no token");
  }
  const { accessToken, sessionUrl } = grant;
  const client = await JmapClient.connect({
    getAccessToken: async () => await Promise.resolve(accessToken),
    sessionUrl,
  });

  // The session announces the bridge's VAPID key.
  expect(
    client.session.capabilities["urn:ietf:params:jmap:webpush-vapid"]
  ).toMatchObject({ applicationServerKey: expect.any(String) });

  // A failed earlier try may have left a subscription behind.
  const [left] = await client.request((b) => [
    b.call("PushSubscription/get", {}),
  ]);
  if (left.list.length > 0) {
    await client.request((b) => [
      b.call("PushSubscription/set", {
        destroy: left.list.map((item) => item.id),
      }),
    ]);
  }

  // Turning the setting on subscribes; the bridge sends the verification.
  // Click only while it is off: a second click on a slow machine would
  // turn it off again and subscribe twice.
  await page.goto("/settings/general");
  const toggle = page.getByRole("switch", { name: setting });
  await expect(async () => {
    if (!(await toggle.isChecked())) {
      await toggle.click();
    }
    await expect(toggle).toBeChecked({ timeout: 5000 });
  }).toPass();
  await expect.poll(() => service.received.length).toBe(1);
  const [verification] = service.received;
  expect(verification).toMatchObject({ "@type": "PushVerification" });
  if (!isObject(verification)) {
    throw new Error("no verification");
  }
  // The worker passes it to the tab, which sends the code back.
  await deliver(verification);
  await expect
    .poll(async () => {
      const [result] = await client.request((b) => [
        b.call("PushSubscription/get", {}),
      ]);
      return result.list.map((item) => item.verificationCode);
    })
    .toStrictEqual([verification.verificationCode]);
  return { client, deliver, service, worker };
};

const unsubscribe = async ({ client, service }: Setup): Promise<void> => {
  const [subscriptions] = await client.request((b) => [
    b.call("PushSubscription/get", {}),
  ]);
  await client.request((b) => [
    b.call("PushSubscription/set", {
      destroy: subscriptions.list.map((item) => item.id),
    }),
  ]);
  await service.close();
};

test("new mail with every tab closed: pushed by the bridge, shown by the worker", async ({
  context,
  page,
}) => {
  const setup = await subscribed(context, page, "New mail notifications");
  const { client, deliver, service, worker } = setup;
  const accountId = client.primaryAccountId();

  // Every tab of the app closed (this one shows another site).
  await page.goto("about:blank");

  // New mail arrives.
  const subject = `Bridge push e2e ${Date.now()}`;
  const raw = new TextEncoder().encode(
    `From: Ana Ruiz <ana@example.com>\r\nTo: alice@nebulamail.test\r\nSubject: ${subject}\r\n` +
      `Message-ID: <push-${Date.now()}@example.com>\r\n\r\nSecret body text.\r\n`
  );
  const [mailboxes] = await client.request((b) => [
    b.call("Mailbox/get", { accountId, properties: ["role"] }),
  ]);
  const inboxId = mailboxes.list.find((item) => item.role === "inbox")?.id;
  if (inboxId === undefined) {
    throw new Error("no inbox");
  }
  const blob = await client.upload(accountId, raw, {
    type: "message/rfc822",
  });
  const [imported] = await client.request((b) => [
    b.call("Email/import", {
      accountId,
      emails: { m: { blobId: blob.blobId, mailboxIds: { [inboxId]: true } } },
    }),
  ]);
  const emailId = imported.created?.m?.id;

  // The bridge pushes EmailDelivery (only what the web asked for).
  await expect.poll(() => service.received.length).toBe(2);
  const [, change] = service.received;
  expect(change).toMatchObject({ "@type": "StateChange" });
  const changed = isObject(change) ? change.changed : null;
  expect(
    Object.keys(isObject(changed) ? (changed[accountId] ?? {}) : {})
  ).toStrictEqual(["EmailDelivery"]);
  await deliver(change);
  await expect
    .poll(async () => await shown(worker))
    .toStrictEqual([{ body: subject, title: "Ana Ruiz" }]);
  expect(JSON.stringify(service.received)).not.toContain("Secret body");

  // Clean up: the email and the subscription.
  await client.request((b) => [
    b.call("Email/set", {
      accountId,
      destroy: emailId === undefined ? [] : [emailId],
    }),
  ]);
  await unsubscribe(setup);
});

test("mail inside the push: put in by the bridge, shown by the worker with no request", async ({
  context,
  page,
}) => {
  test.setTimeout(120_000);
  const setup = await subscribed(context, page, "New mail notifications");
  const { client, deliver, service, worker } = setup;
  const accountId = client.primaryAccountId();

  // The bridge can, and the web asked for it (mail/email-push.ts).
  expect(
    client.session.capabilities["urn:ietf:params:jmap:emailpush"]
  ).toStrictEqual({});
  const [subscriptions] = await client.request((b) => [
    b.call("PushSubscription/get", {}),
  ]);
  const asked: unknown = subscriptions.list[0]?.emailPush;
  expect(isObject(asked) ? asked[accountId] : null).toMatchObject({
    properties: [
      "id",
      "from",
      "subject",
      "receivedAt",
      "keywords",
      "mailboxIds",
    ],
    urgency: "normal",
  });

  // Every tab closed; from now on, what the worker asks for is counted
  // (PW_EXPERIMENTAL_SERVICE_WORKER_NETWORK_EVENTS, playwright config).
  await page.goto("about:blank");
  const requests: string[] = [];
  context.on("request", (request) => {
    if (request.serviceWorker() !== null) {
      requests.push(request.url());
    }
  });

  // Another client adds mail to the inbox: new mail to the bridge.
  const subject = `Bridge EmailPush e2e ${Date.now()}`;
  await imapAppend(
    "alice",
    `From: Ana Ruiz <ana.ruiz@example.com>\r\nTo: alice@example.com\r\nSubject: ${subject}\r\n` +
      `Date: ${new Date().toUTCString()}\r\n` +
      `Message-ID: <emailpush-${Date.now()}@example.com>\r\n\r\nSecret body text.\r\n`
  );

  // Inside the push: sender and subject, never the body.
  await expect.poll(() => service.received.length, { timeout: 60_000 }).toBe(2);
  const [, pushed] = service.received;
  expect(pushed).toMatchObject({
    "@type": "EmailPush",
    accountId,
    emails: [
      { from: [{ email: "ana.ruiz@example.com", name: "Ana Ruiz" }], subject },
    ],
  });
  expect(JSON.stringify(pushed)).not.toContain("Secret body");
  await deliver(pushed);
  await expect
    .poll(async () => await shown(worker))
    .toStrictEqual([{ body: subject, title: "Ana Ruiz" }]);
  // Shown with no token and no JMAP request. Chromium itself loads the
  // notification's icon (a static file of the app, attributed to the
  // worker): that one is no request of ours.
  expect(
    requests.filter((url) => new URL(url).pathname !== "/icons/icon-192.png")
  ).toStrictEqual([]);

  // Clean up: the email and the subscription.
  const emails =
    isObject(pushed) && Array.isArray(pushed.emails) ? pushed.emails : [];
  const ids = emails.flatMap((email: unknown) =>
    isObject(email) && typeof email.id === "string"
      ? [EmailId.parse(email.id)]
      : []
  );
  await client.request((b) => [
    b.call("Email/set", { accountId, destroy: ids }),
  ]);
  await unsubscribe(setup);
});

/** A UTC wall time ("2026-10-27T21:05:00") for JSCalendar. */
const utc = (ms: number): string => new Date(ms).toISOString().slice(0, 19);

test("a reminder with every tab closed: the bridge pushes the alert", async ({
  context,
  page,
}) => {
  test.setTimeout(180_000);
  const setup = await subscribed(context, page, "Event reminders");
  const { client, deliver, service, worker } = setup;
  const accountId = client.primaryAccountId(CAPABILITIES.calendars);
  const [calendars] = await client.request((b) => [
    b.call("Calendar/get", { accountId }),
  ]);
  const calendar = calendars.list[0]?.id;
  if (calendar === undefined) {
    throw new Error("No calendar (the bridge makes one)");
  }
  // Every tab of the app closed: no event source is open.
  await page.goto("about:blank");
  // The next whole minute at least 75 s away; the alert a minute before.
  const at = Math.ceil((Date.now() + 75_000) / 60_000) * 60_000;
  const title = `Push reminder ${Date.now().toString(36)}`;
  const [created] = await client.request((b) => [
    b.call("CalendarEvent/set", {
      accountId,
      create: {
        e: {
          "@type": "Event",
          alerts: {
            a1: {
              "@type": "Alert",
              action: "display",
              trigger: { "@type": "OffsetTrigger", offset: "-PT1M" },
            },
          },
          calendarIds: { [calendar]: true },
          duration: "PT30M",
          start: utc(at),
          timeZone: "Etc/UTC",
          title,
        },
      },
      sendSchedulingMessages: false,
    }),
  ]);
  const eventId = created.created.e?.id;
  expect(eventId).toBeDefined();
  // The bridge decides the alert is due and pushes it (with no event
  // source open, only because of the Web Push subscription).
  await expect
    .poll(() => service.received.length, { timeout: 120_000 })
    .toBe(2);
  const [, alert] = service.received;
  expect(alert).toMatchObject({
    "@type": "CalendarAlert",
    alertId: "a1",
    calendarEventId: eventId,
  });
  await deliver(alert);
  await expect
    .poll(async () => {
      const list = await shown(worker);
      return list.map((item) => item.title);
    })
    .toStrictEqual([title]);

  await client.request((b) => [
    b.call("CalendarEvent/set", {
      accountId,
      destroy: eventId === undefined ? [] : [eventId],
    }),
  ]);
  await unsubscribe(setup);
});
