// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The static zip served by Stalwart alone (docs/static.md),
// as an operator would set it up (e2e-static/setup.ts): the headers and
// CSP of the page, the login in the browser (OAuth code + PKCE against
// Stalwart, public client), the inbox and a message through JMAP, and the
// logout; and that the browser never asks anything of another origin (no
// server of ours, no remote image). Also, at the protocol level, what
// revoking one login does to the other logins of the same user and client
// (W24).

import { createHash, randomBytes } from "node:crypto";

import type { Browser, Page } from "@playwright/test";
import { expect, test } from "@playwright/test";
import { z } from "zod";

import { allowIfAsked, reader, row } from "../e2e-live/ui";
import { OAUTH_SCOPE } from "../src/lib/oauth-scope";
import {
  BASE_PATH,
  BODY_TEXT,
  CLIENT_ID,
  DOMAIN,
  passwordEnv,
  REMOTE_IMAGE,
  RESPONSE_HEADERS,
  STALWART_URL,
  SUBJECT,
  USER,
} from "./env";
import { PROXY_URL, SeenSchema } from "./proxy";
import type { Seen } from "./proxy";

const ORIGIN = new URL(STALWART_URL).origin;
const REDIRECT_URI = `${ORIGIN}${BASE_PATH}/login`;

/**
 * The paths the guide's `Http.allowedEndpoints` rule leaves open on the
 * app's host (docs/static.md, «Use a subdomain»): everything the app
 * asks for must be one of them, or that rule would break it.
 */
const allowedPath = (path: string): boolean =>
  path === BASE_PATH ||
  path.startsWith(`${BASE_PATH}/`) ||
  path.startsWith("/jmap") ||
  // Stalwart's login form asks for its logo as `/logo/`.
  path.startsWith("/logo/") ||
  // Its style and script, `/ui-<hash>.css|js` (since Stalwart 1.0.3; the
  // hash changes with every version).
  /^\/ui-[0-9a-f]+\.(?:css|js)$/u.test(path) ||
  [
    "/.well-known/jmap",
    "/.well-known/oauth-authorization-server",
    "/login",
    // Stalwart 1.0's login page: the client it signs in for, then the form.
    "/api/auth/client",
    "/api/auth",
    "/auth/token",
    // Sign-out revokes the session on the server.
    "/auth/revoke",
    "/logo",
  ].includes(path);

/** What the browser asked for so far: the proxy saw it all (proxy.ts). */
const seenByProxy = async (): Promise<Seen[]> => {
  const response = await fetch(`${PROXY_URL}/requests`);
  return z.array(SeenSchema).parse(await response.json());
};

/** CSP violations: Chromium reports each one on the console. */
const recordViolations = (page: Page): string[] => {
  const violations: string[] = [];
  page.on("console", (message) => {
    if (message.text().includes("Content Security Policy")) {
      violations.push(message.text());
    }
  });
  return violations;
};

test("Stalwart serves the zip with the guide's headers and the page's CSP", async ({
  request,
}) => {
  const page = await request.get(`${BASE_PATH}/`, { maxRedirects: 0 });
  expect(page.status()).toBe(200);
  const headers = page.headers();
  for (const [name, value] of Object.entries(RESPONSE_HEADERS)) {
    expect(headers[name.toLowerCase()], name).toBe(value);
  }
  expect(headers["content-type"]).toContain("text/html");
  expect(headers["cache-control"]).toBe("no-cache");
  // The guide sets no Content-Security-Policy header: `responseHeaders`
  // replaces Stalwart's own, and its login page keeps `frame-ancestors
  // 'none'` (docs/static.md, «Response headers»).
  expect(headers["content-security-policy"]).toBeUndefined();
  const login = await request.get("/login");
  expect(login.headers()["content-security-policy"]).toBe(
    "frame-ancestors 'none'"
  );

  const html = await page.text();
  // Built for the prefix Stalwart mounts it at.
  expect(html).toContain(`href="${BASE_PATH}/assets/`);
  const csp =
    /<meta http-equiv="Content-Security-Policy" content="(?<csp>[^"]+)"/u.exec(
      html
    )?.groups?.csp ?? "";
  const directives = new Map(
    csp
      .replaceAll("&#39;", "'")
      .split(";")
      .map((part) => part.trim().split(/\s+/u))
      .map(([name = "", ...values]) => [name, values])
  );
  const scriptSrc = directives.get("script-src") ?? [];
  expect(scriptSrc).toContain("'strict-dynamic'");
  // Never 'self': Stalwart serves attachments on this same origin.
  expect(scriptSrc).not.toContain("'self'");
  expect(scriptSrc.some((source) => source.startsWith("'sha"))).toBe(true);
  expect(directives.get("connect-src")).toEqual(["'self'"]);
  expect(directives.get("object-src")).toEqual(["'none'"]);
  expect(directives.get("base-uri")).toEqual(["'self'"]);
  expect(directives.get("form-action")).toEqual(["'self'"]);
  expect(directives.get("require-trusted-types-for")).toEqual(["'script'"]);

  // An SPA: any path under the prefix is the same page; the bare prefix
  // redirects to it.
  const deep = await request.get(`${BASE_PATH}/mail/inbox`);
  expect(deep.status()).toBe(200);
  expect(await deep.text()).toBe(html);
  const bare = await request.get(BASE_PATH, { maxRedirects: 0 });
  expect(bare.status()).toBe(302);
  expect(bare.headers().location).toBe(`${BASE_PATH}/`);

  // Hashed files: cached for good.
  const script = /new URL\("(?<src>[^"]+\.js)"/u.exec(html)?.groups?.src;
  expect(script, "the entry script in the loader").toBeDefined();
  const asset = await request.get(
    new URL(script ?? "", `${ORIGIN}${BASE_PATH}/`).href
  );
  expect(asset.status()).toBe(200);
  expect(asset.headers()["cache-control"]).toBe(
    "public, max-age=31536000, immutable"
  );
});

test("login with PKCE, the inbox, a message and logout, all on Stalwart's origin", async ({
  page,
}) => {
  const password = process.env[passwordEnv] ?? "";
  expect(password, "set by e2e-static/setup.ts").not.toBe("");
  await fetch(`${PROXY_URL}/requests`, { method: "DELETE" });
  const violations = recordViolations(page);

  await page.goto(`${BASE_PATH}/login`);
  const authorize = page.waitForRequest(
    (request) =>
      request.isNavigationRequest() &&
      new URL(request.url()).searchParams.has("code_challenge")
  );
  await page.getByRole("button", { exact: true, name: "Sign in" }).click();

  // The app leaves for Stalwart's own page with a PKCE S256 challenge.
  const authorizeRequest = await authorize;
  const authorizeUrl = new URL(authorizeRequest.url());
  const params = authorizeUrl.searchParams;
  expect(authorizeUrl.origin).toBe(ORIGIN);
  expect(params.get("response_type")).toBe("code");
  expect(params.get("client_id")).toBe(CLIENT_ID);
  expect(params.get("redirect_uri")).toBe(REDIRECT_URI);
  expect(params.get("code_challenge_method")).toBe("S256");
  expect(params.get("code_challenge")).toMatch(/^[\w-]{43}$/u);
  expect(params.get("state")?.length).toBeGreaterThanOrEqual(16);

  await page.getByRole("textbox", { name: "Username" }).fill(USER);
  await page.getByLabel("Password").fill(password);
  await page.getByRole("button", { exact: true, name: "Sign in" }).click();
  // Stalwart 1.0 asks alice to allow the app (a registered client: no
  // "unverified" mark), with the four scopes.
  await expect(page.locator("#allow-btn")).toBeVisible();
  await expect(page.locator("#client-unverified")).toBeHidden();
  await allowIfAsked(page, `${ORIGIN}${BASE_PATH}/mail/inbox`);

  // The inbox through JMAP, and the message.
  await row(page, SUBJECT).click();
  await expect(reader(page)).toContainText(SUBJECT);
  await expect(
    reader(page).locator("iframe").first().contentFrame().getByText(BODY_TEXT)
  ).toBeVisible();

  // The token exchange, from the session worker: code + verifier, no
  // client secret (a public client).
  const afterLogin = await seenByProxy();
  const tokens = afterLogin.filter(
    (seen) => new URL(seen.url).pathname === "/auth/token"
  );
  expect(tokens.length).toBeGreaterThan(0);
  const exchange = new URLSearchParams(tokens[0]?.body ?? "");
  expect(exchange.get("grant_type")).toBe("authorization_code");
  expect(exchange.get("client_id")).toBe(CLIENT_ID);
  expect(exchange.get("redirect_uri")).toBe(REDIRECT_URI);
  expect(exchange.get("code_verifier")).toMatch(/^[\w-]{43,128}$/u);
  expect(exchange.has("client_secret")).toBe(false);

  // Nothing of the login in the page's storage (the refresh token lives
  // only in the session worker's memory).
  const stored = await page.evaluate(() =>
    JSON.stringify(
      [localStorage, sessionStorage].map((storage) =>
        Object.fromEntries(
          Array.from({ length: storage.length }, (_, index) => {
            const key = storage.key(index) ?? "";
            return [key, storage.getItem(key)];
          })
        )
      )
    )
  );
  expect(stored).not.toMatch(/token|refresh|verifier/iu);

  // Sign out: back to the login page, and the app needs a new login.
  await page.getByRole("button", { name: `Account: ${USER}` }).click();
  await page.getByRole("menuitem", { name: "Sign out" }).click();
  await expect(page).toHaveURL(new RegExp(`${BASE_PATH}/login`, "u"));
  await expect(
    page.getByRole("button", { exact: true, name: "Sign in" })
  ).toBeVisible();
  await page.goto(`${BASE_PATH}/mail/inbox`);
  await expect(page).toHaveURL(new RegExp(`${BASE_PATH}/login`, "u"));

  // The worker revoked the session on the server (Stalwart 1.0 deletes the
  // whole grant): the refresh token it held no longer works.
  const afterLogout = await seenByProxy();
  const revocations = afterLogout.filter(
    (seen) => new URL(seen.url).pathname === "/auth/revoke"
  );
  expect(revocations).toHaveLength(1);
  const revocation = new URLSearchParams(revocations[0]?.body ?? "");
  expect(revocation.get("client_id")).toBe(CLIENT_ID);
  expect(revocation.get("token_type_hint")).toBe("refresh_token");
  const refreshAfter = await fetch(`${STALWART_URL}/auth/token`, {
    body: new URLSearchParams({
      client_id: CLIENT_ID,
      grant_type: "refresh_token",
      refresh_token: revocation.get("token") ?? "",
    }),
    method: "POST",
  });
  expect(refreshAfter.status).toBe(400);
  expect(await refreshAfter.json()).toMatchObject({ error: "invalid_grant" });

  // Only Stalwart's origin, only the paths the guide's rule allows, never
  // the email's remote image, and no CSP violation.
  const seen = await seenByProxy();
  const urls = seen.map((item) => new URL(item.url));
  expect(
    urls.filter((url) => url.origin !== ORIGIN).map((url) => url.href)
  ).toEqual([]);
  expect(
    urls.filter((url) => !allowedPath(url.pathname)).map((url) => url.href)
  ).toEqual([]);
  expect(urls.map((url) => url.href)).not.toContain(REMOTE_IMAGE);
  expect(urls.some((url) => url.pathname.startsWith("/jmap"))).toBe(true);
  expect(violations).toEqual([]);
});

const MetadataSchema = z.object({
  authorization_endpoint: z.string(),
  revocation_endpoint: z.string().optional(),
  token_endpoint: z.string(),
});
type Metadata = z.infer<typeof MetadataSchema>;

const TokensSchema = z.object({
  access_token: z.string(),
  refresh_token: z.string(),
});
type Tokens = z.infer<typeof TokensSchema>;

const escapeRegExp = (text: string): string =>
  text.replaceAll(/[.*+?^${}()|[\]\\]/gu, String.raw`\$&`);

/**
 * One real login of alice in a browser context of its own, as the app does
 * it (code + PKCE S256, public client), but the code is exchanged here:
 * the app keeps its tokens in the session worker, out of the test's reach.
 */
const login = async (
  browser: Browser,
  metadata: Metadata,
  password: string
): Promise<Tokens> => {
  const context = await browser.newContext();
  try {
    const page = await context.newPage();
    const verifier = randomBytes(32).toString("base64url");
    const state = randomBytes(16).toString("base64url");
    let redirected: URL | undefined;
    // The app is not loaded: it would exchange the code itself.
    await page.route(`${REDIRECT_URI}**`, async (route) => {
      redirected = new URL(route.request().url());
      await route.fulfill({ body: "", contentType: "text/html" });
    });
    const authorize = new URL(metadata.authorization_endpoint);
    authorize.search = new URLSearchParams({
      client_id: CLIENT_ID,
      code_challenge: createHash("sha256").update(verifier).digest("base64url"),
      code_challenge_method: "S256",
      redirect_uri: REDIRECT_URI,
      response_type: "code",
      // What the app asks for: Stalwart refuses other scopes for this
      // registered client.
      scope: OAUTH_SCOPE,
      state,
    }).toString();
    await page.goto(authorize.href);
    await page.getByRole("textbox", { name: "Username" }).fill(USER);
    await page.getByLabel("Password").fill(password);
    await page.getByRole("button", { exact: true, name: "Sign in" }).click();
    await allowIfAsked(
      page,
      new RegExp(`^${escapeRegExp(REDIRECT_URI)}\\?`, "u")
    );
    expect(redirected?.searchParams.get("state")).toBe(state);
    const code = redirected?.searchParams.get("code") ?? "";
    expect(code).not.toBe("");
    const response = await fetch(metadata.token_endpoint, {
      body: new URLSearchParams({
        client_id: CLIENT_ID,
        code,
        code_verifier: verifier,
        grant_type: "authorization_code",
        redirect_uri: REDIRECT_URI,
      }),
      method: "POST",
    });
    expect(response.status).toBe(200);
    return TokensSchema.parse(await response.json());
  } finally {
    await context.close();
  }
};

const refresh = async (
  metadata: Metadata,
  refreshToken: string
): Promise<Response> =>
  await fetch(metadata.token_endpoint, {
    body: new URLSearchParams({
      client_id: CLIENT_ID,
      grant_type: "refresh_token",
      refresh_token: refreshToken,
    }),
    method: "POST",
  });

const sessionStatus = async (accessToken: string): Promise<number> => {
  const response = await fetch(`${STALWART_URL}/jmap/session`, {
    headers: { authorization: `Bearer ${accessToken}` },
  });
  await response.body?.cancel();
  return response.status;
};

// The session worker revokes the old session before a new login, in case
// Stalwart revoked every grant of the user and client at once. This tells
// whether it revokes only the grant of the token it is given.
test("revoking one grant leaves the other grants of the same user and client", async ({
  browser,
}) => {
  const password = process.env[passwordEnv] ?? "";
  expect(password, "set by e2e-static/setup.ts").not.toBe("");
  const metadataResponse = await fetch(
    `${STALWART_URL}/.well-known/oauth-authorization-server`
  );
  const metadata = MetadataSchema.parse(await metadataResponse.json());
  const first = await login(browser, metadata, password);
  const second = await login(browser, metadata, password);
  expect(second.refresh_token).not.toBe(first.refresh_token);

  const revoked = await fetch(
    metadata.revocation_endpoint ?? `${STALWART_URL}/auth/revoke`,
    {
      body: new URLSearchParams({
        client_id: CLIENT_ID,
        token: first.refresh_token,
        token_type_hint: "refresh_token",
      }),
      method: "POST",
    }
  );
  expect(revoked.status).toBe(200);

  const firstRefresh = await refresh(metadata, first.refresh_token);
  expect(firstRefresh.status, "the revoked refresh token still works").toBe(
    400
  );
  expect(await firstRefresh.json()).toMatchObject({ error: "invalid_grant" });

  // Whether the first access token still works until it expires is not the
  // question: only noted.
  const firstAccess = await sessionStatus(first.access_token);
  test.info().annotations.push({
    description: `GET /jmap/session with the revoked grant's access token: ${firstAccess}`,
    type: "access token after revoke",
  });

  const otherGrantGone =
    "Stalwart revoked the other login too (every grant of the user and " +
    "client): src/session/worker.ts must keep revoking the old session " +
    "before the new login";
  expect(await sessionStatus(second.access_token), otherGrantGone).toBe(200);
  const secondRefresh = await refresh(metadata, second.refresh_token);
  expect(secondRefresh.status, otherGrantGone).toBe(200);
  TokensSchema.parse(await secondRefresh.json());
});

// --- HTML attachments ---------------------------------------------------

const MAIL_CAPS = ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"];
const MARK = "attachment-script-ran";
const HTML = `<!doctype html><title>x</title><script>document.title = "${MARK}"; window.top.postMessage("${MARK}", "*")</script><p>hostile attachment</p>`;

const basic = (password: string): string =>
  `Basic ${Buffer.from(`${USER}:${password}`).toString("base64")}`;

const Session = z.object({
  downloadUrl: z.string(),
  primaryAccounts: z.object({ "urn:ietf:params:jmap:mail": z.string() }),
});

const Responses = z.object({
  methodResponses: z.array(
    z.tuple([z.string(), z.record(z.string(), z.unknown()), z.string()])
  ),
});

const jmap = async (
  password: string,
  calls: readonly (readonly [string, Record<string, unknown>])[]
): Promise<Record<string, unknown>[]> => {
  const response = await fetch(`${STALWART_URL}/jmap/`, {
    body: JSON.stringify({
      methodCalls: calls.map(([name, args], i) => [name, args, String(i)]),
      using: MAIL_CAPS,
    }),
    headers: {
      authorization: basic(password),
      "content-type": "application/json",
    },
    method: "POST",
  });
  return Responses.parse(await response.json()).methodResponses.map(
    ([, args]) => args
  );
};

/** Imports an email with an HTML attachment; the attachment's URL. */
const seedHtmlAttachment = async (password: string): Promise<string> => {
  const sessionResponse = await fetch(`${STALWART_URL}/jmap/session`, {
    headers: { authorization: basic(password) },
  });
  const session = Session.parse(await sessionResponse.json());
  const accountId = session.primaryAccounts["urn:ietf:params:jmap:mail"];
  const [inboxes] = await jmap(password, [
    ["Mailbox/query", { accountId, filter: { role: "inbox" } }],
  ]);
  const [inbox] = z.object({ ids: z.array(z.string()) }).parse(inboxes).ids;
  const boundary = randomBytes(8).toString("hex");
  const raw = [
    `From: Mallory <mallory@${DOMAIN}>`,
    `To: ${USER}`,
    "Subject: HTML attachment check",
    `Date: ${new Date().toUTCString()}`,
    `Message-ID: <${randomBytes(8).toString("hex")}@${DOMAIN}>`,
    "MIME-Version: 1.0",
    `Content-Type: multipart/mixed; boundary="${boundary}"`,
    "",
    `--${boundary}`,
    "Content-Type: text/plain; charset=utf-8",
    "",
    "See the attachment.",
    `--${boundary}`,
    'Content-Type: text/html; charset=utf-8; name="invoice.html"',
    'Content-Disposition: inline; filename="invoice.html"',
    "",
    HTML,
    `--${boundary}--`,
    "",
  ].join("\r\n");
  const upload = await fetch(`${STALWART_URL}/jmap/upload/${accountId}/`, {
    body: raw,
    headers: {
      authorization: basic(password),
      "content-type": "message/rfc822",
    },
    method: "POST",
  });
  const { blobId } = z
    .object({ blobId: z.string() })
    .parse(await upload.json());
  const [imported] = await jmap(password, [
    [
      "Email/import",
      {
        accountId,
        emails: {
          m: { blobId, keywords: {}, mailboxIds: { [inbox ?? ""]: true } },
        },
      },
    ],
  ]);
  const emailId = z
    .object({ created: z.object({ m: z.object({ id: z.string() }) }) })
    .parse(imported).created.m.id;
  const [got] = await jmap(password, [
    ["Email/get", { accountId, ids: [emailId], properties: ["attachments"] }],
  ]);
  const part = z
    .object({
      list: z.array(
        z.object({
          attachments: z.array(
            z.object({ blobId: z.string(), type: z.string() })
          ),
        })
      ),
    })
    .parse(got)
    .list[0]?.attachments.find((item) => item.type === "text/html");
  if (part === undefined) {
    throw new Error("no HTML attachment");
  }
  // RFC 8620 §2: {accountId}, {blobId}, {type}, {name}.
  return session.downloadUrl
    .replace("{accountId}", encodeURIComponent(accountId))
    .replace("{blobId}", encodeURIComponent(part.blobId))
    .replace("{type}", encodeURIComponent("text/html"))
    .replace("{name}", "invoice.html");
};

// The static app lives on Stalwart's origin, which also serves every blob
// at /jmap/download/ (docs/static.md). An HTML attachment opened as a page
// there would run in the app's origin, next to the session:
// without credentials (a link, a navigation) Stalwart refuses, a real
// access token in the URL doesn't work either, and with credentials (only
// `fetch` sends them, and it never renders) it is a download, not a page.
test("an HTML attachment never opens as a page on the app's origin", async ({
  browser,
  page,
}) => {
  const password = process.env[passwordEnv] ?? "";
  expect(password, "set by e2e-static/setup.ts").not.toBe("");
  const url = await seedHtmlAttachment(password);
  expect(url.startsWith(STALWART_URL), url).toBeTruthy();
  const metadataResponse = await fetch(
    `${STALWART_URL}/.well-known/oauth-authorization-server`
  );
  const metadata = MetadataSchema.parse(await metadataResponse.json());
  const { access_token: token } = await login(browser, metadata, password);

  // With the app's own credentials: a download even for a part marked
  // inline (Stalwart 1.0.1), and never sniffed into a page.
  const withAuth = await fetch(url, {
    headers: { authorization: `Bearer ${token}` },
  });
  await withAuth.body?.cancel();
  expect(withAuth.status).toBe(200);
  expect(withAuth.headers.get("content-disposition") ?? "").toMatch(
    /^attachment\b/u
  );
  expect(withAuth.headers.get("x-content-type-options")).toBe("nosniff");

  // Without credentials, as a link or a navigation: refused.
  const anonymous = await fetch(url);
  await anonymous.body?.cancel();
  expect(anonymous.status).toBe(401);

  // The same real token in the URL (some servers take `?access_token=`).
  const viaQuery = await fetch(
    `${url}${url.includes("?") ? "&" : "?"}access_token=${encodeURIComponent(token)}`
  );
  await viaQuery.body?.cancel();
  expect(viaQuery.status).toBe(401);

  // The browser navigating to it: refused, no page.
  const response = await page.goto(url);
  expect(response?.status()).toBe(401);
});
