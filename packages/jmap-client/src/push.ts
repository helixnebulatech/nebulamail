// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Push over EventSource (RFC 8620 §7.3), read with `fetch` so we can send
// `Authorization: Bearer` (the EventSource API can't). Yields StateChange
// objects; reconnects with exponential backoff; stops when the signal fires.

import { z } from "zod";

import { JmapError, isJmapError, requestErrorFromBody } from "./errors";
import { AccountId, State } from "./ids";
import { CalendarAlertSchema } from "./push-subscription";
import type { CalendarAlert } from "./push-subscription";
import { feedSse, initialSseState } from "./sse";
import type { SseEvent, SseState } from "./sse";
import { readLimitedText } from "./transport";
import type { Transport } from "./transport";

export const StateChangeSchema = z.looseObject({
  "@type": z.literal("StateChange"),
  changed: z.partialRecord(AccountId, z.record(z.string(), State)),
  pushState: z.string().optional(),
});
/** `changed[accountId][typeName]` is the new state string for that type. */
export type StateChange = z.infer<typeof StateChangeSchema>;

export interface PushOptions {
  /** Type names to subscribe to (e.g. `["Email", "Mailbox"]`), or "*". */
  readonly types?: "*" | readonly string[];
  /**
   * Seconds between server pings (0 = none). With pings on, a connection
   * that stays silent for 2×ping+10 s is considered dead and reopened.
   */
  readonly ping?: number;
  readonly signal?: AbortSignal;
  /** First reconnect delay (doubles each failure, with jitter). */
  readonly minBackoffMs?: number;
  readonly maxBackoffMs?: number;
  /**
   * Calendar alerts (draft-ietf-jmap-calendars), sent as `calendarAlert`
   * events when `types` include "CalendarAlert". They are not state
   * changes, so they come here instead of the iterator.
   */
  readonly onCalendarAlert?: (alert: CalendarAlert) => void;
  /** Called on every (re)connection, e.g. to resync with `/changes`. */
  readonly onConnect?: () => void;
  /**
   * Called when an open connection ends (dropped, silent too long, or
   * stopped), before waiting to reconnect: changes aren't pushed until the
   * next `onConnect`.
   */
  readonly onDisconnect?: () => void;
  /**
   * Called once per connection, when its first bytes arrive (an event or a
   * ping): only then is it known to deliver. A host or proxy that holds the
   * response back (Cloudflare quick tunnels do) opens the connection
   * (`onConnect`) but never gets here before it ends.
   */
  readonly onLive?: () => void;
}

export interface PushContext {
  readonly transport: Transport;
  /** Expanded, origin-checked event source URL for these options. */
  readonly url: (values: Readonly<Record<string, string>>) => URL;
}

const MAX_ERROR_BODY_BYTES = 64 * 1024;

const isFatal = (error: unknown): boolean => {
  if (!isJmapError(error)) {
    return true;
  }
  switch (error.detail.kind) {
    case "network":
    case "aborted": {
      return false;
    }
    case "http":
    case "request": {
      const { status } = error.detail;
      return status < 500 && status !== 408 && status !== 429;
    }
    case "auth":
    case "crossOrigin":
    case "invalidResponse":
    case "invalidUrl":
    case "limit":
    case "method":
    case "unauthorized":
    case "usage": {
      return true;
    }
    default: {
      return true;
    }
  }
};

/** Sleeps `ms`, resolving early (without throwing) if `signal` aborts. */
const sleep = async (ms: number, signal: AbortSignal | undefined) => {
  const { promise, resolve } = Promise.withResolvers<boolean>();
  const timer = setTimeout(() => {
    resolve(true);
  }, ms);
  const onAbort = () => {
    clearTimeout(timer);
    resolve(false);
  };
  signal?.addEventListener("abort", onAbort, { once: true });
  await promise;
  signal?.removeEventListener("abort", onAbort);
};

// A function call, so TypeScript doesn't narrow `aborted` across awaits.
const isAborted = (signal: AbortSignal | undefined): boolean =>
  signal?.aborted === true;

const backoffMs = (attempt: number, min: number, max: number): number => {
  const base = Math.min(max, min * 2 ** attempt);
  // Full jitter between base/2 and base: avoids thundering-herd reconnects.
  return base / 2 + (Math.random() * base) / 2;
};

/** A `calendarAlert` event (Stalwart's name for it), if valid. */
const toCalendarAlert = (event: SseEvent): CalendarAlert | undefined => {
  if (event.type !== "calendarAlert") {
    return undefined;
  }
  let json: unknown = undefined;
  try {
    json = JSON.parse(event.data);
  } catch {
    return undefined;
  }
  const parsed = CalendarAlertSchema.safeParse(json);
  return parsed.success ? parsed.data : undefined;
};

const toStateChange = (event: SseEvent): StateChange | undefined => {
  if (event.type !== "state" && event.type !== "message") {
    return undefined;
  }
  let json: unknown = undefined;
  try {
    json = JSON.parse(event.data);
  } catch {
    return undefined;
  }
  const parsed = StateChangeSchema.safeParse(json);
  return parsed.success ? parsed.data : undefined;
};

/**
 * Reads one connection.
 * @yields {SseEvent} Each complete SSE event.
 * @returns The parser state at end of stream.
 */
const readConnection = async function* readConnection(
  body: ReadableStream<Uint8Array>,
  initial: SseState,
  onActivity: () => void
): AsyncGenerator<SseEvent, SseState, undefined> {
  const reader = body.getReader();
  // `stream: true` keeps multi-byte UTF-8 sequences split across chunks.
  const decoder = new TextDecoder();
  let state = initial;
  try {
    let done = false;
    while (!done) {
      // Stream reads are inherently sequential.
      // oxlint-disable-next-line no-await-in-loop
      const chunk = await reader.read();
      ({ done } = chunk);
      if (chunk.value !== undefined) {
        onActivity();
        const result = feedSse(
          state,
          decoder.decode(chunk.value, { stream: true })
        );
        ({ state } = result);
        yield* result.events;
      }
    }
  } finally {
    reader.releaseLock();
  }
  return state;
};

/** Resets a timer on activity; fires `onSilence` after `ms` of silence. */
const createWatchdog = (ms: number, onSilence: () => void) => {
  let timer: ReturnType<typeof setTimeout> | undefined;
  return {
    kick: () => {
      clearTimeout(timer);
      if (ms > 0) {
        timer = setTimeout(onSilence, ms);
      }
    },
    stop: () => {
      clearTimeout(timer);
    },
  };
};

interface ConnectionParams {
  readonly context: PushContext;
  readonly url: URL;
  readonly ping: number;
  readonly signal: AbortSignal | undefined;
  readonly onOpen: () => void;
  readonly onClose: () => void;
  readonly onLive: () => void;
  readonly onCalendarAlert: ((alert: CalendarAlert) => void) | undefined;
}

/**
 * Opens one EventSource connection and reads it to the end.
 * @yields {StateChange} Each StateChange received on this connection.
 * @returns The SSE state to resume from (last event id, retry).
 */
const connectOnce = async function* connectOnce(
  params: ConnectionParams,
  previous: SseState
): AsyncGenerator<StateChange, SseState, undefined> {
  const connection = new AbortController();
  const signal =
    params.signal === undefined
      ? connection.signal
      : AbortSignal.any([params.signal, connection.signal]);
  // A connection that stays silent well past the ping interval is dead
  // (e.g. a NAT dropped it); abort it so the caller reconnects.
  const watchdog = createWatchdog(
    params.ping > 0 ? (params.ping * 2 + 10) * 1000 : 0,
    () => {
      connection.abort();
    }
  );
  let opened = false;
  try {
    const { lastEventId } = previous;
    const response = await params.context.transport.send(params.url, {
      cache: "no-store",
      headers: {
        Accept: "text/event-stream",
        ...(lastEventId === "" ? {} : { "Last-Event-ID": lastEventId }),
      },
      signal,
    });
    if (!response.ok || response.body === null) {
      const text = await readLimitedText(response, MAX_ERROR_BODY_BYTES);
      throw requestErrorFromBody(response.status, text);
    }
    watchdog.kick();
    opened = true;
    params.onOpen();
    // Partial lines/events don't survive a reconnect; id and retry do.
    const fresh: SseState = {
      ...initialSseState,
      lastEventId,
      retryMs: previous.retryMs,
    };
    let live = false;
    const events = readConnection(response.body, fresh, () => {
      watchdog.kick();
      if (!live) {
        live = true;
        params.onLive();
      }
    });
    let next = await events.next();
    while (next.done !== true) {
      const alert = toCalendarAlert(next.value);
      if (alert !== undefined) {
        params.onCalendarAlert?.(alert);
      }
      const change = toStateChange(next.value);
      if (change !== undefined) {
        yield change;
      }
      // Reading a stream is sequential by nature.
      // oxlint-disable-next-line no-await-in-loop
      next = await events.next();
    }
    return next.value;
  } catch (error) {
    // Body read failures (connection dropped, watchdog abort) surface as
    // plain DOMException/TypeError: classify them as retryable network errors.
    if (isJmapError(error)) {
      throw error;
    }
    throw new JmapError({ kind: "network" }, { cause: error });
  } finally {
    watchdog.stop();
    connection.abort();
    if (opened) {
      params.onClose();
    }
  }
};

/**
 * Async iterator of StateChange objects. Ends when `options.signal` aborts;
 * throws `JmapError` on non-retryable failures (401 after refresh, 4xx).
 * @yields {StateChange} Each StateChange, across reconnections.
 */
export const stateChanges = async function* stateChanges(
  context: PushContext,
  options: PushOptions = {}
): AsyncGenerator<StateChange, void, undefined> {
  const { signal } = options;
  const ping = options.ping ?? 30;
  const types = options.types ?? "*";
  const min = options.minBackoffMs ?? 1000;
  const max = options.maxBackoffMs ?? 60_000;
  const backoff = { attempt: 0 };
  const params: ConnectionParams = {
    context,
    onOpen: () => {
      backoff.attempt = 0;
      options.onConnect?.();
    },
    onClose: () => {
      options.onDisconnect?.();
    },
    onLive: () => {
      options.onLive?.();
    },
    onCalendarAlert: options.onCalendarAlert,
    ping,
    signal,
    url: context.url({
      closeafter: "no",
      ping: String(ping),
      types: types === "*" ? "*" : types.join(","),
    }),
  };
  let sse: SseState = initialSseState;
  while (!isAborted(signal)) {
    try {
      sse = yield* connectOnce(params, sse);
    } catch (error) {
      if (isAborted(signal)) {
        return;
      }
      if (isFatal(error)) {
        throw error;
      }
    }
    const delay = sse.retryMs ?? backoffMs(backoff.attempt, min, max);
    backoff.attempt += 1;
    // Reconnects are sequential by nature.
    // oxlint-disable-next-line no-await-in-loop
    await sleep(Math.min(Math.max(delay, min), max), signal);
  }
};
