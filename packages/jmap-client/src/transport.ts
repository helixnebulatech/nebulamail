// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Authenticated fetch. Every request to the JMAP server goes through here.

import { JmapError, isJmapError } from "./errors";
import {
  MAX_UNAVAILABLE_RETRIES,
  isSafeToRepeat,
  pause,
  spread,
  unavailableWaitMs,
} from "./retry";
import { sendXhr } from "./xhr";
import type { XhrFactory, XhrRequest, XhrResponse } from "./xhr";

export type FetchLike = (
  input: URL | string,
  init: RequestInit
) => Promise<Response>;

export interface TokenRequest {
  /** True after a 401: the cached token is no longer accepted. */
  readonly forceRefresh: boolean;
  readonly signal?: AbortSignal | undefined;
}

/** Returns a (short-lived) OAuth access token. Never logged by the client. */
export type GetAccessToken = (request: TokenRequest) => Promise<string>;

/**
 * How requests are authenticated: a Bearer token (the default), or the
 * page's own cookies for a session URL on the page's origin served by a
 * backend that holds the token itself (a BFF proxy, see `ConnectAuth`).
 */
export type TransportAuth =
  | { readonly kind: "bearer"; readonly getAccessToken: GetAccessToken }
  | { readonly kind: "same-origin-cookies" };

export interface AuthorizedFetchOptions {
  readonly fetch: FetchLike;
  readonly auth: TransportAuth;
  /** Sent with every request (e.g. a header a BFF proxy requires). */
  readonly headers?: Readonly<Record<string, string>>;
}

export interface SendOptions {
  readonly method?: "GET" | "POST";
  readonly headers?: Readonly<Record<string, string>>;
  readonly body?: NonNullable<RequestInit["body"]>;
  /** Bodies that can't be replayed (streams) are not retried on 401. */
  readonly replayable?: boolean;
  readonly signal?: AbortSignal | undefined;
  /**
   * `error` (default) for API/upload/download calls; `manual` only for the
   * session discovery, which handles the redirect itself (see session.ts).
   */
  readonly redirect?: "error" | "manual";
  /**
   * `no-store` for the event stream. A fetch option, not a `Cache-Control`
   * request header: that header isn't CORS-safelisted, forces a preflight,
   * and Stalwart's CORS doesn't allow it.
   */
  readonly cache?: NonNullable<RequestInit["cache"]>;
}

export class Transport {
  readonly #fetch: FetchLike;
  readonly #getAccessToken: GetAccessToken | undefined;
  readonly #headers: Readonly<Record<string, string>>;
  #refreshing: Promise<string> | undefined = undefined;

  constructor(options: AuthorizedFetchOptions) {
    this.#fetch = options.fetch;
    this.#getAccessToken =
      options.auth.kind === "bearer" ? options.auth.getAccessToken : undefined;
    this.#headers = options.headers ?? {};
  }

  /**
   * True when the page's cookies authenticate (no token): uploads may then
   * use XHR on the page's own origin and never on another (see xhr.ts).
   */
  get sameOriginCookies(): boolean {
    return this.#getAccessToken === undefined;
  }

  /**
   * A POST with upload progress (XMLHttpRequest, see xhr.ts), with the
   * same token handling as `send`.
   */
  async sendWithProgress(
    factory: XhrFactory,
    request: Omit<XhrRequest, "token">
  ): Promise<XhrResponse> {
    const attempt = async (forceRefresh: boolean): Promise<XhrResponse> =>
      await sendXhr(factory, {
        ...request,
        headers: { ...this.#headers, ...request.headers },
        token: await this.#token(forceRefresh, request.signal),
      });
    const first = await attempt(false);
    if (first.status !== 401) {
      return first;
    }
    const second = await attempt(true);
    if (second.status === 401) {
      throw new JmapError({ kind: "unauthorized" });
    }
    return second;
  }

  /**
   * Sends a request (`#sendOnce`). A 503 with a short `Retry-After` is
   * repeated if that is safe (retry.ts); `options.signal` ends the wait.
   */
  async send(url: URL, options: SendOptions = {}): Promise<Response> {
    const repeatable =
      options.replayable !== false &&
      isSafeToRepeat(options.method, options.body);
    for (let retries = 0; ; retries += 1) {
      // One try at a time: each depends on the one before.
      // oxlint-disable-next-line no-await-in-loop
      const response = await this.#sendOnce(url, options);
      const wait =
        repeatable && retries < MAX_UNAVAILABLE_RETRIES
          ? unavailableWaitMs(response)
          : null;
      if (wait === null) {
        return response;
      }
      // oxlint-disable-next-line no-await-in-loop
      await response.body?.cancel();
      // oxlint-disable-next-line no-await-in-loop
      await pause(spread(wait), options.signal);
    }
  }

  /**
   * Sends one request with `Authorization: Bearer`. On 401 asks for a fresh
   * token once and retries once; a second 401 becomes `unauthorized`. With
   * same-origin cookies there is no token: the retry gives the backend,
   * which refreshes its own token on a 401, one more chance.
   */
  async #sendOnce(url: URL, options: SendOptions): Promise<Response> {
    const first = await this.#attempt(url, options, false);
    if (first.status !== 401) {
      return first;
    }
    await first.body?.cancel();
    if (options.replayable === false) {
      throw new JmapError({ kind: "unauthorized" });
    }
    const second = await this.#attempt(url, options, true);
    if (second.status === 401) {
      await second.body?.cancel();
      throw new JmapError({ kind: "unauthorized" });
    }
    return second;
  }

  async #attempt(
    url: URL,
    options: SendOptions,
    forceRefresh: boolean
  ): Promise<Response> {
    const token = await this.#token(forceRefresh, options.signal);
    const isStream = options.body instanceof ReadableStream;
    const init: RequestInit = {
      // Bearer tokens are our only credential: never attach cookies or
      // HTTP auth the browser may hold for the mail server's origin (no
      // ambient authority, nothing for CSRF to ride on). Without a token
      // the page's cookies are the credential, and only for its own origin
      // (the backend then checks the request's origin itself).
      credentials: token === undefined ? "same-origin" : "omit",
      headers: {
        ...this.#headers,
        ...options.headers,
        ...(token === undefined ? {} : { Authorization: `Bearer ${token}` }),
      },
      method: options.method ?? "GET",
      // A redirect would re-send the Authorization header to wherever the
      // server (or an attacker controlling a redirect) points, bypassing the
      // origin guard. Same-origin redirects are not needed by JMAP, so fail.
      redirect: options.redirect ?? "error",
      // Don't leak the app's URL (which can contain ids) to the server.
      referrerPolicy: "no-referrer",
      ...(options.body === undefined ? {} : { body: options.body }),
      ...(options.cache === undefined ? {} : { cache: options.cache }),
      // Required by the Fetch spec for streaming request bodies.
      ...(isStream ? { duplex: "half" } : {}),
      ...(options.signal === undefined ? {} : { signal: options.signal }),
    };
    try {
      return await this.#fetch(url, init);
    } catch (error) {
      // Our own body stream errored (e.g. upload above maxSizeUpload): fetch
      // rejects with it directly or wraps it as `cause` (undici).
      if (isJmapError(error)) {
        throw error;
      }
      if (error instanceof Error && isJmapError(error.cause)) {
        throw error.cause;
      }
      if (options.signal?.aborted === true) {
        throw new JmapError({ kind: "aborted" }, { cause: error });
      }
      throw new JmapError({ kind: "network" }, { cause: error });
    }
  }

  /** The token for one attempt; `undefined` with same-origin cookies. */
  async #token(
    forceRefresh: boolean,
    signal: AbortSignal | undefined
  ): Promise<string | undefined> {
    const getAccessToken = this.#getAccessToken;
    if (getAccessToken === undefined) {
      return undefined;
    }
    try {
      if (!forceRefresh) {
        return await getAccessToken({ forceRefresh, signal });
      }
      // Several requests may hit 401 at once; refresh only once for all.
      // The shared refresh is not tied to one caller's AbortSignal.
      this.#refreshing ??= getAccessToken({ forceRefresh });
      try {
        return await this.#refreshing;
      } finally {
        this.#refreshing = undefined;
      }
    } catch (error) {
      if (signal?.aborted === true) {
        throw new JmapError({ kind: "aborted" }, { cause: error });
      }
      throw new JmapError({ kind: "auth" }, { cause: error });
    }
  }
}

/** Passes bytes through, erroring the stream once `maxBytes` is exceeded. */
export const limitStream = (
  body: ReadableStream<Uint8Array> | null,
  maxBytes: number
): ReadableStream<Uint8Array> | null => {
  if (body === null) {
    return null;
  }
  let seen = 0;
  return body.pipeThrough(
    new TransformStream<Uint8Array, Uint8Array>({
      transform: (chunk, controller) => {
        seen += chunk.byteLength;
        if (seen > maxBytes) {
          controller.error(
            new JmapError({
              actual: seen,
              kind: "limit",
              limit: "maxBytes",
              max: maxBytes,
            })
          );
          return;
        }
        controller.enqueue(chunk);
      },
    })
  );
};

/** Reads a body as text, refusing to buffer more than `maxBytes`. */
export const readLimitedText = async (
  response: Response,
  maxBytes: number
): Promise<string> => {
  const declared = Number(response.headers.get("content-length") ?? "0");
  if (declared > maxBytes) {
    await response.body?.cancel();
    throw new JmapError({
      actual: declared,
      kind: "limit",
      limit: "maxBytes",
      max: maxBytes,
    });
  }
  const limited = limitStream(response.body, maxBytes);
  return await new Response(limited).text();
};
