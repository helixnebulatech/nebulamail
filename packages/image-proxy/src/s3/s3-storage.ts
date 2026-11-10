// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// S3-compatible `ImageStorage`: AWS S3, MinIO, Garage,
// Ceph RGW, R2's S3 API… With `fetch` and our own SigV4 (`sigv4.ts`), no SDK.
//
// - Object keys are `img/v1/<sha256>/<sha256>` (the core's key), so a
//   bucket listing shows hashes only, never URLs or account ids.
// - Retention: an object counts as expired `maxAgeMs` after its
//   `Last-Modified` (the server's own date, like R2 and Blob). Expired
//   objects are deleted on read and by `sweep()`; add a bucket lifecycle
//   rule on `img/` as a second net (README).
// - The endpoint is trusted configuration written by the admin (often a
//   private address, e.g. `http://minio:9000` in Docker), so these requests
//   do NOT go through net-guard: they never carry a URL from an email.
//   What we do enforce: only the configured endpoint, redirects refused
//   (`redirect: "manual"` and any 3xx is an error), 10 s per request, size
//   limits on every body we read, and no credential or key in errors.
// - Runtime-agnostic (WebCrypto + fetch): Node, Workers, Vercel.

import type { StoredImage, SweepableStorage } from "../config";
import { DEFAULT_LIMITS } from "../config";
import type { PreloadStateStore } from "../preload/state";
import { checkStateKey, MAX_STATE_BYTES, stateKeyOf } from "../preload/state";
import { STORAGE_KEY_PATTERN } from "../storage-key";
import { storedType } from "../stored-type";
import { EMPTY_SHA256, sha256Hex, signRequest, uriEncode } from "./sigv4";
import type { S3Credentials } from "./sigv4";

export type S3Fetch = (
  input: string,
  init: RequestInit & { readonly redirect: "manual" }
) => Promise<Response>;

export interface S3StorageOptions {
  /** `https://s3.eu-west-1.amazonaws.com`, `http://minio:9000`… Origin only. */
  readonly endpoint: string;
  readonly bucket: string;
  /** `us-east-1` by default; Garage uses `garage`, R2 `auto`. */
  readonly region?: string;
  readonly credentials: S3Credentials;
  /**
   * `path` (default): `<endpoint>/<bucket>/<key>`, what MinIO and Garage
   * expect. `virtual`: `<bucket>.<endpoint host>/<key>` (AWS's preferred).
   */
  readonly urlStyle?: "path" | "virtual";
  /** Retention since upload. Default 30 days; no upper bound. */
  readonly maxAgeMs?: number;
  /** Objects bigger than this are treated as missing. Default 5 MB. */
  readonly maxBytes?: number;
  /** Per request. Default 10 s. */
  readonly timeoutMs?: number;
  /** Clock for expiry. Signing always uses the real time. */
  readonly now?: () => number;
  /** Default: the global `fetch`. */
  readonly fetch?: S3Fetch;
}

export class S3Error extends Error {
  override readonly name = "S3Error";
}

const PREFIX = "img/";
/** ListObjectsV2 pages are at most 1000 keys; their XML is far below this. */
const MAX_LIST_BYTES = 4 * 1024 * 1024;
const BUCKET_NAME = /^[\d.a-z-]{3,63}$/u;

const objectKey = (key: string): string => {
  if (!STORAGE_KEY_PATTERN.test(key)) {
    throw new TypeError("image proxy: unexpected storage key");
  }
  return `${PREFIX}${key.replaceAll(":", "/")}`;
};

interface Chunk {
  readonly done: boolean;
  readonly value?: unknown;
}

/** Read a body; `null` (and cancel) if it is not bytes or goes over `max`. */
const readAtMost = async (
  response: Response,
  max: number
): Promise<Uint8Array | null> => {
  const reader = response.body?.getReader();
  if (reader === undefined) {
    return new Uint8Array(0);
  }
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    // Chunks arrive in order.
    // oxlint-disable-next-line no-await-in-loop
    const chunk: Chunk = await reader.read();
    if (chunk.done) {
      break;
    }
    if (
      !(chunk.value instanceof Uint8Array) ||
      total + chunk.value.byteLength > max
    ) {
      // oxlint-disable-next-line no-await-in-loop
      await reader.cancel();
      return null;
    }
    total += chunk.value.byteLength;
    chunks.push(chunk.value);
  }
  const bytes = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return bytes;
};

const discard = async (response: Response): Promise<void> => {
  try {
    await response.body?.cancel();
  } catch {
    // Nothing to clean up.
  }
};

const XML_ENTITIES: Readonly<Record<string, string>> = {
  amp: "&",
  apos: "'",
  gt: ">",
  lt: "<",
  quot: '"',
};

const xmlText = (text: string): string =>
  text.replaceAll(
    /&(?<name>amp|apos|gt|lt|quot);/gu,
    (_m, name: string) => XML_ENTITIES[name] ?? ""
  );

const tag = (xml: string, name: string): string | undefined => {
  const value = new RegExp(`<${name}>(?<value>[^<]*)</${name}>`, "u").exec(xml)
    ?.groups?.value;
  return value === undefined ? undefined : xmlText(value);
};

/** The parts of a ListObjectsV2 answer we use. Our keys are plain hex. */
export const parseListing = (
  xml: string
): {
  readonly objects: readonly {
    readonly key: string;
    readonly lastModified: number;
  }[];
  readonly next: string | undefined;
} => {
  const objects: { key: string; lastModified: number }[] = [];
  for (const match of xml.matchAll(
    /<Contents>(?<body>[\S\s]*?)<\/Contents>/gu
  )) {
    const body = match.groups?.body ?? "";
    const key = tag(body, "Key");
    const lastModified = Date.parse(tag(body, "LastModified") ?? "");
    if (key !== undefined && Number.isFinite(lastModified)) {
      objects.push({ key, lastModified });
    }
  }
  const truncated = tag(xml, "IsTruncated") === "true";
  return {
    next: truncated ? tag(xml, "NextContinuationToken") : undefined,
    objects,
  };
};

const fail = async (response: Response, method: string): Promise<never> => {
  await discard(response);
  // Status only: S3 error bodies can echo the key or the bucket.
  throw new S3Error(
    `image proxy: S3 ${method} failed with ${String(response.status)}`
  );
};

type S3Method = "GET" | "PUT" | "DELETE";

interface S3Client {
  readonly request: (
    method: S3Method,
    path: string,
    body?: { readonly bytes: Uint8Array; readonly type: string }
  ) => Promise<Response>;
  readonly remove: (name: string) => Promise<void>;
  readonly list: (prefix: string) => AsyncGenerator<ListedObject>;
}

interface ListedObject {
  readonly key: string;
  readonly lastModified: number;
}

/** Signed requests to the configured endpoint only, redirects refused. */
const s3Client = (
  options: Omit<S3StorageOptions, "maxAgeMs" | "maxBytes" | "now">
): S3Client => {
  const endpoint = URL.parse(options.endpoint);
  if (
    endpoint === null ||
    (endpoint.protocol !== "https:" && endpoint.protocol !== "http:") ||
    endpoint.username !== "" ||
    endpoint.password !== "" ||
    endpoint.pathname !== "/" ||
    endpoint.search !== "" ||
    endpoint.hash !== ""
  ) {
    throw new TypeError(
      "image proxy: the S3 endpoint must be an http(s) origin, without path"
    );
  }
  if (!BUCKET_NAME.test(options.bucket)) {
    throw new TypeError("image proxy: invalid S3 bucket name");
  }
  const timeoutMs = options.timeoutMs ?? 10_000;
  const region = options.region ?? "us-east-1";
  const net: S3Fetch =
    options.fetch ??
    (async (input, init) => await globalThis.fetch(input, init));
  const base =
    (options.urlStyle ?? "path") === "path"
      ? `${endpoint.origin}/${options.bucket}`
      : `${endpoint.protocol}//${options.bucket}.${endpoint.host}`;

  const request = async (
    method: S3Method,
    path: string,
    body?: { readonly bytes: Uint8Array; readonly type: string }
  ): Promise<Response> => {
    const url = new URL(`${base}${path}`);
    const payloadHash =
      body === undefined ? EMPTY_SHA256 : await sha256Hex(body.bytes);
    const signed = await signRequest({
      credentials: options.credentials,
      date: new Date(),
      headers: {
        "x-amz-content-sha256": payloadHash,
        ...(body === undefined ? {} : { "content-type": body.type }),
      },
      method,
      payloadHash,
      region,
      service: "s3",
      url,
    });
    // `host` is set by fetch itself from the URL (the same value we signed).
    const { host: _host, ...headers } = signed.headers;
    const response = await net(url.href, {
      headers,
      method,
      redirect: "manual",
      signal: AbortSignal.timeout(timeoutMs),
      ...(body === undefined ? {} : { body: Uint8Array.from(body.bytes) }),
    });
    if (response.status >= 300 && response.status < 400) {
      await discard(response);
      throw new S3Error(`image proxy: S3 ${method} redirected (refused)`);
    }
    return response;
  };

  const remove = async (name: string): Promise<void> => {
    const response = await request("DELETE", `/${name}`);
    await discard(response);
    if (
      response.status !== 204 &&
      response.status !== 200 &&
      response.status !== 404
    ) {
      await fail(response, "DELETE");
    }
  };
  /**
   * Every object under `prefix`, page by page (ListObjectsV2).
   * @yields {ListedObject} Each object's key and last-modified time.
   */
  const list = async function* list(
    prefix: string
  ): AsyncGenerator<ListedObject> {
    let token: string | undefined;
    do {
      const query = [
        "list-type=2",
        `prefix=${uriEncode(prefix)}`,
        ...(token === undefined
          ? []
          : [`continuation-token=${uriEncode(token)}`]),
      ].join("&");
      // Pages are sequential: each needs the previous token.
      // oxlint-disable-next-line no-await-in-loop
      const response = await request("GET", `?${query}`);
      if (response.status !== 200) {
        // oxlint-disable-next-line no-await-in-loop
        await fail(response, "LIST");
      }
      // oxlint-disable-next-line no-await-in-loop
      const bytes = await readAtMost(response, MAX_LIST_BYTES);
      if (bytes === null) {
        throw new S3Error("image proxy: S3 listing too large");
      }
      const page = parseListing(new TextDecoder().decode(bytes));
      for (const object of page.objects) {
        if (object.key.startsWith(prefix)) {
          yield object;
        }
      }
      token = page.next;
    } while (token !== undefined);
  };
  return { list, remove, request };
};

export const createS3Storage = (
  options: S3StorageOptions
): SweepableStorage => {
  const { list, remove, request } = s3Client(options);
  const maxAgeMs = options.maxAgeMs ?? DEFAULT_LIMITS.cacheTtlMs;
  if (!Number.isSafeInteger(maxAgeMs) || maxAgeMs <= 0) {
    throw new TypeError("image proxy: maxAgeMs must be a positive integer");
  }
  const maxBytes = options.maxBytes ?? DEFAULT_LIMITS.maxBytesPerImage;
  const now = options.now ?? Date.now;

  const expired = (lastModified: number): boolean =>
    !Number.isFinite(lastModified) || lastModified + maxAgeMs <= now();

  return {
    delete: async (key) => {
      await remove(objectKey(key));
    },
    get: async (key): Promise<StoredImage | null> => {
      const name = objectKey(key);
      const response = await request("GET", `/${name}`);
      if (response.status === 404) {
        await discard(response);
        return null;
      }
      if (response.status !== 200) {
        return await fail(response, "GET");
      }
      const lastModified = Date.parse(
        response.headers.get("last-modified") ?? ""
      );
      if (expired(lastModified)) {
        await discard(response);
        await remove(name);
        return null;
      }
      const type = storedType(
        response.headers.get("content-type") ?? undefined
      );
      const declared = Number(response.headers.get("content-length") ?? "0");
      if (type === undefined || declared > maxBytes) {
        await discard(response);
        return null;
      }
      const bytes = await readAtMost(response, maxBytes);
      return bytes === null
        ? null
        : { bytes, expiresAt: lastModified + maxAgeMs, type };
    },
    // Entries live `maxAgeMs` from upload, whatever the ttl (`fixedTtlMs`).
    fixedTtlMs: maxAgeMs,
    put: async (key, image) => {
      const response = await request("PUT", `/${objectKey(key)}`, image);
      await discard(response);
      if (response.status !== 200) {
        await fail(response, "PUT");
      }
    },
    sweep: async () => {
      let deleted = 0;
      for await (const object of list(PREFIX)) {
        if (expired(object.lastModified)) {
          // One at a time: a sweep should not flood the server. (Batch
          // DeleteObjects needs Content-MD5, which WebCrypto lacks.)
          // oxlint-disable-next-line no-await-in-loop
          await remove(object.key);
          deleted += 1;
        }
      }
      return deleted;
    },
  };
};

const STATE_PREFIX = "pre/v1/";

const stateName = (key: string): string =>
  `${STATE_PREFIX}${checkStateKey(key).replace(":", "/")}`;

/**
 * Preload state in the same bucket, under `pre/v1/<kind>/
 * <hex>`: outside `img/`, so the sweep and an `img/` lifecycle rule never
 * delete an opt-out. Do not put a lifecycle rule on `pre/`.
 */
export const createS3StateStore = (
  options: Omit<S3StorageOptions, "maxAgeMs" | "maxBytes" | "now">
): PreloadStateStore => {
  const { list: listObjects, remove, request } = s3Client(options);
  return {
    delete: async (key) => {
      await remove(stateName(key));
    },
    list: async function* list(kind) {
      const prefix = `${STATE_PREFIX}${kind}/`;
      for await (const object of listObjects(prefix)) {
        const key = stateKeyOf(kind, object.key.slice(prefix.length));
        if (key !== undefined) {
          yield key;
        }
      }
    },
    get: async (key) => {
      const response = await request("GET", `/${stateName(key)}`);
      if (response.status === 404) {
        await discard(response);
        return null;
      }
      if (response.status !== 200) {
        return await fail(response, "GET");
      }
      const bytes = await readAtMost(response, MAX_STATE_BYTES);
      if (bytes === null) {
        return null;
      }
      try {
        return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
      } catch {
        return null;
      }
    },
    put: async (key, value) => {
      const bytes = new TextEncoder().encode(value);
      if (bytes.byteLength > MAX_STATE_BYTES) {
        throw new RangeError("image proxy: state record too large");
      }
      const response = await request("PUT", `/${stateName(key)}`, {
        bytes,
        type: "application/json",
      });
      await discard(response);
      if (response.status !== 200) {
        await fail(response, "PUT");
      }
    },
  };
};
