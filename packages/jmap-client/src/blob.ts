// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Binary data (RFC 8620 §6): download via `downloadUrl`, upload via
// `uploadUrl`. Both URLs come from the (origin-checked) session templates and
// are re-checked after expansion.

import { z } from "zod";

import { JmapError, parseOrThrow, requestErrorFromBody } from "./errors";
import { AccountId, BlobId } from "./ids";
import type { Session } from "./session";
import { UnsignedInt } from "./standard";
import { limitStream, readLimitedText } from "./transport";
import type { Transport } from "./transport";
import { checkUrl, expandTemplate } from "./url";
import type { OriginPolicy } from "./url";
import { xhrFor } from "./xhr";
import type { XhrUploads } from "./xhr";

const MAX_ERROR_BODY_BYTES = 64 * 1024;

export interface DownloadOptions {
  readonly accountId: AccountId;
  readonly blobId: BlobId;
  /** File name the server puts in Content-Disposition. */
  readonly name?: string;
  /** Content-Type the server should answer with. */
  readonly type?: string;
  /** Fail (`limit` error) instead of reading more than this many bytes. */
  readonly maxBytes?: number;
  readonly signal?: AbortSignal;
}

export const downloadUrlFor = (
  session: Session,
  policy: OriginPolicy,
  options: Pick<DownloadOptions, "accountId" | "blobId" | "name" | "type">
): URL =>
  checkUrl(
    expandTemplate(session.downloadUrl, {
      accountId: options.accountId,
      blobId: options.blobId,
      name: options.name ?? "download",
      type: options.type ?? "application/octet-stream",
    }),
    session.sessionUrl,
    policy
  );

/**
 * Downloads a blob. The returned `Response` body is size-limited when
 * `maxBytes` is set: a larger `Content-Length` fails before reading, and a
 * body that grows past the limit errors the stream.
 */
export const download = async (
  transport: Transport,
  session: Session,
  policy: OriginPolicy,
  options: DownloadOptions
): Promise<Response> => {
  const url = downloadUrlFor(session, policy, options);
  const response = await transport.send(url, { signal: options.signal });
  if (!response.ok) {
    const body = await readLimitedText(response, MAX_ERROR_BODY_BYTES);
    throw requestErrorFromBody(response.status, body);
  }
  const { maxBytes } = options;
  if (maxBytes === undefined) {
    return response;
  }
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
  return new Response(limitStream(response.body, maxBytes), {
    headers: response.headers,
    status: response.status,
    statusText: response.statusText,
  });
};

const UploadResponseSchema = z.looseObject({
  accountId: AccountId,
  blobId: BlobId,
  size: UnsignedInt,
  type: z.string(),
});
export type UploadResult = z.infer<typeof UploadResponseSchema>;

export type UploadBody =
  | ArrayBuffer
  | Blob
  | ReadableStream<Uint8Array>
  | Uint8Array<ArrayBuffer>;

export interface UploadOptions {
  /** Defaults to the Blob's type, then `application/octet-stream`. */
  readonly type?: string;
  /** Size of a stream body, if known, for the early `maxSizeUpload` check. */
  readonly size?: number;
  /**
   * Upload progress, and once more at the end. Stream bodies report bytes
   * as they are handed to fetch; Blob/ArrayBuffer bodies report real
   * network progress when the client has an XHR factory (`connect({ xhr })`),
   * otherwise only the final call (`JmapClient.uploadReportsProgress`).
   */
  readonly onProgress?: (sent: number, total: number | undefined) => void;
  readonly signal?: AbortSignal;
}

export const uploadUrlFor = (
  session: Session,
  policy: OriginPolicy,
  accountId: AccountId
): URL =>
  checkUrl(
    expandTemplate(session.uploadUrl, { accountId }),
    session.sessionUrl,
    policy
  );

const knownSize = (
  body: UploadBody,
  declared: number | undefined
): number | undefined => {
  if (body instanceof Blob) {
    return body.size;
  }
  if (body instanceof ArrayBuffer || body instanceof Uint8Array) {
    return body.byteLength;
  }
  return declared;
};

const tooLarge = (actual: number, max: number): JmapError =>
  new JmapError({ actual, kind: "limit", limit: "maxSizeUpload", max });

/** Counts bytes, reports progress and enforces `max` on a stream body. */
const meter = (
  body: ReadableStream<Uint8Array>,
  max: number,
  total: number | undefined,
  onProgress: UploadOptions["onProgress"]
): ReadableStream<Uint8Array> => {
  let sent = 0;
  return body.pipeThrough(
    new TransformStream<Uint8Array, Uint8Array>({
      transform: (chunk, controller) => {
        sent += chunk.byteLength;
        if (sent > max) {
          controller.error(tooLarge(sent, max));
          return;
        }
        controller.enqueue(chunk);
        onProgress?.(sent, total);
      },
    })
  );
};

/**
 * Whether an upload of `body` to `url` reports progress while it is sent.
 * Stream bodies count the bytes handed to fetch; other bodies only with XHR,
 * which is never used on the page's own origin (`xhrFor`). `false` means
 * `onProgress` only fires once, at the end: show an indeterminate bar.
 */
export const reportsUploadProgress = (
  xhr: XhrUploads | undefined,
  url: URL,
  body: UploadBody,
  sameOriginCookies = false
): boolean =>
  body instanceof ReadableStream ||
  xhrFor(xhr, url, sameOriginCookies) !== undefined;

/** Sends the upload; returns the response status and (limited) text. */
const sendUpload = async (
  transport: Transport,
  url: URL,
  body: UploadBody,
  context: {
    readonly type: string;
    readonly max: number;
    readonly size: number | undefined;
    readonly options: UploadOptions;
    readonly xhr: XhrUploads | undefined;
  }
): Promise<{ readonly status: number; readonly text: string }> => {
  const { max, options, size, type } = context;
  // Never XHR on the page's own origin: it would send its cookies (unless
  // those cookies are the credential, `sameOriginCookies`).
  const xhr = xhrFor(context.xhr, url, transport.sameOriginCookies);
  const headers = { "Content-Type": type };
  if (body instanceof ReadableStream) {
    const response = await transport.send(url, {
      body: meter(body, max, size, options.onProgress),
      headers,
      method: "POST",
      replayable: false,
      signal: options.signal,
    });
    return {
      status: response.status,
      text: await readLimitedText(response, MAX_ERROR_BODY_BYTES),
    };
  }
  if (xhr !== undefined && options.onProgress !== undefined) {
    const response = await transport.sendWithProgress(xhr, {
      body,
      headers,
      onProgress: options.onProgress,
      signal: options.signal,
      url,
    });
    if (response.text.length > MAX_ERROR_BODY_BYTES) {
      throw new JmapError({
        actual: response.text.length,
        kind: "limit",
        limit: "maxBytes",
        max: MAX_ERROR_BODY_BYTES,
      });
    }
    return response;
  }
  const response = await transport.send(url, {
    body,
    headers,
    method: "POST",
    replayable: true,
    signal: options.signal,
  });
  return {
    status: response.status,
    text: await readLimitedText(response, MAX_ERROR_BODY_BYTES),
  };
};

/**
 * Uploads binary data and returns its blob id. Fails early with a `limit`
 * error if the size is known to exceed the server's `maxSizeUpload`. With
 * `onProgress` and `xhr`, non-stream bodies to another origin than the
 * page's go through XMLHttpRequest so progress is reported as bytes leave.
 */
export const upload = async (
  transport: Transport,
  session: Session,
  policy: OriginPolicy,
  accountId: AccountId,
  body: UploadBody,
  options: UploadOptions = {},
  xhr?: XhrUploads
): Promise<UploadResult> => {
  const max = session.core.maxSizeUpload;
  const size = knownSize(body, options.size);
  if (size !== undefined && size > max) {
    throw tooLarge(size, max);
  }
  const url = uploadUrlFor(session, policy, accountId);
  const type =
    options.type ??
    (body instanceof Blob && body.type !== "" ? body.type : undefined) ??
    "application/octet-stream";
  const { status, text } = await sendUpload(transport, url, body, {
    max,
    options,
    size,
    type,
    xhr,
  });
  if (status < 200 || status > 299) {
    throw requestErrorFromBody(status, text);
  }
  let json: unknown = undefined;
  try {
    json = JSON.parse(text);
  } catch (error) {
    throw new JmapError(
      { issues: ["upload response is not JSON"], kind: "invalidResponse" },
      { cause: error }
    );
  }
  const result = parseOrThrow(UploadResponseSchema, json);
  options.onProgress?.(result.size, result.size);
  return result;
};
