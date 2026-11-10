// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// A 503 with `Retry-After` is the server asking to come back in a moment.
// Stalwart 1.0 answers it when its memory budget runs out, with
// `Retry-After: 5` (`MEMORY_RETRY_AFTER`). There it happens on downloads and
// before a request runs. Inside a method the budget gives a `serverUnavailable`
// method error instead, in a 200 and with no `Retry-After`: the client repeats
// a request of nothing but reads that got one (`unavailableMethodWaitMs`).
//
// Only what is safe to repeat is repeated: a GET, or a JMAP request of
// nothing but reads. A write isn't: on Stalwart a 503 means it didn't run, but
// a proxy in front or another server doesn't promise that, and creating or
// sending twice is worse than one error. The caller gets the 503, as before.

import { z } from "zod";

import { JmapError } from "./errors";
import { parseHttpDate } from "./http-date";

/** Tries after the first. */
export const MAX_UNAVAILABLE_RETRIES = 2;

/** Longer waits aren't worth it: the 503 goes to the caller. */
export const MAX_RETRY_AFTER_MS = 10_000;

/**
 * The wait when a browser hides the header: Stalwart allows other origins
 * but doesn't list `Retry-After` in `Access-Control-Expose-Headers`, and the
 * web talks to it from another origin. Its own value (5 s). Only what is
 * safe to repeat gets here, so a 503 that didn't ask for it costs a wait.
 */
export const HIDDEN_RETRY_AFTER_MS = 5000;

/** RFC 8620 methods that change nothing, by the end of their name. */
const READ_ONLY_METHOD = /\/(?:changes|get|parse|query|queryChanges)$/u;

const RequestSchema = z.looseObject({
  methodCalls: z.array(z.tuple([z.string()]).rest(z.unknown())).min(1),
});

/** Whether every method of this JMAP request body only reads. */
export const isReadOnlyRequest = (body: unknown): boolean => {
  if (typeof body !== "string") {
    return false;
  }
  try {
    const request: unknown = JSON.parse(body);
    const parsed = RequestSchema.safeParse(request);
    return (
      parsed.success &&
      parsed.data.methodCalls.every(([name]) => READ_ONLY_METHOD.test(name))
    );
  } catch {
    return false;
  }
};

/** Whether this request can be sent again without changing anything twice. */
export const isSafeToRepeat = (
  method: "GET" | "POST" | undefined,
  body: unknown
): boolean => (method ?? "GET") === "GET" || isReadOnlyRequest(body);

/**
 * How long a 503 asks us to wait, or null if it isn't worth waiting: another
 * status, no usable `Retry-After` (seconds or a date), or too long. Hidden
 * from us, the usual wait.
 */
export const unavailableWaitMs = (
  response: Response,
  now: number = Date.now()
): number | null => {
  if (response.status !== 503) {
    return null;
  }
  const value = response.headers.get("retry-after")?.trim() ?? "";
  if (value === "" && response.type === "cors") {
    return HIDDEN_RETRY_AFTER_MS;
  }
  // RFC 9110 §10.2.3: Retry-After = HTTP-date / delay-seconds, and
  // §5.6.7: recipients MUST accept all three HTTP-date formats.
  let ms = Number.NaN;
  if (/^\d+$/u.test(value)) {
    ms = Number(value) * 1000;
  } else {
    const date = parseHttpDate(value, now);
    ms = date === null ? Number.NaN : date - now;
  }
  if (Number.isNaN(ms) || ms > MAX_RETRY_AFTER_MS) {
    return null;
  }
  return Math.max(ms, 0);
};

/**
 * The wait before repeating a request whose method answered
 * `serverUnavailable`: Stalwart gives none, so its own `MEMORY_RETRY_AFTER`.
 */
export const SERVER_UNAVAILABLE_WAIT_MS = 5000;

/**
 * How long to wait before sending a request again because a method ran out
 * of server memory, or null. Only reads are sent again, and only
 * `MAX_UNAVAILABLE_RETRIES` times: a write may have half run.
 */
export const unavailableMethodWaitMs = (
  body: string,
  responses: readonly (readonly [string, unknown, string])[],
  retries: number
): number | null => {
  if (retries >= MAX_UNAVAILABLE_RETRIES || !isReadOnlyRequest(body)) {
    return null;
  }
  const unavailable = responses.some(
    ([name, raw]) =>
      name === "error" &&
      typeof raw === "object" &&
      raw !== null &&
      "type" in raw &&
      raw.type === "serverUnavailable"
  );
  return unavailable ? SERVER_UNAVAILABLE_WAIT_MS : null;
};

/**
 * A wait plus up to half of it again, at random: requests turned away
 * together would otherwise all come back at the same moment and be turned
 * away again (seen with Stalwart 1.0.1: of 4 downloads at once, 3 get the
 * 503 and meet again 5 s later). Never shorter than asked.
 */
export const spread = (
  ms: number,
  random: () => number = Math.random
): number => ms + Math.floor(random() * (ms / 2));

/** Waits, or fails with `aborted` as soon as `signal` fires. */
export const pause = async (
  ms: number,
  signal: AbortSignal | undefined
): Promise<void> => {
  if (signal?.aborted === true) {
    throw new JmapError({ kind: "aborted" });
  }
  const { promise, reject, resolve } = Promise.withResolvers<undefined>();
  const timer = setTimeout(() => {
    resolve(undefined);
  }, ms);
  const onAbort = (): void => {
    clearTimeout(timer);
    reject(new JmapError({ kind: "aborted" }));
  };
  signal?.addEventListener("abort", onAbort, { once: true });
  try {
    await promise;
  } finally {
    signal?.removeEventListener("abort", onAbort);
  }
};
