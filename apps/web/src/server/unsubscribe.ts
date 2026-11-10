// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// POST /api/unsubscribe { emailId } — one-click unsubscribe (RFC 8058).
//
// The browser only names the email. The BFF downloads its headers itself
// with the user's token, checks again that one-click is allowed
// (mail/unsubscribe.ts: https, List-Unsubscribe-Post, aligned DKIM pass from
// our server that signs both headers) and only then POSTs
// `List-Unsubscribe=One-Click` through net-guard: https on 443 only, pinned
// IP, no cookies, credentials or referrer (RFC 8058 §3.1, §6), short
// timeout, response body never read. Redirects are never followed: RFC 8058
// §3.1 says the sender MUST NOT answer with one (browsers turn a redirected
// POST into a GET), so any 3xx is reported as not confirmed.

import { AccountId, EmailId } from "@nebulamail/jmap-client";
import type { FetchRequest, GuardedFetch } from "@nebulamail/net-guard/node";
import { z } from "zod";

import {
  ONE_CLICK_BODY,
  UNSUBSCRIBE_PROPERTIES,
  unsubscribeHeaders,
  unsubscribeOptions,
} from "../mail/unsubscribe";
import type { AuthConfig } from "./auth/handlers";
import { isSameOrigin, json, withSessionToken } from "./auth/handlers";
import { connectAs } from "./jmap-connect";
import type { RateLimiter } from "./rate-limit";
import { bodyError, readJsonBody } from "./request-body";

export type { RateLimiter } from "./rate-limit";
export { createRateLimiter } from "./rate-limit";

export interface UnsubscribeDeps {
  /** JMAP (the user's own server). */
  readonly fetch: typeof fetch;
  /** The sender's server: net-guard only, with HTTPS_POLICY. */
  readonly guardedFetch: GuardedFetch;
  readonly rateLimit: RateLimiter;
}

export type UnsubscribeResponse =
  | { readonly ok: true }
  | {
      readonly ok: false;
      /**
       * `not-allowed`: the email doesn't qualify for one-click (any more).
       * `blocked`: net-guard refused the URL or a redirect (private address…).
       * `rejected`: the sender's server answered with an error.
       * `redirected`: it answered with a redirect (RFC 8058 §3.1 forbids
       *   it), so the unsubscribe is not confirmed; it is never followed.
       * `unreachable`: network error or timeout.
       */
      readonly error:
        | "not-allowed"
        | "blocked"
        | "rejected"
        | "redirected"
        | "unreachable";
    };

const MAX_REQUEST_BYTES = 1024;

// `accountId`: an email of a folder shared with the user (another account
// of the same session; checked below, and the server checks access).
const Body = z.strictObject({
  accountId: AccountId.optional(),
  emailId: EmailId,
});

const ONE_CLICK_REQUEST: FetchRequest = Object.freeze({
  body: Object.freeze({
    data: ONE_CLICK_BODY,
    type: "application/x-www-form-urlencoded",
  }),
  method: "POST",
  readBody: false,
  // RFC 8058 §3.1: a redirect is not a valid answer; following it would
  // turn the POST into a GET (net-guard does that after 301/302/303).
  redirects: "none",
});

/** Reasons net-guard gives for policy blocks (the rest are network trouble). */
const POLICY_BLOCKS = new Set([
  "scheme",
  "userinfo",
  "port",
  "private-ip",
  "too-many-redirects",
]);

const reply = (response: UnsubscribeResponse, status = 200): Response =>
  json(response, status);

export const handleUnsubscribe = async (
  request: Request,
  auth: AuthConfig,
  deps: UnsubscribeDeps
): Promise<Response> => {
  if (request.method !== "POST" || !isSameOrigin(request, auth.appUrl)) {
    return json({ error: "forbidden" }, 403);
  }
  const read = await readJsonBody(request, MAX_REQUEST_BYTES);
  if (!read.ok) {
    return json(bodyError(read.status), read.status);
  }
  const body = Body.safeParse(read.value);
  if (!body.success) {
    return json({ error: "invalid" }, 400);
  }
  return await withSessionToken(request, auth, async (token) => {
    const client = await connectAs(auth, deps.fetch, token.accessToken);
    const accountId = body.data.accountId ?? client.primaryAccountId();
    if (client.session.accounts[accountId] === undefined) {
      return json({ error: "not-found" }, 404);
    }
    if (!deps.rateLimit.take(client.session.username, auth.oidc.now())) {
      return json({ error: "rate-limited" }, 429);
    }

    const [result] = await client.request((b) => [
      b.call("Email/get", {
        accountId,
        ids: [body.data.emailId],
        properties: [...UNSUBSCRIBE_PROPERTIES],
      }),
    ]);
    const [email] = result.list;
    if (email === undefined) {
      return json({ error: "not-found" }, 404);
    }
    const { oneClick } = unsubscribeOptions(
      unsubscribeHeaders(email, client.session.apiUrl.hostname)
    );
    if (oneClick === null) {
      return reply({ error: "not-allowed", ok: false }, 422);
    }

    const sent = await deps.guardedFetch(oneClick, ONE_CLICK_REQUEST);
    if (!sent.ok) {
      return reply(
        {
          error: POLICY_BLOCKS.has(sent.reason) ? "blocked" : "unreachable",
          ok: false,
        },
        502
      );
    }
    // RFC 8058 §3.1: "The mail sender MUST NOT return an HTTPS redirect".
    if (sent.status >= 300 && sent.status <= 399) {
      return reply({ error: "redirected", ok: false }, 502);
    }
    // RFC 8058 doesn't define the answer; any 2xx means accepted.
    if (sent.status < 200 || sent.status > 299) {
      return reply({ error: "rejected", ok: false }, 502);
    }
    return reply({ ok: true });
  });
};
