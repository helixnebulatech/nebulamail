// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Preload relay: runs NEXT TO STALWART (same machine or
// private network), so the email never leaves the mail server.
//
//   Stalwart ──MTA Hook (whole email)──▶ relay ──URLs + hashed recipients──▶ proxy
//
// - Stalwart's MTA Hook posts every email to the relay (`POST
//   /hooks/stalwart`, Bearer `hookSecret`). The relay reads it locally with
//   the same code as the proxy (`../preload/parse.ts`), drops trackers,
//   hashes the envelope recipients with the proxy's index key, and sends
//   the proxy only `{ urls, recipients }` (`../preload/url-hook.ts`).
// - The proxy (a host like Vercel) sees the image URLs of every incoming
//   email and when it arrived, never its subject, sender, recipients,
//   text or attachments. Image URLs can still say something (a newsletter's
//   domain, an id in the URL): the README explains it.
// - Never blocks mail: once authenticated, Stalwart always gets `accept`.
//   Without email tokens the email is read and the proxy called in the
//   background; with them the relay waits for the token (at most
//   `timeoutMs`) and, if the proxy does not answer, still accepts (forged
//   token headers are removed). An email with too much HTML to parse while
//   Stalwart waits goes in the background, without a token, as in the proxy.
// - The relay parses hostile HTML (parse5, in worker threads with a deadline)
//   on the mail server: run it as its own user, on loopback, sandboxed (README).
// - `hookSecret` and `upstreamSecret` must differ: Stalwart's configuration
//   then never holds a key the proxy accepts.

import { isTrackerUrl } from "@nebulamail/mail-render/trackers";
import type { ConcurrencyLimiter } from "@nebulamail/net-guard";
import {
  createConcurrencyLimiter,
  QueueFullError,
} from "@nebulamail/net-guard";

import { readLimited, sameSecret } from "../body";
import type { ImageUrlExtractor, RequestContext } from "../config";
import { DEFAULT_LIMITS } from "../config";
import {
  DEFAULT_MAX_MESSAGE_BYTES,
  MIN_PRELOAD_SECRET_LENGTH,
} from "../preload/hook";
import {
  ACCEPT,
  acceptWith,
  MAX_HOOK_RECIPIENTS,
  parseHookMessage,
  readHookMessage,
  tokenModifications,
  tooMuchHtml,
} from "../preload/parse";
import type { HookMessage, ParsedEmail } from "../preload/parse";
import { createRecipientHasher, hashRecipients } from "../preload/recipients";
import type { UrlHookRequest } from "../preload/url-hook";
import {
  MAX_HOOK_URLS,
  UrlHookAnswer,
  URL_HOOK_PATH,
} from "../preload/url-hook";

export const RELAY_HOOK_PATH = "/hooks/stalwart";
export const DEFAULT_RELAY_TIMEOUT_MS = 5000;

export class RelayConfigError extends Error {
  override readonly name = "RelayConfigError";
}

/** What the relay logs: counts and reasons only, never URLs or addresses. */
export type RelayLogEvent =
  | {
      readonly event: "relay-sent";
      readonly images: number;
      readonly recipients: number;
      readonly token: boolean;
    }
  | {
      readonly event: "relay-skipped";
      readonly reason:
        | "busy"
        | "invalid"
        | "no-image"
        | "no-recipient"
        | "too-large"
        | "too-many-recipients";
    }
  | { readonly event: "relay-token-skipped"; readonly reason: "too-large" }
  | {
      readonly event: "relay-upstream-error";
      /** HTTP status, or 0 for a network error or timeout. */
      readonly status: number;
    }
  | { readonly event: "rejected"; readonly status: number };

export interface RelayConfig {
  /** Bearer token of Stalwart's MTA Hook (`httpAuth`). 32+ characters. */
  readonly hookSecret: string;
  /** The proxy's base URL, e.g. `https://img.example.com`. */
  readonly upstream: string;
  /** The proxy's `IMAGE_PRELOAD_SECRET`. 32+ characters, not `hookSecret`. */
  readonly upstreamSecret: string;
  /** The proxy's `IMAGE_PRELOAD_INDEX_KEY`. Default: `upstreamSecret`. */
  readonly indexKey?: string;
  /**
   * Image URL extraction: `createWorkerExtractor` from
   * `@nebulamail/mail-render/server-extract` (parsed in a worker thread with
   * a deadline).
   */
  readonly extractor: ImageUrlExtractor;
  /** Ask the proxy for an email token header (static build). Default off. */
  readonly emailTokens?: boolean;
  /** Hook bodies over this are accepted without preloading. Default 25 MB. */
  readonly maxMessageBytes?: number;
  /** Image URLs sent per email. Default: the proxy's default (60). */
  readonly maxImagesPerEmail?: number;
  /** Proxy request timeout. Default 5 s. */
  readonly timeoutMs?: number;
  /** Proxy requests at the same time, and waiting. Defaults 4 and 100. */
  readonly concurrency?: number;
  readonly maxQueue?: number;
  /** Default: mail-render's tracker list. */
  readonly isTracker?: (url: string) => boolean;
  /** For tests. Default: the global `fetch`. */
  readonly fetch?: typeof fetch;
  readonly log?: (event: RelayLogEvent) => void;
}

const LOOPBACK = new Set(["localhost", "127.0.0.1", "[::1]"]);

/** https, or http only on loopback (a proxy on the same machine). */
const upstreamUrl = (value: string): URL => {
  let url: URL;
  try {
    url = new URL(URL_HOOK_PATH, value);
  } catch {
    throw new RelayConfigError("preload relay: the upstream is not a URL");
  }
  const local = LOOPBACK.has(url.hostname);
  if (
    !(url.protocol === "https:" || (url.protocol === "http:" && local)) ||
    url.username !== "" ||
    url.password !== ""
  ) {
    throw new RelayConfigError(
      "preload relay: the upstream must be https (http only on loopback), without credentials"
    );
  }
  return url;
};

const bearer = (header: string | null): string | null => {
  const match = /^Bearer (?<token>\S+)$/u.exec(header ?? "");
  return match?.groups?.token ?? null;
};

const json = (status: number, body: unknown): Response =>
  Response.json(body, {
    headers: {
      "Cache-Control": "no-store",
      "X-Content-Type-Options": "nosniff",
    },
    status,
  });

export type RelayHandler = (
  request: Request,
  context?: RequestContext
) => Promise<Response>;

export const createRelay = (config: RelayConfig): RelayHandler => {
  for (const [name, value] of [
    ["hookSecret", config.hookSecret],
    ["upstreamSecret", config.upstreamSecret],
    ["indexKey", config.indexKey ?? config.upstreamSecret],
  ] as const) {
    if (value.length < MIN_PRELOAD_SECRET_LENGTH) {
      throw new RelayConfigError(
        `preload relay: ${name} needs ${String(MIN_PRELOAD_SECRET_LENGTH)}+ characters`
      );
    }
  }
  if (config.hookSecret === config.upstreamSecret) {
    throw new RelayConfigError(
      "preload relay: hookSecret and upstreamSecret must be different"
    );
  }
  const target = upstreamUrl(config.upstream);
  const doFetch = config.fetch ?? globalThis.fetch.bind(globalThis);
  const log = (event: RelayLogEvent): void => {
    config.log?.(event);
  };
  const hasher = createRecipientHasher(
    config.indexKey ?? config.upstreamSecret
  );
  const isTracker =
    config.isTracker ?? ((url: string) => isTrackerUrl(url) !== null);
  const maxImages = Math.min(
    config.maxImagesPerEmail ?? DEFAULT_LIMITS.maxImagesPerEmail,
    MAX_HOOK_URLS
  );
  const maxBytes = config.maxMessageBytes ?? DEFAULT_MAX_MESSAGE_BYTES;
  const timeoutMs = config.timeoutMs ?? DEFAULT_RELAY_TIMEOUT_MS;
  const emailTokens = config.emailTokens === true;
  const jobs: ConcurrencyLimiter = createConcurrencyLimiter({
    max: config.concurrency ?? 4,
    maxQueue: config.maxQueue ?? 100,
  });

  /** The proxy's answer, or null (logged) on any failure. */
  const send = async (
    body: UrlHookRequest
  ): Promise<{ readonly token?: string | undefined } | null> => {
    let response: Response;
    try {
      response = await doFetch(target, {
        body: JSON.stringify(body),
        headers: {
          authorization: `Bearer ${config.upstreamSecret}`,
          "content-type": "application/json",
        },
        method: "POST",
        redirect: "error",
        signal: AbortSignal.timeout(timeoutMs),
      });
    } catch {
      log({ event: "relay-upstream-error", status: 0 });
      return null;
    }
    if (!response.ok) {
      log({ event: "relay-upstream-error", status: response.status });
      await response.body?.cancel();
      return null;
    }
    try {
      const answer = UrlHookAnswer.parse(await response.json());
      log({
        event: "relay-sent",
        images: body.urls.length,
        recipients: body.recipients.length,
        token: answer.token !== undefined,
      });
      return answer;
    } catch {
      log({ event: "relay-upstream-error", status: response.status });
      return null;
    }
  };

  /** The request for the proxy, or null (logged) when nothing is to be sent. */
  const prepare = async (
    parsed: ParsedEmail
  ): Promise<UrlHookRequest | null> => {
    if (parsed.urls.length === 0) {
      log({ event: "relay-skipped", reason: "no-image" });
      return null;
    }
    if (parsed.recipients.length > MAX_HOOK_RECIPIENTS) {
      log({ event: "relay-skipped", reason: "too-many-recipients" });
      return null;
    }
    const recipients = await hashRecipients(hasher, parsed.recipients);
    if (recipients.length === 0) {
      log({ event: "relay-skipped", reason: "no-recipient" });
      return null;
    }
    return { recipients, urls: parsed.urls, v: 1 };
  };

  const parse = async (message: HookMessage): Promise<ParsedEmail> =>
    await parseHookMessage(
      { extractor: config.extractor, isTracker, maxImages },
      message
    );

  /** Read the email and call the proxy, after answering Stalwart. */
  const inBackground = (
    message: HookMessage,
    context: RequestContext
  ): void => {
    const job = (async () => {
      try {
        await jobs.run(async () => {
          const body = await prepare(await parse(message));
          if (body !== null) {
            await send(body);
          }
        });
      } catch (error) {
        log({
          event: "relay-skipped",
          reason: error instanceof QueueFullError ? "busy" : "invalid",
        });
      }
    })();
    context.waitUntil?.(job);
  };

  const hook = async (
    request: Request,
    context: RequestContext
  ): Promise<Response> => {
    const token = bearer(request.headers.get("authorization"));
    if (token === null || !(await sameSecret(token, config.hookSecret))) {
      log({ event: "rejected", status: 401 });
      return json(401, { error: "unauthorized" });
    }
    const body = await readLimited(request, maxBytes);
    if (!body.ok) {
      log({
        event: "relay-skipped",
        reason: body.code === "body-too-large" ? "too-large" : "invalid",
      });
      return json(200, ACCEPT);
    }
    const message = readHookMessage(body.text);
    if (message === null) {
      log({ event: "relay-skipped", reason: "invalid" });
      return json(200, ACCEPT);
    }
    if (!emailTokens) {
      inBackground(message, context);
      return json(200, ACCEPT);
    }
    if (tooMuchHtml(message)) {
      log({ event: "relay-token-skipped", reason: "too-large" });
      inBackground(message, context);
      return json(200, acceptWith(tokenModifications(message.headers, null)));
    }
    const prepared = await prepare(await parse(message));
    // The answer carries the token: wait for the proxy (bounded). With no
    // token, a forged one is still removed.
    const answer =
      prepared === null ? null : await send({ ...prepared, token: true });
    return json(
      200,
      acceptWith(tokenModifications(message.headers, answer?.token ?? null))
    );
  };

  return async (request, context = {}) => {
    const { pathname } = new URL(request.url);
    if (pathname === RELAY_HOOK_PATH && request.method === "POST") {
      return await hook(request, context);
    }
    if (pathname === "/healthz" && request.method === "GET") {
      return json(200, { ok: true, relay: true });
    }
    return json(404, { error: "not-found" });
  };
};
