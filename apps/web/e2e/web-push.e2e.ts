// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// New mail with every tab closed: the service worker gets a
// Web Push StateChange ("EmailDelivery"), asks the fake JMAP server what
// is new from the state stored on the device and shows the sender and the
// subject, once. The push is delivered with Chromium's DevTools protocol
// (`ServiceWorker.deliverPushMessage`), as a push service would; what the
// worker showed and stored is read inside the worker, with no tab open.
//
// Also mail inside the push (Stalwart 1.0 `emailPush`): an EmailPush shown
// with the server out of reach.
//
// Needs PW_EXPERIMENTAL_SERVICE_WORKER_NETWORK_EVENTS=1 (playwright.config.ts)
// so the worker's requests reach the fake JMAP server.

import type { CDPSession, Page, Worker } from "@playwright/test";
import { expect, test } from "@playwright/test";

import { TEST_ENV } from "./env";
import type { FakeEmail } from "./fake-jmap";
import { deliver, installFakeJmap } from "./fake-jmap";
import { signIn } from "./session";

// The headless shell has no notifications (Notification.permission stays
// "denied"): Chromium's full browser, headless.
test.use({ channel: "chromium" });

test.beforeEach(({ browserName }, info) => {
  test.skip(
    browserName !== "chromium" || info.project.name !== "desktop",
    "the push service is Chromium's DevTools; one run is enough"
  );
});

const arrived = (id: string, subject: string): FakeEmail => ({
  // A right-to-left override in the name: shown without it.
  from: { email: "ana@example.com", name: "Ana‮ Ruiz" },
  html: "<p>Secret body text</p>",
  id,
  keywords: {},
  mailboxIds: { inbox: true },
  receivedAt: new Date().toISOString(),
  subject,
});

const setting = (page: Page) =>
  page.getByRole("switch", { name: "New mail notifications" });

const toggleSetting = async (page: Page): Promise<void> => {
  await page.goto("/mail/inbox");
  await expect(page.getByText("Weekly digest").first()).toBeVisible();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await setting(page).click();
};

/** The worker's registration, for DevTools. */
const registrationOf = async (cdp: CDPSession): Promise<string> => {
  const { promise, resolve } = Promise.withResolvers<string>();
  cdp.on("ServiceWorker.workerRegistrationUpdated", (event) => {
    const found = event.registrations.find(
      (registration) => !registration.isDeleted
    );
    if (found !== undefined) {
      resolve(found.registrationId);
    }
  });
  await cdp.send("ServiceWorker.enable");
  return await promise;
};

/** Notifications the worker shows now, by tag. */
const shown = async (worker: Worker) =>
  await worker.evaluate(async () => {
    const registration: unknown = Reflect.get(globalThis, "registration");
    if (!(registration instanceof ServiceWorkerRegistration)) {
      throw new TypeError("Not the service worker");
    }
    const list = await registration.getNotifications();
    return list
      .map((item) => ({ body: item.body, tag: item.tag, title: item.title }))
      .toSorted((a, b) => a.tag.localeCompare(b.tag));
  });

const shownTags = async (worker: Worker): Promise<string[]> => {
  const list = await shown(worker);
  return list.map((item) => item.tag);
};

/** The Email state the worker stored (mail/push-store.ts). */
const storedState = async (worker: Worker) =>
  await worker.evaluate(async () => {
    // oxlint-disable-next-line unicorn/consistent-function-scoping -- this body runs inside the worker: it can't use anything from the test's scope.
    const settle = async <T>(request: IDBRequest<T>): Promise<T> => {
      const { promise, reject, resolve } = Promise.withResolvers<T>();
      request.addEventListener("success", () => {
        resolve(request.result);
      });
      request.addEventListener("error", () => {
        reject(request.error ?? new Error("IndexedDB failed"));
      });
      return await promise;
    };
    const db = await settle(indexedDB.open("nebula-mail-push", 1));
    const all: unknown = await settle(
      db.transaction("kv").objectStore("kv").getAll()
    );
    db.close();
    for (const item of Array.isArray(all) ? all : []) {
      if (typeof item === "object" && item !== null) {
        const id: unknown = Reflect.get(item, "id");
        if (typeof id === "string" && id.startsWith("state ")) {
          const value: unknown = Reflect.get(item, "value");
          return value;
        }
      }
    }
    return null;
  });

/** Whose mail comes inside the push, as the tab stored it (push-store.ts). */
const storedTarget = async (worker: Worker) =>
  await worker.evaluate(async () => {
    // oxlint-disable-next-line unicorn/consistent-function-scoping -- this body runs inside the worker: it can't use anything from the test's scope.
    const settle = async <T>(request: IDBRequest<T>): Promise<T> => {
      const { promise, reject, resolve } = Promise.withResolvers<T>();
      request.addEventListener("success", () => {
        resolve(request.result);
      });
      request.addEventListener("error", () => {
        reject(request.error ?? new Error("IndexedDB failed"));
      });
      return await promise;
    };
    const db = await settle(indexedDB.open("nebula-mail-push", 1));
    const found: unknown = await settle(
      db.transaction("kv").objectStore("kv").get("email push")
    );
    db.close();
    if (typeof found !== "object" || found === null) {
      return null;
    }
    const value: unknown = Reflect.get(found, "value");
    return value;
  });

test("new mail with every tab closed: the worker announces it once", async ({
  context,
  page,
}) => {
  await signIn(context);
  await context.grantPermissions(["notifications"], {
    origin: TEST_ENV.APP_URL,
  });
  const jmap = await installFakeJmap(page, { serviceWorker: true });
  await page.goto("/mail/inbox");
  await page.evaluate(async () => {
    const registration = await navigator.serviceWorker.ready;
    return registration.scope;
  });
  const [worker] = context.serviceWorkers();
  if (worker === undefined) {
    throw new Error("No service worker");
  }

  // Turning the setting on tells the worker, and where to start from.
  await toggleSetting(page);
  await expect(setting(page)).toBeChecked();
  await expect.poll(async () => await storedState(worker)).toBe("1");

  const cdp = await context.newCDPSession(page);
  const registrationId = await registrationOf(cdp);
  const push = async (state: string): Promise<void> => {
    await cdp.send("ServiceWorker.deliverPushMessage", {
      data: JSON.stringify({
        "@type": "StateChange",
        changed: { a: { EmailDelivery: state } },
      }),
      origin: new URL(TEST_ENV.APP_URL).origin,
      registrationId,
    });
  };
  /** New mail arrives and is pushed; the worker has dealt with it. */
  const arrive = async (email: FakeEmail): Promise<void> => {
    deliver(jmap, email);
    const state = String(1 + jmap.arrivals.length);
    await push(state);
    await expect.poll(async () => await storedState(worker)).toBe(state);
  };

  // Every tab of the app closed (this one shows another site).
  await page.goto("about:blank");
  await arrive(arrived("n1", "Lunch on Friday?"));
  await expect
    .poll(async () => await shown(worker))
    .toStrictEqual([
      { body: "Lunch on Friday?", tag: "wm-n1", title: "Ana Ruiz" },
    ]);
  expect(JSON.stringify(await shown(worker))).not.toContain("Secret body");

  // The same push again, and mail already read: nothing new.
  await push("2");
  await arrive({
    ...arrived("n2", "Read elsewhere"),
    keywords: { $seen: true },
  });
  await arrive(arrived("n3", "Second one"));
  await expect
    .poll(async () => await shownTags(worker))
    .toStrictEqual(["wm-n1", "wm-n3"]);

  // A visible tab shows new mail itself: the worker stays quiet.
  await page.goto("/mail/inbox");
  await expect(page.getByText("Weekly digest").first()).toBeVisible();
  await arrive(arrived("n4", "While you look"));

  // Turned off: nothing more, even with every tab closed.
  await toggleSetting(page);
  await expect(setting(page)).not.toBeChecked();
  await page.goto("about:blank");
  deliver(jmap, arrived("n5", "After turning it off"));
  await push("6");
  // Give the worker time to (not) show it.
  await page.waitForTimeout(1500);
  await expect(shownTags(worker)).resolves.toStrictEqual(["wm-n1", "wm-n3"]);
});

test("mail inside the push (Stalwart 1.0): shown with no request", async ({
  context,
  page,
}) => {
  await signIn(context);
  await context.grantPermissions(["notifications"], {
    origin: TEST_ENV.APP_URL,
  });
  const jmap = await installFakeJmap(page, { serviceWorker: true });
  jmap.emailPush = true;
  await page.goto("/mail/inbox");
  await page.evaluate(async () => {
    const registration = await navigator.serviceWorker.ready;
    return registration.scope;
  });
  const [worker] = context.serviceWorkers();
  if (worker === undefined) {
    throw new Error("No service worker");
  }
  await toggleSetting(page);
  await expect(setting(page)).toBeChecked();
  // The tab tells the worker whose mail comes in the push, and the inbox.
  await expect
    .poll(async () => await storedTarget(worker))
    .toMatchObject({ accountId: "a", inboxId: "inbox" });

  const cdp = await context.newCDPSession(page);
  const registrationId = await registrationOf(cdp);
  const now = new Date().toISOString();
  const pushMail = async (
    accountId: string,
    emails: readonly unknown[]
  ): Promise<void> => {
    await cdp.send("ServiceWorker.deliverPushMessage", {
      data: JSON.stringify({ "@type": "EmailPush", accountId, emails }),
      origin: new URL(TEST_ENV.APP_URL).origin,
      registrationId,
    });
  };
  const email = {
    from: [{ email: "[EMAIL]", name: "Ana\u202E Ruiz" }],
    id: "p1",
    keywords: {},
    mailboxIds: { inbox: true },
    receivedAt: now,
    // Text, never markup.
    subject: "<img src=x onerror=alert(1)> Lunch",
  };

  // Every tab closed and the server out of reach: all of it is in the push.
  await page.goto("about:blank");
  jmap.down = true;
  await pushMail("a", [
    email,
    // Not what notify.ts keeps: read, junk, another folder, old, bad id.
    { ...email, id: "p2", keywords: { $seen: true } },
    { ...email, id: "p3", keywords: { $junk: true } },
    { ...email, id: "p4", mailboxIds: { archive: true } },
    { ...email, id: "p5", receivedAt: "2020-01-01T00:00:00Z" },
    { ...email, id: "../p6" },
  ]);
  await expect
    .poll(async () => await shown(worker))
    .toStrictEqual([
      {
        body: "<img src=x onerror=alert(1)> Lunch",
        tag: "wm-p1",
        title: "Ana Ruiz",
      },
    ]);

  // Once; and nothing for an account this device didn't set up.
  await pushMail("a", [email]);
  await pushMail("b", [{ ...email, id: "q1" }]);
  // A sender left out (the push was full): still a title.
  await pushMail("a", [{ ...email, from: undefined, id: "p7" }]);
  // A subject left out (Stalwart drops one too big for the push, seen
  // 2026-11-03 with 5000 characters): not «(no subject)».
  const { subject: _dropped, ...withoutSubject } = email;
  await pushMail("a", [{ ...withoutSubject, id: "p8" }]);
  await pushMail("a", [{ ...withoutSubject, from: undefined, id: "p9" }]);
  await expect
    .poll(async () => await shown(worker))
    .toStrictEqual([
      {
        body: "<img src=x onerror=alert(1)> Lunch",
        tag: "wm-p1",
        title: "Ana Ruiz",
      },
      {
        body: "<img src=x onerror=alert(1)> Lunch",
        tag: "wm-p7",
        title: "New mail",
      },
      { body: "New mail", tag: "wm-p8", title: "Ana Ruiz" },
      { body: "", tag: "wm-p9", title: "New mail" },
    ]);
});
