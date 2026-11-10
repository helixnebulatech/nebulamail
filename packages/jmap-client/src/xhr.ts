// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Uploads with progress. `fetch` can't report upload progress for a Blob
// (streamed request bodies need HTTP/2 and only work in Chromium), so
// browsers upload with XMLHttpRequest. The structural type below is what we
// use of it, so the package needs no DOM types and tests can fake it.

import { JmapError } from "./errors";

export interface XhrProgress {
  readonly loaded: number;
  readonly total: number;
  readonly lengthComputable: boolean;
}

/** The part of XMLHttpRequest the client uses. */
export interface XhrLike {
  readonly open: (method: "POST", url: string) => void;
  readonly setRequestHeader: (name: string, value: string) => void;
  readonly send: (body: Blob | ArrayBuffer | Uint8Array<ArrayBuffer>) => void;
  readonly abort: () => void;
  withCredentials: boolean;
  readonly status: number;
  readonly responseText: string;
  /** The final URL, after redirects (empty before the response). */
  readonly responseURL: string;
  readonly upload: {
    readonly addEventListener: (
      type: "progress",
      listener: (event: XhrProgress) => void
    ) => void;
  };
  readonly addEventListener: (
    type: "load" | "error" | "abort" | "timeout",
    listener: () => void
  ) => void;
}

/** Makes a new XMLHttpRequest: `() => new XMLHttpRequest()` in browsers. */
export type XhrFactory = () => XhrLike;

/**
 * Uploads with progress. `pageOrigin` is the origin of the page running the
 * client (browsers: `location.origin`): XHR is only used for uploads to
 * ANOTHER origin, because `withCredentials = false` only leaves cookies out
 * across origins. On the page's own origin XHR would send that origin's
 * cookies (the static build talks to Stalwart on its own origin, which also
 * serves `/admin` and `/account`), so those uploads go through fetch with
 * `credentials: "omit"` and `onProgress` only fires at the end.
 */
export interface XhrUploads {
  readonly create: XhrFactory;
  readonly pageOrigin: string;
}

const originOf = (value: string): string | undefined => {
  try {
    return new URL(value).origin;
  } catch {
    return undefined;
  }
};

/**
 * The XHR factory to use for an upload to `url` (the final URL, after
 * `serverAliases`), or `undefined` for fetch. A `pageOrigin` that is not a
 * URL counts as the same origin: fetch, never cookies. With
 * `sameOriginCookies` (a BFF proxy on the page's origin, no token) it is
 * the other way round: the page's cookies are the credential, so XHR only
 * on the page's own origin.
 */
export const xhrFor = (
  uploads: XhrUploads | undefined,
  url: URL,
  sameOriginCookies = false
): XhrFactory | undefined => {
  if (uploads === undefined) {
    return undefined;
  }
  const page = originOf(uploads.pageOrigin);
  if (sameOriginCookies) {
    return page === url.origin ? uploads.create : undefined;
  }
  return page === undefined || page === url.origin ? undefined : uploads.create;
};

export interface XhrRequest {
  readonly url: URL;
  /** Without one (same-origin cookies), no `Authorization` header. */
  readonly token: string | undefined;
  readonly headers: Readonly<Record<string, string>>;
  readonly body: Blob | ArrayBuffer | Uint8Array<ArrayBuffer>;
  readonly onProgress?:
    | ((sent: number, total: number | undefined) => void)
    | undefined;
  readonly signal?: AbortSignal | undefined;
}

export interface XhrResponse {
  readonly status: number;
  readonly text: string;
}

/**
 * One POST with XHR, only to another origin than the page's (`xhrFor`), or
 * only to the page's own with same-origin cookies. Same rules as the fetch
 * transport (no cookies across origins, no redirects).
 */
export const sendXhr = async (
  factory: XhrFactory,
  request: XhrRequest
): Promise<XhrResponse> => {
  const { promise, reject, resolve } = Promise.withResolvers<XhrResponse>();
  if (request.signal?.aborted === true) {
    throw new JmapError({ kind: "aborted" });
  }
  const xhr = factory();
  xhr.open("POST", request.url.href);
  // Bearer tokens are our only credential: never cookies (see transport.ts).
  // `false` is XHR's `same-origin` mode: it leaves cookies out only across
  // origins, which is why same-origin uploads never come here (`xhrFor`).
  xhr.withCredentials = false;
  for (const [name, value] of Object.entries(request.headers)) {
    xhr.setRequestHeader(name, value);
  }
  if (request.token !== undefined) {
    xhr.setRequestHeader("Authorization", `Bearer ${request.token}`);
  }
  const { onProgress } = request;
  if (onProgress !== undefined) {
    xhr.upload.addEventListener("progress", (event) => {
      onProgress(
        event.loaded,
        event.lengthComputable ? event.total : undefined
      );
    });
  }
  const onAbort = (): void => {
    xhr.abort();
  };
  request.signal?.addEventListener("abort", onAbort, { once: true });
  const done = (): void => {
    request.signal?.removeEventListener("abort", onAbort);
  };
  xhr.addEventListener("load", () => {
    done();
    // XHR follows redirects and fetch's `redirect: "error"` has no XHR
    // equivalent: refuse an answer that came from somewhere else.
    if (xhr.responseURL !== "" && xhr.responseURL !== request.url.href) {
      reject(
        new JmapError({
          actual: 1,
          kind: "limit",
          limit: "maxRedirects",
          max: 0,
        })
      );
      return;
    }
    resolve({ status: xhr.status, text: xhr.responseText });
  });
  xhr.addEventListener("error", () => {
    done();
    reject(new JmapError({ kind: "network" }));
  });
  xhr.addEventListener("timeout", () => {
    done();
    reject(new JmapError({ kind: "network" }));
  });
  xhr.addEventListener("abort", () => {
    done();
    reject(new JmapError({ kind: "aborted" }));
  });
  xhr.send(request.body);
  return await promise;
};
