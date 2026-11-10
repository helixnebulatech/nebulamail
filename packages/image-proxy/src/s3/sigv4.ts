// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// AWS Signature Version 4 (header auth), WebCrypto only, so it runs on Node,
// Workers and Vercel without an SDK (no 100-package AWS SDK for
// four requests). Checked against AWS's published test suite and the S3
// examples in tests (`sigv4.test.ts`) and live against MinIO.
//
// What it covers is what our S3 storage sends: GET, PUT, DELETE and a list
// with a query string, signed headers `host`, `x-amz-*` and any others the
// caller passes. Paths are encoded once (S3 style), not twice like other
// AWS services; `doubleEncodePath` exists for the generic test suite.

export interface S3Credentials {
  readonly accessKeyId: string;
  readonly secretAccessKey: string;
  /** Temporary credentials (STS): sent as `x-amz-security-token`. */
  readonly sessionToken?: string;
}

export interface SignInput {
  readonly method: string;
  readonly url: URL;
  /** Headers to sign and send (any case). `host` and `x-amz-date` are added. */
  readonly headers: Readonly<Record<string, string>>;
  /** Hex SHA-256 of the body (`EMPTY_SHA256` for none). */
  readonly payloadHash: string;
  readonly region: string;
  readonly service: string;
  readonly credentials: S3Credentials;
  readonly date: Date;
  /** Non-S3 services encode the path twice. Default false (S3). */
  readonly doubleEncodePath?: boolean;
}

export interface Signed {
  /** Every header to send, `authorization` included (lowercase names). */
  readonly headers: Record<string, string>;
  readonly canonicalRequest: string;
  readonly stringToSign: string;
  readonly signature: string;
}

export const EMPTY_SHA256 =
  "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

const encoder = new TextEncoder();

const hex = (bytes: ArrayBuffer): string =>
  [...new Uint8Array(bytes)]
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");

export const sha256Hex = async (data: Uint8Array | string): Promise<string> =>
  hex(
    await crypto.subtle.digest(
      "SHA-256",
      typeof data === "string" ? encoder.encode(data) : Uint8Array.from(data)
    )
  );

const hmac = async (
  key: ArrayBuffer | Uint8Array,
  data: string
): Promise<ArrayBuffer> => {
  const cryptoKey = await crypto.subtle.importKey(
    "raw",
    key instanceof Uint8Array ? Uint8Array.from(key) : key,
    { hash: "SHA-256", name: "HMAC" },
    false,
    ["sign"]
  );
  return await crypto.subtle.sign("HMAC", cryptoKey, encoder.encode(data));
};

/** RFC 3986 percent-encoding: everything but `A-Za-z0-9-._~`. */
export const uriEncode = (text: string): string =>
  [...encoder.encode(text)]
    .map((byte) => {
      const char = String.fromCodePoint(byte);
      return /^[\w.~-]$/u.test(char)
        ? char
        : `%${byte.toString(16).toUpperCase().padStart(2, "0")}`;
    })
    .join("");

const safeDecode = (text: string): string => {
  try {
    return decodeURIComponent(text);
  } catch {
    return text;
  }
};

const canonicalUri = (url: URL, twice: boolean): string => {
  const path = url.pathname === "" ? "/" : url.pathname;
  return path
    .split("/")
    .map((segment) => {
      const once = uriEncode(safeDecode(segment));
      return twice ? uriEncode(once) : once;
    })
    .join("/");
};

const canonicalQuery = (url: URL): string => {
  const raw = url.search.startsWith("?") ? url.search.slice(1) : url.search;
  if (raw === "") {
    return "";
  }
  return raw
    .split("&")
    .filter((pair) => pair !== "")
    .map((pair) => {
      const eq = pair.indexOf("=");
      const name = eq === -1 ? pair : pair.slice(0, eq);
      const value = eq === -1 ? "" : pair.slice(eq + 1);
      return [
        uriEncode(safeDecode(name)),
        uriEncode(safeDecode(value)),
      ] as const;
    })
    .toSorted(([a, av], [b, bv]) => {
      if (a !== b) {
        return a < b ? -1 : 1;
      }
      if (av === bv) {
        return 0;
      }
      return av < bv ? -1 : 1;
    })
    .map(([name, value]) => `${name}=${value}`)
    .join("&");
};

/** `20150830T123600Z`. */
export const amzDate = (date: Date): string =>
  date.toISOString().replaceAll(/[:-]|\.\d{3}/gu, "");

const signingKey = async (
  secret: string,
  day: string,
  region: string,
  service: string
): Promise<ArrayBuffer> => {
  const kDate = await hmac(encoder.encode(`AWS4${secret}`), day);
  const kRegion = await hmac(kDate, region);
  const kService = await hmac(kRegion, service);
  return await hmac(kService, "aws4_request");
};

export const signRequest = async (input: SignInput): Promise<Signed> => {
  const stamp = amzDate(input.date);
  const day = stamp.slice(0, 8);
  const headers = new Map<string, string[]>();
  const add = (name: string, value: string): void => {
    const key = name.toLowerCase();
    headers.set(key, [...(headers.get(key) ?? []), value]);
  };
  for (const [name, value] of Object.entries(input.headers)) {
    add(name, value);
  }
  if (!headers.has("host")) {
    add("host", input.url.host);
  }
  if (!headers.has("x-amz-date")) {
    add("x-amz-date", stamp);
  }
  if (input.credentials.sessionToken !== undefined) {
    add("x-amz-security-token", input.credentials.sessionToken);
  }
  const names = [...headers.keys()].toSorted();
  const canonicalHeaders = names
    .map(
      (name) =>
        `${name}:${(headers.get(name) ?? [])
          .map((value) => value.trim().replaceAll(/\s+/gu, " "))
          .join(",")}\n`
    )
    .join("");
  const signedHeaders = names.join(";");
  const canonicalRequest = [
    input.method.toUpperCase(),
    canonicalUri(input.url, input.doubleEncodePath ?? false),
    canonicalQuery(input.url),
    canonicalHeaders,
    signedHeaders,
    input.payloadHash,
  ].join("\n");
  const scope = `${day}/${input.region}/${input.service}/aws4_request`;
  const stringToSign = [
    "AWS4-HMAC-SHA256",
    stamp,
    scope,
    await sha256Hex(canonicalRequest),
  ].join("\n");
  const key = await signingKey(
    input.credentials.secretAccessKey,
    day,
    input.region,
    input.service
  );
  const signature = hex(await hmac(key, stringToSign));
  const out: Record<string, string> = {};
  for (const name of names) {
    out[name] = (headers.get(name) ?? []).join(",");
  }
  out.authorization = `AWS4-HMAC-SHA256 Credential=${input.credentials.accessKeyId}/${scope}, SignedHeaders=${signedHeaders}, Signature=${signature}`;
  return { canonicalRequest, headers: out, signature, stringToSign };
};
