// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Just enough MIME (RFC 2045/2046) to find the HTML bodies of an incoming
// email for `preload`. Read-only and bounded: depth, parts and
// output size are capped, and anything odd is skipped (a skipped image is
// only "not preloaded": the email then works as in cache mode).
//
// Stalwart's MTA Hook sends the top-level headers as `[name, value]` pairs
// and the rest of the message as `contents` (a string: Stalwart decodes the
// raw bytes as UTF-8, lossily). 8-bit bodies in other charsets are therefore
// already damaged before they reach us; base64 and quoted-printable bodies
// (nearly all HTML mail) decode exactly.

export interface MimeLimits {
  readonly maxDepth: number;
  readonly maxParts: number;
  /** Total characters of HTML returned. */
  readonly maxHtmlChars: number;
}

export const DEFAULT_MIME_LIMITS: MimeLimits = Object.freeze({
  maxDepth: 8,
  maxHtmlChars: 5 * 1024 * 1024,
  maxParts: 100,
});

export type HeaderList = readonly (readonly [string, string])[];

interface ContentType {
  readonly type: string;
  readonly params: ReadonlyMap<string, string>;
}

const unfold = (value: string): string =>
  value.replaceAll(/\r?\n[\t ]+/gu, " ").trim();

/** First value of each header, by lowercase name. */
const headerMap = (headers: HeaderList): Map<string, string> => {
  const map = new Map<string, string>();
  for (const [name, value] of headers) {
    const key = name.trim().toLowerCase();
    if (!map.has(key)) {
      map.set(key, unfold(value));
    }
  }
  return map;
};

/** Split a header block from its body; the block ends at the first empty line. */
export const splitHeaders = (
  text: string
): { readonly headers: HeaderList; readonly body: string } => {
  const end = /\r?\n\r?\n/u.exec(text);
  const block = end === null ? text : text.slice(0, end.index);
  const body = end === null ? "" : text.slice(end.index + end[0].length);
  const headers: [string, string][] = [];
  for (const line of block.split(/\r?\n/u)) {
    const last = headers.at(-1);
    if (/^[\t ]/u.test(line) && last !== undefined) {
      last[1] = `${last[1]} ${line.trim()}`;
    } else {
      const colon = line.indexOf(":");
      if (colon > 0) {
        headers.push([line.slice(0, colon), line.slice(colon + 1).trim()]);
      }
    }
  }
  return { body, headers };
};

/** `type/subtype; a=b; c="d"` → lowercase type and parameters. */
export const parseContentType = (value: string | undefined): ContentType => {
  const [head = "", ...rest] = (value ?? "").split(";");
  const params = new Map<string, string>();
  for (const part of rest) {
    const eq = part.indexOf("=");
    if (eq > 0) {
      const name = part.slice(0, eq).trim().toLowerCase();
      let raw = part.slice(eq + 1).trim();
      if (raw.startsWith('"') && raw.endsWith('"') && raw.length >= 2) {
        raw = raw.slice(1, -1).replaceAll(/\\(?<char>.)/gu, "$<char>");
      }
      if (!params.has(name)) {
        params.set(name, raw);
      }
    }
  }
  const type = head.trim().toLowerCase();
  return { params, type: type === "" ? "text/plain" : type };
};

const EQUALS = 0x3d;
const CR = 0x0d;
const LF = 0x0a;
const SPACE = 0x20;
const TAB = 0x09;

/** Value of an ASCII hex digit, or -1. */
const hexValue = (byte: number | undefined): number => {
  if (byte === undefined) {
    return -1;
  }
  if (byte >= 0x30 && byte <= 0x39) {
    return byte - 0x30;
  }
  // oxlint-disable-next-line no-bitwise -- ASCII lower case.
  const lower = byte | 0x20;
  return lower >= 0x61 && lower <= 0x66 ? lower - 0x61 + 10 : -1;
};

/** Index after a soft line break (`=` [\t ]* CR? LF) at `i`, or -1. */
const softBreakEnd = (bytes: Uint8Array, i: number): number => {
  let j = i + 1;
  while (bytes[j] === SPACE || bytes[j] === TAB) {
    j += 1;
  }
  if (bytes[j] === CR) {
    j += 1;
  }
  return bytes[j] === LF ? j + 1 : -1;
};

/**
 * Quoted-printable → bytes, in one pass over the UTF-8 encoding of `text`
 * (non-ASCII characters are kept as UTF-8). `null` when the output would
 * pass `maxBytes`: each output byte takes 1 to 3 characters, so a text of
 * more than 3 × `maxBytes` characters is refused before encoding it.
 */
export const decodeQuotedPrintable = (
  text: string,
  maxBytes = Number.POSITIVE_INFINITY
): Uint8Array | null => {
  if (text.length > 3 * maxBytes) {
    return null;
  }
  // Decoded in place: the output is never longer than the input.
  const bytes = new TextEncoder().encode(text);
  let out = 0;
  let i = 0;
  while (i < bytes.length) {
    const byte = bytes[i] ?? 0;
    if (byte === EQUALS) {
      const end = softBreakEnd(bytes, i);
      const high = hexValue(bytes[i + 1]);
      const low = hexValue(bytes[i + 2]);
      if (end !== -1) {
        i = end;
      } else if (high !== -1 && low !== -1) {
        bytes[out] = high * 16 + low;
        out += 1;
        i += 3;
      } else {
        bytes[out] = byte;
        out += 1;
        i += 1;
      }
    } else {
      bytes[out] = byte;
      out += 1;
      i += 1;
    }
  }
  return out > maxBytes ? null : bytes.subarray(0, out);
};

/**
 * Base64 → bytes; `null` when it is not base64 or would decode to more
 * than `maxBytes` (checked before decoding).
 */
export const decodeBase64 = (
  text: string,
  maxBytes = Number.POSITIVE_INFINITY
): Uint8Array | null => {
  const clean = text.replaceAll(/\s+/gu, "");
  if (!/^[\d+/A-Za-z]*={0,2}$/u.test(clean)) {
    return null;
  }
  if (Math.floor((clean.length * 3) / 4) > maxBytes + 2) {
    return null;
  }
  let binary: string;
  try {
    binary = atob(clean);
  } catch {
    return null;
  }
  if (binary.length > maxBytes) {
    return null;
  }
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.codePointAt(i) ?? 0;
  }
  return bytes;
};

const decodeText = (bytes: Uint8Array, charset: string | undefined): string => {
  try {
    return new TextDecoder(charset ?? "utf-8").decode(bytes);
  } catch {
    // Unknown charset label: the best guess is UTF-8.
    return new TextDecoder("utf-8").decode(bytes);
  }
};

/**
 * Body of a leaf part as text, by its Content-Transfer-Encoding; `null` if
 * it cannot be read or would not fit in `maxChars`.
 *
 * Encoded parts are refused before decoding when their bytes pass
 * 4 × `maxChars`: a heuristic that fails closed (one UTF-16 unit can take
 * up to 4 bytes, e.g. in gb18030, and ISO-2022-JP escapes take bytes that
 * give no text). The caller still checks the decoded length.
 */
const leafText = (
  headers: ReadonlyMap<string, string>,
  body: string,
  charset: string | undefined,
  maxChars: number
): string | null => {
  const encoding = (headers.get("content-transfer-encoding") ?? "7bit")
    .trim()
    .toLowerCase();
  const maxBytes = 4 * maxChars;
  if (encoding === "base64") {
    const bytes = decodeBase64(body, maxBytes);
    return bytes === null ? null : decodeText(bytes, charset);
  }
  if (encoding === "quoted-printable") {
    const bytes = decodeQuotedPrintable(body, maxBytes);
    return bytes === null ? null : decodeText(bytes, charset);
  }
  // 7bit / 8bit / binary: Stalwart already turned the bytes into a string.
  return body.length > maxChars ? null : body;
};

const escapeRegExp = (text: string): string =>
  text.replaceAll(/[$()*+.?[\\\]^{|}]/gu, String.raw`\$&`);

/** The body parts of a multipart body (preamble and epilogue dropped). */
const splitMultipart = (body: string, boundary: string): string[] => {
  const delimiter = new RegExp(
    `(?:^|\\r?\\n)--${escapeRegExp(boundary)}(--)?[\\t ]*(?:\\r?\\n|$)`,
    "gu"
  );
  const parts: string[] = [];
  let start: number | undefined;
  for (const match of body.matchAll(delimiter)) {
    if (start !== undefined) {
      parts.push(body.slice(start, match.index));
    }
    if (match[1] === "--") {
      start = undefined;
      break;
    }
    start = match.index + match[0].length;
  }
  if (start !== undefined) {
    // No closing delimiter: keep the last part anyway.
    parts.push(body.slice(start));
  }
  return parts;
};

/**
 * The HTML bodies of an email, in order: every `text/html` part that is not
 * an attachment, inside any multipart. Never throws.
 */
export const htmlBodies = (
  headers: HeaderList,
  body: string,
  limits: MimeLimits = DEFAULT_MIME_LIMITS
): string[] => {
  const out: string[] = [];
  let parts = 0;
  let chars = 0;
  const walk = (list: HeaderList, text: string, depth: number): void => {
    parts += 1;
    if (parts > limits.maxParts || depth > limits.maxDepth) {
      return;
    }
    const map = headerMap(list);
    const contentType = parseContentType(map.get("content-type"));
    const boundary = contentType.params.get("boundary");
    if (contentType.type.startsWith("multipart/")) {
      if (boundary !== undefined && boundary !== "") {
        for (const part of splitMultipart(text, boundary)) {
          const child = splitHeaders(part);
          walk(child.headers, child.body, depth + 1);
        }
      }
      return;
    }
    const disposition = parseContentType(map.get("content-disposition")).type;
    if (contentType.type !== "text/html" || disposition === "attachment") {
      return;
    }
    const html = leafText(
      map,
      text,
      contentType.params.get("charset"),
      limits.maxHtmlChars - chars
    );
    if (html !== null && chars + html.length <= limits.maxHtmlChars) {
      chars += html.length;
      out.push(html);
    }
  };
  try {
    walk(headers, body, 0);
  } catch {
    // Malformed input: whatever was found so far.
  }
  return out;
};
