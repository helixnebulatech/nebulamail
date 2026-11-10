// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The full BFF mode against a real Stalwart
// (playwright.proxy.config.ts): the app signs in through
// Stalwart's own page as alice of `bun run dev:setup`, and from then on
// every JMAP request goes to /jmap-proxy/ on the app's origin. Checked on
// every request of the page and its service worker: no Authorization
// header, no access or refresh token in any answer, nothing sent to
// Stalwart but its login page; /auth/token is 404 and the CSP lets pages
// connect to their own origin only. Skipped unless these are set
// (passwords in docker/.env):
//
//   MAIL_LIVE_URL=http://localhost:8080 MAIL_LIVE_ALICE_PASSWORD=… \
//   bun run test:proxy
//
// It sends one email from alice to herself; everything it creates is
// deleted at the end.

import { readFile } from "node:fs/promises";
import { setTimeout as delay } from "node:timers/promises";

import type { FetchLike, JmapClient, MailboxId } from "@nebulamail/jmap-client";
import { JmapClient as Client } from "@nebulamail/jmap-client";
import type { BrowserContext, Page, Request, Response } from "@playwright/test";
import { expect, test } from "@playwright/test";

import {
  allowIfAsked,
  destroyMatching,
  openComposer,
  reader,
  row,
} from "../e2e-live/ui";
import { parseSealKeys, unseal } from "../src/server/auth/seal";

const env = {
  alicePassword: process.env.MAIL_LIVE_ALICE_PASSWORD ?? "",
  url: process.env.MAIL_LIVE_URL ?? "",
};
const configured = Object.values(env).every((value) => value !== "");

const ALICE = ["alice", "nebulamail.test"].join("@");
/** Stalwart 1.0 takes basic auth with the login name, not the address. */
const ALICE_LOGIN = "alice";
const APP = "http://localhost:3001";
/** dev:setup's app port is part of the cookie names on localhost. */
const SESSION_COOKIE = "__Host-wm-session-3001";
const PROXY_COOKIE = "__Host-wm-proxy-3001";

/**
 * What the browser may ask Stalwart itself: its login page (and what that
 * page uses), nothing else. Everything else must go through the proxy.
 */
const LOGIN_PATHS = new Set([
  "/login",
  "/api/auth",
  "/api/auth/client",
  "/logo",
]);
// Since Stalwart 1.0.3 the page loads its style and script from
// /ui-<hash>.css|js; the hash changes with every version (docs/jmap-proxy.md).
const LOGIN_ASSET = /^\/ui-[0-9a-f]+\.(?:css|js)$/u;
const isLoginPath = (path: string): boolean =>
  LOGIN_PATHS.has(path) || path.startsWith("/logo/") || LOGIN_ASSET.test(path);

test.skip(!configured, "set MAIL_LIVE_* (see the comment at the top)");
test.describe.configure({ timeout: 240_000 });

const RUN = `nebulaproxy${Date.now().toString(36)}`;

/** Stalwart takes basic auth too: for what the test sets up and checks. */
const basicFetch =
  (user: string, password: string): FetchLike =>
  async (input, init) => {
    const headers = new Headers(init.headers);
    headers.set("Authorization", `Basic ${btoa(`${user}:${password}`)}`);
    return await fetch(input, { ...init, headers });
  };

const direct = async (): Promise<JmapClient> =>
  await Client.connect({
    fetch: basicFetch(ALICE_LOGIN, env.alicePassword),
    getAccessToken: async () => await Promise.resolve("unused"),
    sessionUrl: `${env.url}/.well-known/jmap`,
  });

const mailbox = async (
  client: JmapClient,
  role: string
): Promise<MailboxId> => {
  const [boxes] = await client.request((b) => [
    b.call("Mailbox/get", {
      accountId: client.primaryAccountId(),
      properties: ["role"],
    }),
  ]);
  const found = boxes.list.find((box) => box.role === role);
  if (found === undefined) {
    throw new Error(`No ${role} folder`);
  }
  return found.id;
};

/** An email delivered to alice's inbox behind the app's back (push test). */
const importToInbox = async (
  client: JmapClient,
  subject: string
): Promise<void> => {
  const accountId = client.primaryAccountId();
  const raw = new TextEncoder().encode(
    [
      `From: Someone <${["someone", "example.net"].join("@")}>`,
      `To: ${ALICE}`,
      `Subject: ${subject}`,
      `Message-ID: <${RUN}.${Date.now()}@example.net>`,
      `Date: ${new Date().toUTCString()}`,
      "Content-Type: text/plain; charset=utf-8",
      "",
      "Arrived while the inbox was open.",
      "",
    ].join("\r\n")
  );
  const { blobId } = await client.upload(accountId, raw, {
    type: "message/rfc822",
  });
  const inbox = await mailbox(client, "inbox");
  await client.request((b) => [
    b.call("Email/import", {
      accountId,
      emails: { push: { blobId, mailboxIds: { [inbox]: true } } },
    }),
  ]);
};

interface Traffic {
  readonly requests: Request[];
  readonly bodies: { readonly url: string; readonly text: string }[];
}

/** Records every request of the context (pages and service worker). */
const record = (context: BrowserContext): Traffic => {
  const traffic: Traffic = { bodies: [], requests: [] };
  context.on("request", (request) => {
    traffic.requests.push(request);
  });
  context.on("response", (response: Response) => {
    const type = response.headers()["content-type"] ?? "";
    // Text answers only: never the push stream (it doesn't end) or files.
    if (!/json|html|javascript|text\/plain/u.test(type)) {
      return;
    }
    const read = async (): Promise<void> => {
      const text = await response.text();
      traffic.bodies.push({ text, url: response.url() });
    };
    read().catch(() => null);
  });
  return traffic;
};

/**
 * Every header of a request. allHeaders() waits for the answer of requests
 * still open (the push stream): headers() then, which has what the page set.
 */
const headersOf = async (request: Request): Promise<Record<string, string>> =>
  await Promise.race([
    request.allHeaders().catch(() => request.headers()),
    delay(1000).then(() => request.headers()),
  ]);

/** The tokens in the sealed session cookie (the test knows the keys). */
const sessionTokens = async (
  context: BrowserContext
): Promise<readonly string[]> => {
  const local = await readFile(
    new URL("../.env.local", import.meta.url),
    "utf-8"
  );
  const keys = /^SESSION_KEYS=(?<keys>.+)$/mu.exec(local)?.groups?.keys ?? "";
  const jar = await context.cookies(APP);
  const cookie = jar.find((item) => item.name === SESSION_COOKIE);
  const session: unknown = await unseal(
    await parseSealKeys(keys.trim()),
    "session",
    cookie?.value ?? ""
  );
  if (
    typeof session !== "object" ||
    session === null ||
    !("accessToken" in session) ||
    !("refreshToken" in session)
  ) {
    throw new Error("No session cookie");
  }
  return [String(session.accessToken), String(session.refreshToken)];
};

const signIn = async (page: Page): Promise<void> => {
  await page.goto("/login");
  await page.getByRole("button", { exact: true, name: "Sign in" }).click();
  await page.getByRole("textbox", { name: "Username" }).fill(ALICE_LOGIN);
  await page.getByLabel("Password").fill(env.alicePassword);
  await page.getByRole("button", { exact: true, name: "Sign in" }).click();
  await allowIfAsked(page, /\/mail\/inbox/u);
};

test.afterAll(async () => {
  if (configured) {
    await destroyMatching(await direct(), RUN);
  }
});

test("JMAP only through the proxy: login, inbox, send, files, push, logout", async ({
  browser,
}) => {
  const context = await browser.newContext({ acceptDownloads: true });
  const traffic = record(context);
  const page = await context.newPage();
  const client = await direct();

  await signIn(page);
  // The page as the browser got it (page.request would skip the
  // browser's host mapping when the app runs on another port).
  const inboxPage = await page.reload();
  if (inboxPage === null) {
    throw new Error("No response for the inbox");
  }
  // CSP: pages connect to their own origin (and the image proxy), never
  // to Stalwart; its origin stays in form-action for the login.
  const csp = inboxPage.headers()["content-security-policy"] ?? "";
  const connect = /connect-src (?<values>[^;]+)/u.exec(csp)?.groups?.values;
  expect(connect).toBeDefined();
  expect(connect).not.toContain("8080");
  expect(csp).toMatch(/form-action 'self' http:\/\/localhost:8080/u);

  // The inbox shows (through the proxy).
  await expect(
    page.getByRole("region", { name: "Messages" }).getByRole("listitem").first()
  ).toBeVisible();

  // Send to herself with an attachment (upload through the proxy).
  const subject = `Proxy ${RUN} sent`;
  const box = await openComposer(page);
  await box.getByRole("combobox", { name: "To" }).fill(`${ALICE},`);
  await box.getByRole("textbox", { name: "Subject" }).fill(subject);
  await box
    .getByRole("textbox", { name: "Message" })
    .fill("Through the proxy.");
  await box.locator("input[type=file][multiple]").setInputFiles({
    buffer: Buffer.from(`attached ${RUN}`),
    mimeType: "text/plain",
    name: "notes.txt",
  });
  await expect(
    box.getByRole("list", { name: "Attachments" }).getByText("notes.txt")
  ).toBeVisible();
  await box.getByRole("button", { name: /^Send/u }).click();
  await expect(
    page.getByRole("region", { name: "Notifications" })
  ).toContainText("Sent");

  // Stalwart delivers it to her inbox; then it shows up by push (no
  // reload), and opens.
  const inboxId = await mailbox(client, "inbox");
  await expect
    .poll(
      async () => {
        // Not a subject filter: Stalwart indexes text a moment later.
        const [, latest] = await client.request((b) => {
          const query = b.call("Email/query", {
            accountId: client.primaryAccountId(),
            filter: { inMailbox: inboxId },
            limit: 10,
            sort: [{ isAscending: false, property: "receivedAt" }],
          });
          return [
            query,
            b.call("Email/get", {
              "#ids": query.ref("/ids"),
              accountId: client.primaryAccountId(),
              properties: ["subject"],
            }),
          ] as const;
        });
        return latest.list.filter((email) => email.subject === subject).length;
      },
      { timeout: 90_000 }
    )
    .toBe(1);
  const pushes = traffic.requests.filter((request) =>
    request.url().includes("/jmap-proxy/eventsource")
  );
  // The page listens for changes through the proxy.
  expect(pushes.length).toBeGreaterThan(0);
  await expect(row(page, subject)).toBeVisible({ timeout: 30_000 });
  await row(page, subject).click();
  await expect(reader(page)).toContainText("Through the proxy.");

  // Download the attachment (through the proxy, fetched and saved).
  const saved = page.waitForEvent("download");
  await reader(page)
    .getByRole("button", { name: "Download notes.txt" })
    .click();
  const download = await saved;
  const content = await readFile(await download.path(), "utf-8");
  expect(content).toBe(`attached ${RUN}`);

  // Push: an email delivered behind the app's back shows up by itself.
  await page
    .getByRole("link", { name: /^Inbox/u })
    .first()
    .click();
  const pushed = `Proxy ${RUN} pushed`;
  await importToInbox(client, pushed);
  await expect(row(page, pushed)).toBeVisible({ timeout: 60_000 });

  // The page itself: /auth/token is gone; the proxy refuses without its
  // header (CSRF) and works with it.
  const probes = await page.evaluate(async () => {
    const token = await fetch("/auth/token", {
      credentials: "same-origin",
      method: "POST",
    });
    const bare = await fetch("/jmap-proxy/session", {
      credentials: "same-origin",
    });
    const proxied = await fetch("/jmap-proxy/session", {
      credentials: "same-origin",
      headers: { "X-Nebula-Proxy": "1" },
    });
    return [token.status, bare.status, proxied.status];
  });
  expect(probes).toStrictEqual([404, 403, 200]);

  // Nothing carried a token, and nothing but the login went to Stalwart.
  const [accessToken = "", refreshToken = ""] = await sessionTokens(context);
  expect(accessToken.length).toBeGreaterThan(20);
  const authorized: string[] = [];
  const toStalwart: string[] = [];
  const allHeaders = await Promise.all(
    traffic.requests.map(async (request) => await headersOf(request))
  );
  for (const [index, request] of traffic.requests.entries()) {
    if (allHeaders[index]?.authorization !== undefined) {
      authorized.push(request.url());
    }
    const url = new URL(request.url());
    if (url.origin === env.url && !isLoginPath(url.pathname)) {
      toStalwart.push(`${request.method()} ${url.pathname}`);
    }
  }
  // The checks saw real traffic: the login on Stalwart, answers read.
  expect(
    traffic.requests.filter(
      (request) => new URL(request.url()).origin === env.url
    ).length
  ).toBeGreaterThan(0);
  expect(traffic.bodies.length).toBeGreaterThan(10);
  expect(authorized).toStrictEqual([]);
  expect(toStalwart).toStrictEqual([]);
  const leaks = traffic.bodies
    .filter(
      ({ text }) => text.includes(accessToken) || text.includes(refreshToken)
    )
    .map(({ url }) => url);
  expect(leaks).toStrictEqual([]);
  // JMAP really went through the proxy, push and files included.
  const proxied = new Set(
    traffic.requests
      .map((request) => new URL(request.url()))
      .filter(
        (url) => url.origin === APP && url.pathname.startsWith("/jmap-proxy/")
      )
      .map((url) => url.pathname.split("/")[2])
  );
  expect([...proxied]).toEqual(
    expect.arrayContaining([
      "api",
      "download",
      "eventsource",
      "session",
      "upload",
    ])
  );

  // Logout cuts access at once: the proxy refuses the very next request.
  await page.getByRole("button", { name: /^Account/u }).click();
  await page.getByRole("menuitem", { name: "Sign out" }).click();
  await expect(page).toHaveURL(/\/login/u);
  const jar = await context.cookies(APP);
  const cookies = jar.map((cookie) => cookie.name);
  expect(cookies).not.toContain(SESSION_COOKIE);
  expect(cookies).not.toContain(PROXY_COOKIE);
  const afterLogout = await page.evaluate(async () => {
    const response = await fetch("/jmap-proxy/session", {
      credentials: "same-origin",
      headers: { "X-Nebula-Proxy": "1" },
    });
    return response.status;
  });
  expect(afterLogout).toBe(401);
  await context.close();
});
