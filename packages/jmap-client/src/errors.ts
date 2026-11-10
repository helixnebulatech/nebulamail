// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// One error class for everything the client can fail with. The `detail`
// field is a discriminated union on `kind`, so callers can `switch` on it and
// TypeScript checks they handled every case. Messages never contain tokens
// or request headers.

import { z } from "zod";

import type { OtherString } from "./other-string";

// RFC 8620 §3.6.1 request-level problem types (plus anything else).
export type RequestErrorType =
  | "urn:ietf:params:jmap:error:limit"
  | "urn:ietf:params:jmap:error:notJSON"
  | "urn:ietf:params:jmap:error:notRequest"
  | "urn:ietf:params:jmap:error:unknownCapability"
  | OtherString;

// RFC 8620 §3.6.2 / RFC 8621 method-level error types (plus extensions).
export type MethodErrorType =
  | "accountNotFound"
  | "accountNotSupportedByMethod"
  | "accountReadOnly"
  | "anchorNotFound"
  | "cannotCalculateChanges"
  /** Stalwart 1.0: a recurrence it can't expand. */
  | "cannotCalculateOccurrences"
  /** Stalwart 1.0: `expandRecurrences` over too long a time range. */
  | "expandDurationTooLarge"
  | "forbidden"
  | "fromAccountNotFound"
  | "fromAccountNotSupportedByMethod"
  | "invalidArguments"
  | "invalidResultReference"
  /** Stalwart 1.0: an object it can't find (e.g. a blob to parse). */
  | "notFound"
  /** Stalwart 1.0: too many requests; try later. */
  | "rateLimit"
  | "requestTooLarge"
  | "serverFail"
  | "serverPartialFail"
  | "serverUnavailable"
  | "stateMismatch"
  /** Stalwart 1.0: an argument or result over a size limit. */
  | "tooLarge"
  | "tooManyChanges"
  | "unknownDataType"
  | "unknownMethod"
  | "unsupportedFilter"
  | "unsupportedSort"
  | OtherString;

export type JmapErrorDetail =
  /** `fetch` itself rejected (DNS, TLS, CORS, connection reset...). */
  | { readonly kind: "network" }
  /** The caller's AbortSignal fired. */
  | { readonly kind: "aborted" }
  /** `getAccessToken()` threw. */
  | { readonly kind: "auth" }
  /** Still 401 after refreshing the token once. */
  | { readonly kind: "unauthorized" }
  /** Non-2xx HTTP status without a JMAP problem document. */
  | { readonly kind: "http"; readonly status: number }
  /** RFC 7807 problem document (RFC 8620 §3.6.1). */
  | {
      readonly kind: "request";
      readonly status: number;
      readonly type: RequestErrorType;
      readonly detail?: string;
      readonly limit?: string;
    }
  /** `["error", {...}, callId]` for one method call (RFC 8620 §3.6.2). */
  | {
      readonly kind: "method";
      readonly method: string;
      readonly callId: string;
      readonly type: MethodErrorType;
      readonly description?: string;
      readonly arguments: Readonly<Record<string, unknown>>;
    }
  /** The server answered something that does not match RFC 8620/8621. */
  | {
      readonly kind: "invalidResponse";
      readonly issues: readonly string[];
    }
  /** A server-provided URL is not http(s) or not a valid template. */
  | { readonly kind: "invalidUrl"; readonly url: string }
  /** A server-provided URL points to an origin that is not allowed. */
  | {
      readonly kind: "crossOrigin";
      readonly url: string;
      readonly allowedOrigins: readonly string[];
    }
  /** A client-side pre-check against a server or caller limit failed. */
  | {
      readonly kind: "limit";
      readonly limit:
        | "maxBytes"
        | "maxCallsInRequest"
        | "maxSizeRequest"
        | "maxSizeUpload"
        | "maxRedirects";
      readonly max: number;
      readonly actual: number;
    }
  /** The request builder was used incorrectly (e.g. a foreign reference). */
  | { readonly kind: "usage"; readonly reason: string };

export type JmapErrorKind = JmapErrorDetail["kind"];

const describe = (detail: JmapErrorDetail): string => {
  switch (detail.kind) {
    case "network": {
      return "JMAP: network error";
    }
    case "aborted": {
      return "JMAP: aborted";
    }
    case "auth": {
      return "JMAP: could not obtain an access token";
    }
    case "unauthorized": {
      return "JMAP: unauthorized (401 after token refresh)";
    }
    case "http": {
      return `JMAP: HTTP ${String(detail.status)}`;
    }
    case "request": {
      return `JMAP request error: ${detail.type}`;
    }
    case "method": {
      return `JMAP method error in ${detail.method} (${detail.callId}): ${detail.type}`;
    }
    case "invalidResponse": {
      return `JMAP: invalid server response (${detail.issues.slice(0, 3).join("; ")})`;
    }
    case "invalidUrl": {
      return "JMAP: invalid URL from server";
    }
    case "crossOrigin": {
      return "JMAP: server URL points to a different origin";
    }
    case "limit": {
      return `JMAP: ${detail.limit} exceeded (${String(detail.actual)} > ${String(detail.max)})`;
    }
    case "usage": {
      return `JMAP: ${detail.reason}`;
    }
    default: {
      return "JMAP error";
    }
  }
};

export class JmapError extends Error {
  readonly detail: JmapErrorDetail;

  constructor(detail: JmapErrorDetail, options?: { readonly cause?: unknown }) {
    super(describe(detail), options);
    this.name = "JmapError";
    this.detail = detail;
  }

  get kind(): JmapErrorKind {
    return this.detail.kind;
  }
}

export const isJmapError = (value: unknown): value is JmapError =>
  value instanceof JmapError;

/** A method-level error (`["error", {type}, callId]`) of this type. */
export const isMethodError = (
  value: unknown,
  type: MethodErrorType
): value is JmapError =>
  isJmapError(value) &&
  value.detail.kind === "method" &&
  value.detail.type === type;

/** Converts zod issues into short strings (paths only, never values). */
export const zodIssues = (error: z.ZodError): string[] =>
  error.issues.map(
    (issue) => `${issue.path.map(String).join(".") || "(root)"}: ${issue.code}`
  );

export const invalidResponse = (error: z.ZodError): JmapError =>
  new JmapError({ issues: zodIssues(error), kind: "invalidResponse" });

/** Parses with zod and turns failures into `invalidResponse` errors. */
export const parseOrThrow = <T>(schema: z.ZodType<T>, raw: unknown): T => {
  const result = schema.safeParse(raw);
  if (result.success) {
    return result.data;
  }
  throw invalidResponse(result.error);
};

const ProblemSchema = z.looseObject({
  detail: z.string().optional(),
  limit: z.string().optional(),
  status: z.number().optional(),
  type: z.string(),
});

/** Builds the error for a non-2xx response body (already read as text). */
export const requestErrorFromBody = (
  status: number,
  body: string
): JmapError => {
  let json: unknown = undefined;
  try {
    json = JSON.parse(body);
  } catch {
    return new JmapError({ kind: "http", status });
  }
  const problem = ProblemSchema.safeParse(json);
  if (!problem.success) {
    return new JmapError({ kind: "http", status });
  }
  const { detail, limit, type } = problem.data;
  return new JmapError({
    kind: "request",
    status,
    type,
    ...(detail === undefined ? {} : { detail }),
    ...(limit === undefined ? {} : { limit }),
  });
};
