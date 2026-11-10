// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import type { z } from "zod";

import {
  download,
  downloadUrlFor,
  reportsUploadProgress,
  upload,
  uploadUrlFor,
} from "./blob";
import type {
  DownloadOptions,
  UploadBody,
  UploadOptions,
  UploadResult,
} from "./blob";
import { CAPABILITIES } from "./capabilities";
import type { Capability } from "./capabilities";
import { JmapError, parseOrThrow, requestErrorFromBody } from "./errors";
import type { AccountId } from "./ids";
import type { OtherString } from "./other-string";
import { stateChanges } from "./push";
import type { PushOptions, StateChange } from "./push";
import { RequestBuilder, ResponseSchema, settleCalls } from "./request";
import type { Resolved, Settled, SettledResults } from "./request";
import { pause, spread, unavailableMethodWaitMs } from "./retry";
import { fetchSession, primaryAccountId } from "./session";
import type { Session } from "./session";
import { Slots } from "./slots";
import { Transport, readLimitedText } from "./transport";
import type { FetchLike, GetAccessToken } from "./transport";
import { checkUrl, expandTemplate, makeOriginPolicy } from "./url";
import type { OriginPolicy } from "./url";
import type { XhrUploads } from "./xhr";

/**
 * How the client authenticates: a Bearer token from `getAccessToken` (the
 * usual way), or `sameOriginCookies` for a BFF proxy
 * the session URL is on the page's own origin, a backend there
 * holds the token and adds it, and the browser only sends its cookies
 * (`credentials: "same-origin"`, no `Authorization`). Server-provided URLs
 * must then stay on the session URL's origin, so cookies never go
 * elsewhere: `allowCrossOriginUrls` and `serverAliases` are refused.
 */
export type ConnectAuth =
  | {
      /** Called before each request; `forceRefresh` is true after a 401. */
      readonly getAccessToken: GetAccessToken;
      readonly sameOriginCookies?: false;
    }
  | {
      readonly getAccessToken?: undefined;
      readonly sameOriginCookies: true;
    };

export type ConnectOptions = ConnectAuth & ConnectBaseOptions;

export interface ConnectBaseOptions {
  /** Session resource or `https://host/.well-known/jmap`. Trusted input. */
  readonly sessionUrl: URL | string;
  /**
   * Sent with every request to the server, e.g. the anti-CSRF header a BFF
   * proxy requires. Not secrets: they are not treated like the token.
   */
  readonly headers?: Readonly<Record<string, string>>;
  /** Defaults to `globalThis.fetch`. Pass a guarded fetch on servers. */
  readonly fetch?: FetchLike;
  /**
   * Origins (besides the session URL's) that server-provided URLs may point
   * to, or `true` for any. Default `false`: a session document can't make us
   * send the token to another origin.
   */
  readonly allowCrossOriginUrls?: boolean | readonly string[];
  /**
   * Other origins of the session URL's own server. A server-provided URL on
   * one of them is used on the session URL's origin instead (same path):
   * Stalwart answers on every host name but writes its public name in the
   * session, and a web app it serves on another name can then keep every
   * request on its own origin (no CORS, `connect-src 'self'`). Only for
   * names that really are the same server.
   */
  readonly serverAliases?: readonly string[];
  /** Max bytes buffered for one API response (default 32 MiB). */
  readonly maxResponseBytes?: number;
  /**
   * Uploads with progress (browsers: `{ create: () => new XMLHttpRequest(),
   * pageOrigin: location.origin }`). Only used for uploads to another
   * origin than the page's (see `XhrUploads`). Otherwise, or without it,
   * `onProgress` only fires at the end.
   */
  readonly xhr?: XhrUploads;
  readonly signal?: AbortSignal;
}

export interface RequestOptions {
  readonly signal?: AbortSignal;
  /**
   * Called each time the API request is about to go out (after any session
   * refresh and client-side checks). An error thrown before it was called
   * means the server never saw the request; after it, the server may have
   * run it even if the answer was lost (a send, say).
   */
  readonly onSend?: () => void;
}

const DEFAULT_MAX_RESPONSE_BYTES = 32 * 1024 * 1024;

const isPlainObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" &&
  value !== null &&
  Object.getPrototypeOf(value) === Object.prototype;

const mapHandles = (
  value: unknown,
  builder: RequestBuilder,
  pick: (callId: string) => unknown
): unknown => {
  if (builder.isHandle(value)) {
    return pick(value.callId);
  }
  if (Array.isArray(value)) {
    return value.map((item: unknown) => mapHandles(item, builder, pick));
  }
  if (isPlainObject(value)) {
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [
        key,
        mapHandles(item, builder, pick),
      ])
    );
  }
  return value;
};

/**
 * Tries after the server refused a request as one too many, and the first
 * wait (it doubles): about 8 s in all, then the error reaches the caller.
 */
const BUSY_RETRIES = 6;
const BUSY_WAIT_MS = 125;

/**
 * One too many at once: the bridge answers 429, Stalwart 400 with the
 * `maxConcurrentRequests` limit. Either way the request didn't run.
 */
const isTooMany = (error: JmapError): boolean =>
  error.detail.kind === "request"
    ? error.detail.status === 429 ||
      error.detail.limit === "maxConcurrentRequests"
    : error.detail.kind === "http" && error.detail.status === 429;

/** For redirect discovery only: no token, no cookies, follows redirects. */
const unauthenticatedProbe =
  (fetchImpl: FetchLike) =>
  async (url: URL, signal?: AbortSignal): Promise<Response> =>
    await fetchImpl(url, {
      credentials: "omit",
      method: "GET",
      redirect: "follow",
      referrerPolicy: "no-referrer",
      ...(signal === undefined ? {} : { signal }),
    });

/**
 * A JMAP client bound to one session. Create it with `JmapClient.connect`.
 *
 * ```ts
 * const client = await JmapClient.connect({ sessionUrl, getAccessToken });
 * const accountId = client.primaryAccountId();
 * const [query, emails] = await client.request((b) => {
 *   const q = b.call("Email/query", { accountId, limit: 50 });
 *   const g = b.call("Email/get", {
 *     "#ids": q.ref("/ids"),
 *     accountId,
 *     properties: ["subject", "from", "receivedAt"],
 *   });
 *   return [q, g] as const;
 * });
 * ```
 */
export class JmapClient {
  readonly #transport: Transport;
  readonly #fetch: FetchLike;
  readonly #policy: OriginPolicy;
  readonly #start: URL;
  readonly #maxResponseBytes: number;
  readonly #xhr: XhrUploads | undefined;
  readonly #slots = new Slots();
  readonly #uploadSlots = new Slots();
  readonly #sessionListeners = new Set<(session: Session) => void>();
  #session: Session;
  #sessionStale = false;

  private constructor(
    init: {
      readonly transport: Transport;
      readonly fetch: FetchLike;
      readonly policy: OriginPolicy;
      readonly start: URL;
      readonly maxResponseBytes: number;
      readonly xhr: XhrUploads | undefined;
    },
    session: Session
  ) {
    this.#transport = init.transport;
    this.#fetch = init.fetch;
    this.#policy = init.policy;
    this.#start = init.start;
    this.#maxResponseBytes = init.maxResponseBytes;
    this.#xhr = init.xhr;
    this.#session = session;
  }

  /** Fetches and validates the session, then returns a ready client. */
  static async connect(options: ConnectOptions): Promise<JmapClient> {
    const fetchImpl: FetchLike =
      options.fetch ??
      (async (input, init) => await globalThis.fetch(input, init));
    const start = new URL(options.sessionUrl);
    if (
      options.sameOriginCookies === true &&
      (options.allowCrossOriginUrls !== undefined ||
        options.serverAliases !== undefined)
    ) {
      // Cookies must never follow a server-provided URL to another origin.
      throw new JmapError({ kind: "invalidUrl", url: start.href });
    }
    const policy = makeOriginPolicy(
      start,
      options.allowCrossOriginUrls,
      options.serverAliases
    );
    const transport = new Transport({
      auth:
        options.sameOriginCookies === true
          ? { kind: "same-origin-cookies" }
          : { getAccessToken: options.getAccessToken, kind: "bearer" },
      fetch: fetchImpl,
      ...(options.headers === undefined ? {} : { headers: options.headers }),
    });
    const init = {
      fetch: fetchImpl,
      maxResponseBytes: options.maxResponseBytes ?? DEFAULT_MAX_RESPONSE_BYTES,
      xhr: options.xhr,
      policy,
      start,
      transport,
    };
    const session = await fetchSession(
      transport,
      unauthenticatedProbe(fetchImpl),
      start,
      policy,
      options.signal
    );
    return new JmapClient(init, session);
  }

  /** The current session. Refreshed automatically when its state changes. */
  get session(): Session {
    return this.#session;
  }

  /** Re-fetches the session resource. */
  async refreshSession(options: RequestOptions = {}): Promise<Session> {
    const previous = this.#session.state;
    this.#session = await fetchSession(
      this.#transport,
      unauthenticatedProbe(this.#fetch),
      this.#start,
      this.#policy,
      options.signal
    );
    this.#sessionStale = false;
    if (this.#session.state !== previous) {
      for (const listener of this.#sessionListeners) {
        listener(this.#session);
      }
    }
    return this.#session;
  }

  /**
   * Calls `listener` whenever a refreshed session differs (a capability
   * came or went). Returns a function that stops it.
   */
  onSessionChange(listener: (session: Session) => void): () => void {
    this.#sessionListeners.add(listener);
    return () => {
      this.#sessionListeners.delete(listener);
    };
  }

  /** Primary account for a capability (default: mail). Throws if none. */
  primaryAccountId(
    capability: Capability | OtherString = CAPABILITIES.mail
  ): AccountId {
    const id = primaryAccountId(this.#session, capability);
    if (id === undefined) {
      throw new JmapError({
        kind: "usage",
        reason: `no primary account for ${capability}`,
      });
    }
    return id;
  }

  /**
   * Builds and sends one batched request. Resolves every `CallHandle` the
   * callback returns (a handle, a tuple or an object of handles) to its
   * typed response. Throws the first method error, if any.
   */
  async request<T>(
    build: (builder: RequestBuilder) => T,
    options: RequestOptions = {}
  ): Promise<Resolved<T>> {
    const builder = new RequestBuilder();
    const shape = build(builder);
    const results = await this.#execute(builder, options);
    const pick = (callId: string): unknown => {
      const result = results.get(callId);
      if (result === undefined) {
        throw new JmapError({ kind: "usage", reason: "unknown call" });
      }
      if (!result.ok) {
        throw result.error;
      }
      return result.value;
    };
    // Errors surface in call order, not in the order of the returned shape.
    for (const result of results.values()) {
      if (!result.ok) {
        throw result.error;
      }
    }
    // The single place where runtime values meet the computed types: each
    // handle's response was parsed by that method's schema (methods.ts), and
    // `Resolved<T>` maps handles to exactly those response types.
    // oxlint-disable-next-line typescript/no-unsafe-type-assertion
    return mapHandles(shape, builder, pick) as Resolved<T>;
  }

  /** Like `request`, but never throws method errors: each is `Settled`. */
  async requestSettled<T>(
    build: (builder: RequestBuilder) => T,
    options: RequestOptions = {}
  ): Promise<SettledResults<T>> {
    const builder = new RequestBuilder();
    const shape = build(builder);
    const results = await this.#execute(builder, options);
    const pick = (callId: string): Settled<unknown> =>
      results.get(callId) ?? {
        error: new JmapError({ kind: "usage", reason: "unknown call" }),
        ok: false,
      };
    // See `request` for why this assertion is sound.
    // oxlint-disable-next-line typescript/no-unsafe-type-assertion
    return mapHandles(shape, builder, pick) as SettledResults<T>;
  }

  /** The download URL for a blob (for `<a download>` you still need auth). */
  downloadUrl(
    options: Pick<DownloadOptions, "accountId" | "blobId" | "name" | "type">
  ): URL {
    return downloadUrlFor(this.#session, this.#policy, options);
  }

  /** Authenticated blob download; see `DownloadOptions.maxBytes`. */
  async download(options: DownloadOptions): Promise<Response> {
    return await download(
      this.#transport,
      this.#session,
      this.#policy,
      options
    );
  }

  /** Downloads a blob fully into memory (use `maxBytes`!). */
  async downloadBlob(options: DownloadOptions): Promise<Blob> {
    const response = await this.download(options);
    return await response.blob();
  }

  /** Upload URL for an account (e.g. to upload with XHR for progress). */
  uploadUrl(accountId: AccountId): URL {
    return uploadUrlFor(this.#session, this.#policy, accountId);
  }

  /**
   * Whether `upload(accountId, body, { onProgress })` reports progress as
   * the bytes leave. `false` (Blob bodies on the page's own origin, or no
   * `xhr` factory) means `onProgress` only fires at the end, so the caller
   * should show an indeterminate bar instead of one stuck at 0.
   */
  uploadReportsProgress(accountId: AccountId, body: UploadBody): boolean {
    return reportsUploadProgress(
      this.#xhr,
      this.uploadUrl(accountId),
      body,
      this.#transport.sameOriginCookies
    );
  }

  /** Uploads a blob; checks `maxSizeUpload` before sending. */
  async upload(
    accountId: AccountId,
    body: UploadBody,
    options: UploadOptions = {}
  ): Promise<UploadResult> {
    await this.#uploadSlots.acquire(
      this.#session.core.maxConcurrentUpload,
      options.signal
    );
    try {
      return await upload(
        this.#transport,
        this.#session,
        this.#policy,
        accountId,
        body,
        options,
        this.#xhr
      );
    } finally {
      this.#uploadSlots.release();
    }
  }

  /** Push notifications: `for await (const change of client.push())`. */
  push(
    options: PushOptions = {}
  ): AsyncGenerator<StateChange, void, undefined> {
    const session = this.#session;
    const policy = this.#policy;
    return stateChanges(
      {
        transport: this.#transport,
        url: (values) =>
          checkUrl(
            expandTemplate(session.eventSourceUrl, values),
            session.sessionUrl,
            policy
          ),
      },
      options
    );
  }

  /**
   * One API request, in turn (`maxConcurrentRequests`), and its response's
   * text. Other tabs and clients of the account count too, so the server
   * may still refuse it as one too many, which means it did nothing: that
   * is tried again a few times, even if it writes.
   */
  async #post(body: string, options: RequestOptions): Promise<string> {
    const { signal } = options;
    for (let attempt = 0; ; attempt += 1) {
      // oxlint-disable-next-line no-await-in-loop -- one at a time.
      const answer = await this.#postOnce(body, options);
      if (typeof answer === "string") {
        return answer;
      }
      if (!isTooMany(answer) || attempt >= BUSY_RETRIES) {
        throw answer;
      }
      // Outside the queue: a request waiting here holds no slot.
      // oxlint-disable-next-line no-await-in-loop -- waits between tries.
      await pause(BUSY_WAIT_MS * 2 ** attempt, signal);
    }
  }

  /** The response's text, or the error the server answered. */
  async #postOnce(
    body: string,
    { onSend, signal }: RequestOptions
  ): Promise<string | JmapError> {
    await this.#slots.acquire(this.#session.core.maxConcurrentRequests, signal);
    try {
      onSend?.();
      const response = await this.#transport.send(this.#session.apiUrl, {
        body,
        headers: {
          Accept: "application/json",
          "Content-Type": "application/json",
        },
        method: "POST",
        signal,
      });
      const text = await readLimitedText(response, this.#maxResponseBytes);
      return response.ok ? text : requestErrorFromBody(response.status, text);
    } finally {
      this.#slots.release();
    }
  }

  /**
   * Sends one chunk and parses the answer. A request of nothing but reads
   * whose method ran out of server memory (`serverUnavailable`, Stalwart
   * 1.0) is sent again after a wait, outside the queue (retry.ts).
   */
  async #send(
    body: string,
    options: RequestOptions
  ): Promise<z.infer<typeof ResponseSchema>> {
    const { signal } = options;
    for (let retries = 0; ; retries += 1) {
      // oxlint-disable-next-line no-await-in-loop -- one try at a time.
      const text = await this.#post(body, options);
      let json: unknown = undefined;
      try {
        json = JSON.parse(text);
      } catch (error) {
        throw new JmapError(
          { issues: ["response is not JSON"], kind: "invalidResponse" },
          { cause: error }
        );
      }
      const parsed = parseOrThrow(ResponseSchema, json);
      const wait = unavailableMethodWaitMs(
        body,
        parsed.methodResponses,
        retries
      );
      if (wait === null) {
        return parsed;
      }
      // oxlint-disable-next-line no-await-in-loop -- waits between tries.
      await pause(spread(wait), signal);
    }
  }

  async #execute(
    builder: RequestBuilder,
    options: RequestOptions
  ): Promise<Map<string, Settled<unknown>>> {
    const { signal } = options;
    if (this.#sessionStale) {
      await this.refreshSession(signal === undefined ? {} : { signal });
    }
    const { core } = this.#session;
    const results = new Map<string, Settled<unknown>>();
    for (const chunk of builder.chunks(core.maxCallsInRequest)) {
      const body = JSON.stringify(chunk.request);
      const size = new TextEncoder().encode(body).byteLength;
      if (core.maxSizeRequest > 0 && size > core.maxSizeRequest) {
        throw new JmapError({
          actual: size,
          kind: "limit",
          limit: "maxSizeRequest",
          max: core.maxSizeRequest,
        });
      }
      // Chunks run in order: later calls may depend on earlier writes.
      // oxlint-disable-next-line no-await-in-loop
      const parsed = await this.#send(body, options);
      if (parsed.sessionState !== this.#session.state) {
        this.#sessionStale = true;
      }
      for (const [callId, settled] of settleCalls(
        chunk.calls,
        parsed.methodResponses
      )) {
        results.set(callId, settled);
      }
    }
    return results;
  }
}
