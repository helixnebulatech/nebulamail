// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Node adapter: fetch a public URL with the connection
// pinned to the validated IP.
//
// How the pinning works: for every hop we run the full guard (`guardUrl`),
// which resolves the name ONCE and validates every A/AAAA answer. Then we
// build a throw-away `undici.Agent` whose custom connector dials one of those
// validated IPs directly (the IP is passed as the socket host, so Node never
// does a DNS lookup of its own; TLS still verifies the certificate against
// the original name via `servername`). There is no second resolution, hence
// no DNS rebinding window. IP literals take the same path: their "resolved"
// address is the literal, validated by `parseTargetUrl`.
//
// The connector also refuses any origin other than the validated one, and a
// `lookup` that always fails is installed as a tripwire in case Node ever
// tries to resolve anyway.
//
// Deno runs this adapter unchanged (it loads the real undici). Under Bun,
// `createGuardedFetch` returns the Bun adapter instead (`../bun/fetch.ts`).
// The redirect loop, body limits and decompression are shared with it
// (`../http/`).
//
// This module never logs. Errors are reduced to a `BlockReason` so no URL,
// address or header leaks into logs through exception messages.

import type { Readable } from "node:stream";

import { Agent } from "undici";

import { createBunFetchInternal, isBun } from "../bun/fetch";
import { GuardError } from "../errors";
import {
  budgetCharge,
  checkContentType,
  IMAGE_ACCEPT,
  REDIRECT_STATUSES,
  USER_AGENT,
} from "../fetch-types";
import type { FetchLimits, FetchRequest, GuardedFetch } from "../fetch-types";
import { DROPPED_HEADERS, decoderFor, readBody } from "../http/body";
import { guardedFetchFromHops } from "../http/hops";
import type { GuardedFetchOptions, Hop, HopRequest } from "../http/hops";
import { checkImage } from "../image";
import type { ImageType } from "../image";
import type { ValidatedTarget } from "../resolve";
import type { BlockReason } from "../types";
import { pinnedConnector } from "./connector";
import type { Dial } from "./connector";

export type {
  FetchFailureReason,
  FetchLimits,
  FetchRequest,
  FetchResult,
  GuardedFetch,
  RedirectMode,
  RequestBody,
} from "../fetch-types";
export { MAX_REQUEST_BODY_BYTES } from "../http/hops";
export type { GuardedFetchOptions } from "../http/hops";
export {
  cancelableResolver,
  createCancelableResolver,
  systemResolver,
} from "../http/resolver";
export type { CancelableResolverOptions } from "../http/resolver";

/** Destroy a response body we will not read, without an unhandled 'error'. */
const discard = (body: Readable | undefined): void => {
  if (body !== undefined && !body.destroyed) {
    body.on("error", () => {
      // Expected: destroying an unread undici body emits an AbortError.
    });
    body.destroy();
  }
};

const headerValue = (
  value: string | string[] | undefined
): string | undefined => (Array.isArray(value) ? value.join(", ") : value);

const cleanHeaders = (
  raw: Record<string, string | string[] | undefined>
): Record<string, string> => {
  const out: Record<string, string> = {};
  for (const [name, value] of Object.entries(raw)) {
    const key = name.toLowerCase();
    const text = headerValue(value);
    if (text !== undefined && !DROPPED_HEADERS.has(key)) {
      out[key] = text;
    }
  }
  return out;
};

const ERROR_REASONS: Readonly<Record<string, BlockReason>> = {
  UND_ERR_ABORTED: "timeout",
  UND_ERR_BODY_TIMEOUT: "timeout",
  UND_ERR_CONNECT_TIMEOUT: "timeout",
  UND_ERR_HEADERS_TIMEOUT: "timeout",
  UND_ERR_RES_EXCEEDED_MAX_SIZE: "size",
};

const reasonFor = (error: unknown, signal: AbortSignal): BlockReason => {
  if (error instanceof GuardError) {
    return error.reason;
  }
  if (signal.aborted) {
    return "timeout";
  }
  const code =
    typeof error === "object" && error !== null && "code" in error
      ? String(error.code)
      : "";
  return ERROR_REASONS[code] ?? "network";
};

const fetchHop = async (
  dial: Dial | undefined,
  target: ValidatedTarget,
  request: FetchRequest,
  hopRequest: HopRequest,
  limits: FetchLimits,
  signal: AbortSignal,
  redirects: number
): Promise<Hop> => {
  const agent = new Agent({
    allowH2: false,
    bodyTimeout: limits.totalTimeoutMs,
    connect: pinnedConnector(target, limits.connectTimeoutMs, dial),
    connections: 1,
    headersTimeout: limits.totalTimeoutMs,
    // Second layer for the raw size; `readBody` enforces it too.
    maxResponseSize: limits.maxBodyBytes,
    pipelining: 1,
  });
  let body: Readable | undefined;
  try {
    const { body: payload, method } = hopRequest;
    const response = await agent.request({
      headers: {
        accept:
          request.accept ?? (request.expect === "image" ? IMAGE_ACCEPT : "*/*"),
        "accept-encoding": "identity",
        "user-agent": USER_AGENT,
        ...(payload === undefined ? {} : { "content-type": payload.type }),
      },
      method,
      ...(payload === undefined ? {} : { body: payload.data }),
      origin: `${target.scheme}//${target.authority}`,
      path: target.path,
      signal,
    });
    const { statusCode, headers } = response;
    ({ body } = response);
    const location = headerValue(headers.location);

    const cleaned = cleanHeaders(headers);
    if (REDIRECT_STATUSES.has(statusCode) && location !== undefined) {
      return {
        headers: cleaned,
        kind: "redirect",
        location,
        status: statusCode,
      };
    }
    if (statusCode < 200 || statusCode > 299 || request.readBody === false) {
      return {
        kind: "done",
        result: {
          body: new Uint8Array(0),
          headers: cleaned,
          ok: true,
          redirects,
          status: statusCode,
          url: target.href,
        },
      };
    }

    const contentType = headerValue(headers["content-type"]);
    checkContentType(contentType, request);
    const declaredLength = Number(headerValue(headers["content-length"]));
    if (
      Number.isFinite(declaredLength) &&
      declaredLength > limits.maxBodyBytes
    ) {
      throw new GuardError("size");
    }
    const decoder = decoderFor(headerValue(headers["content-encoding"]));
    if (decoder === "unsupported") {
      throw new GuardError("content-type");
    }
    const bytes = await readBody(
      body,
      decoder,
      limits.maxBodyBytes,
      budgetCharge(request)
    );

    let imageType: ImageType | undefined;
    if (request.expect === "image") {
      const verdict = checkImage(contentType, bytes);
      if (!verdict.ok) {
        throw new GuardError(verdict.reason);
      }
      imageType = verdict.type;
    }
    return {
      kind: "done",
      result: {
        body: bytes,
        headers: cleaned,
        ok: true,
        redirects,
        status: statusCode,
        url: target.href,
        ...(imageType === undefined ? {} : { imageType }),
      },
    };
  } finally {
    discard(body);
    await agent.destroy();
  }
};

/**
 * Internal factory. `dial` exists only for tests (see `testing.ts`) and is
 * refused outside Vitest, so even a deep import cannot use it in production.
 *
 * Under Bun this returns the Bun adapter (`../bun/fetch.ts`): Bun replaces
 * `undici` with its own stub, so the pinned connector cannot work there.
 */
export const createGuardedFetchInternal = (
  options: GuardedFetchOptions,
  dial: Dial | undefined
): GuardedFetch => {
  if (dial !== undefined && process.env.VITEST !== "true") {
    throw new Error("net-guard: dial override is only available in tests");
  }
  if (isBun()) {
    return createBunFetchInternal(
      options,
      dial === undefined ? undefined : { dial }
    );
  }
  // Node and Deno; the default resolver (c-ares, cancelable and off the
  // libuv threadpool) is the same as Bun's, decided in `guardedFetchFromHops`.
  return guardedFetchFromHops(
    options,
    async (...args) => await fetchHop(dial, ...args),
    reasonFor
  );
};

/**
 * Build a guarded fetch with a custom resolver, policy or tighter limits.
 * Picks the Bun adapter when running under Bun.
 */
export const createGuardedFetch = (
  options: GuardedFetchOptions = {}
): GuardedFetch => createGuardedFetchInternal(options, undefined);
