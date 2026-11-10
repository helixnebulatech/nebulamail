// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// DOM-free classification of image URLs found in an email.
// Shared by the browser pipeline and the BFF extractor so
// both decide the same way.

/** Longer image URLs are blocked (and never matched against tracker regexes). */
export const MAX_IMAGE_URL_LENGTH = 2048;

/**
 * Raster types shown inline from `data:`. SVG is excluded on purpose (see
 * README "Decisions"): it is the only image format that is a document, it
 * has a history of image-mode browser bugs, it can be a CPU bomb, and real
 * email clients (Gmail, Outlook) do not render it, so legitimate mail does
 * not rely on it.
 */
const INLINE_IMAGE_TYPES = new Set([
  "image/apng",
  "image/avif",
  "image/bmp",
  "image/gif",
  "image/jpeg",
  "image/jpg",
  "image/pjpeg",
  "image/png",
  "image/vnd.microsoft.icon",
  "image/webp",
  "image/x-icon",
  "image/x-ms-bmp",
]);

export type ImageUrlClass =
  | { readonly kind: "inline"; readonly url: string }
  | { readonly kind: "cid"; readonly cid: string }
  | { readonly kind: "remote"; readonly url: string; readonly secure: boolean }
  | { readonly kind: "blocked"; readonly reason: BlockedImageReason };

export type BlockedImageReason =
  | "empty"
  | "relative"
  | "svg"
  | "data-other"
  | "scheme"
  | "too-long"
  | "invalid"
  | "too-many";

// The WHATWG URL parser strips leading/trailing C0 controls and spaces and
// removes ASCII tab and newline anywhere; do the same before deciding.
// Index scans, not a regex: `/[\0- ]+$/` is quadratic on a long run of
// spaces followed by anything else.
const TAB_OR_NEWLINE = /[\t\n\r]/gu;
const SCHEME = /^(?<scheme>[a-z][a-z\d+.-]*):/iu;

const isC0OrSpace = (raw: string, index: number): boolean =>
  (raw.codePointAt(index) ?? 0x21) <= 0x20;

export const normalizeUrlInput = (raw: string): string => {
  let start = 0;
  let end = raw.length;
  while (start < end && isC0OrSpace(raw, start)) {
    start += 1;
  }
  while (end > start && isC0OrSpace(raw, end - 1)) {
    end -= 1;
  }
  return raw.slice(start, end).replaceAll(TAB_OR_NEWLINE, "");
};

/** `schemeOf` for input that already went through `normalizeUrlInput`. */
const schemeOfNormalized = (url: string): string | null => {
  const scheme = SCHEME.exec(url)?.groups?.scheme;
  return scheme === undefined ? null : scheme.toLowerCase();
};

/** Lower-case scheme without the colon, or null for relative references. */
export const schemeOf = (raw: string): string | null =>
  schemeOfNormalized(normalizeUrlInput(raw));

/** `text` without trailing `char`s, in linear time (`/,+$/` is quadratic). */
export const trimEndChar = (text: string, char: string): string => {
  let end = text.length;
  while (end > 0 && text[end - 1] === char) {
    end -= 1;
  }
  return text.slice(0, end);
};

/**
 * A Content-ID header value (RFC 2045 §7) → the key used for lookups
 * (lower-case, no `<>`). Never percent-decoded: only `cid:` URLs are.
 */
export const normalizeContentId = (value: string): string =>
  value.trim().replaceAll(/^<|>$/gu, "").trim().toLowerCase();

/**
 * A `cid:` URL's value (without the scheme) → the same key as
 * `normalizeContentId` of the Content-ID it names. RFC 2392 §2: the URL is
 * percent-decoded to get the Content-ID.
 */
export const normalizeCid = (value: string): string => {
  let cid = value.trim();
  try {
    cid = decodeURIComponent(cid);
  } catch {
    // Keep the raw value: a malformed escape is just an unknown Content-ID.
  }
  return normalizeContentId(cid);
};

/**
 * The `cid:` URL of a Content-ID (without `<>`). RFC 2392 §2: characters
 * not allowed in a URL (and `%` itself) are percent-encoded, so
 * `normalizeCid` gets the same Content-ID back.
 */
export const cidUrl = (contentId: string): string =>
  `cid:${encodeURIComponent(contentId).replaceAll("%40", "@")}`;

const classifyData = (url: string): ImageUrlClass => {
  const header = url.slice(
    "data:".length,
    url.includes(",") ? url.indexOf(",") : undefined
  );
  const mediaType = (header.split(";")[0] ?? "").trim().toLowerCase();
  if (mediaType === "image/svg+xml") {
    return { kind: "blocked", reason: "svg" };
  }
  if (INLINE_IMAGE_TYPES.has(mediaType) && url.includes(",")) {
    return { kind: "inline", url };
  }
  return { kind: "blocked", reason: "data-other" };
};

const classifyRemote = (url: string, scheme: string): ImageUrlClass => {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return { kind: "blocked", reason: "invalid" };
  }
  if (
    parsed.hostname === "" ||
    parsed.username !== "" ||
    parsed.password !== ""
  ) {
    return { kind: "blocked", reason: "invalid" };
  }
  if (parsed.href.length > MAX_IMAGE_URL_LENGTH) {
    return { kind: "blocked", reason: "too-long" };
  }
  return { kind: "remote", secure: scheme === "https", url: parsed.href };
};

export const classifyImageUrl = (raw: string): ImageUrlClass => {
  const url = normalizeUrlInput(raw);
  if (url === "") {
    return { kind: "blocked", reason: "empty" };
  }
  const scheme = schemeOfNormalized(url);
  if (scheme === null) {
    // `/x.png`, `x.png`, `//host/x.png`, `\\host\x.png`: would resolve
    // against our own origin, so always blocked.
    return { kind: "blocked", reason: "relative" };
  }
  switch (scheme) {
    case "data": {
      return classifyData(url);
    }
    case "cid": {
      const cid = normalizeCid(url.slice("cid:".length));
      return cid === ""
        ? { kind: "blocked", reason: "empty" }
        : { cid, kind: "cid" };
    }
    case "http":
    case "https": {
      return classifyRemote(url, scheme);
    }
    default: {
      return { kind: "blocked", reason: "scheme" };
    }
  }
};

export interface SrcsetCandidate {
  readonly url: string;
  readonly descriptor: string;
}

/**
 * Splits a srcset into candidates. Simplified from the HTML algorithm: URLs
 * are whitespace-delimited and commas separate candidates. A URL containing
 * a comma may be split wrongly; the pieces are then classified like any
 * other URL (blocked or proxied), so this can break an image, never leak.
 */
const isSpace = (ch: string): boolean =>
  ch === " " || ch === "\t" || ch === "\n" || ch === "\r" || ch === "\f";

export const parseSrcset = (srcset: string): SrcsetCandidate[] => {
  const candidates: SrcsetCandidate[] = [];
  let i = 0;
  while (i < srcset.length) {
    while (
      i < srcset.length &&
      (isSpace(srcset[i] ?? "") || srcset[i] === ",")
    ) {
      i += 1;
    }
    const urlStart = i;
    while (i < srcset.length && !isSpace(srcset[i] ?? "")) {
      i += 1;
    }
    let url = srcset.slice(urlStart, i);
    let descriptor = "";
    if (url.endsWith(",")) {
      url = trimEndChar(url, ",");
    } else {
      const descriptorStart = i;
      while (i < srcset.length && srcset[i] !== ",") {
        i += 1;
      }
      descriptor = srcset.slice(descriptorStart, i).trim();
    }
    if (url !== "") {
      candidates.push({ descriptor, url });
    }
  }
  return candidates;
};
