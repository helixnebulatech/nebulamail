// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Image type checks for the image proxy: the declared
// Content-Type must be allowed AND match the first bytes. SVG is never
// allowed: it is a document that can carry scripts and remote references.

export type ImageType =
  | "image/png"
  | "image/jpeg"
  | "image/gif"
  | "image/webp"
  | "image/avif"
  | "image/x-icon"
  | "image/bmp";

export const IMAGE_TYPES: readonly ImageType[] = Object.freeze([
  "image/png",
  "image/jpeg",
  "image/gif",
  "image/webp",
  "image/avif",
  "image/x-icon",
  "image/bmp",
]);

/** Non-canonical names servers use for the same formats. */
const ALIASES: Readonly<Record<string, ImageType>> = {
  "image/jpg": "image/jpeg",
  "image/pjpeg": "image/jpeg",
  "image/vnd.microsoft.icon": "image/x-icon",
  "image/x-bmp": "image/bmp",
  "image/x-ms-bmp": "image/bmp",
};

const startsWith = (
  bytes: Uint8Array,
  signature: readonly number[],
  offset = 0
): boolean =>
  bytes.length >= offset + signature.length &&
  signature.every((byte, i) => bytes[offset + i] === byte);

const ascii = (text: string): readonly number[] =>
  Array.from({ length: text.length }, (_, i) => text.codePointAt(i) ?? 0);

const PNG = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
const JPEG = [0xff, 0xd8, 0xff];
const GIF87 = ascii("GIF87a");
const GIF89 = ascii("GIF89a");
const RIFF = ascii("RIFF");
const WEBP = ascii("WEBP");
const FTYP = ascii("ftyp");
const ICO = [0x00, 0x00, 0x01, 0x00];
const BMP = ascii("BM");
const AVIF_BRANDS = [ascii("avif"), ascii("avis")];

/** AVIF: an ISO-BMFF `ftyp` box whose major or a compatible brand is avif/avis. */
const isAvif = (bytes: Uint8Array): boolean => {
  if (!startsWith(bytes, FTYP, 4)) {
    return false;
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const boxSize = Math.min(view.getUint32(0), bytes.length);
  // Major brand at 8, minor version at 12, compatible brands from 16.
  for (let offset = 8; offset + 4 <= boxSize; offset += offset === 8 ? 8 : 4) {
    if (AVIF_BRANDS.some((brand) => startsWith(bytes, brand, offset))) {
      return true;
    }
  }
  return false;
};

/** Detect the image format from its first bytes. `undefined` if not a supported image. */
export const sniffImageType = (bytes: Uint8Array): ImageType | undefined => {
  if (startsWith(bytes, PNG)) {
    return "image/png";
  }
  if (startsWith(bytes, JPEG)) {
    return "image/jpeg";
  }
  if (startsWith(bytes, GIF87) || startsWith(bytes, GIF89)) {
    return "image/gif";
  }
  if (startsWith(bytes, RIFF) && startsWith(bytes, WEBP, 8)) {
    return "image/webp";
  }
  if (isAvif(bytes)) {
    return "image/avif";
  }
  // ICO and BMP have short signatures; require a minimal header length.
  if (startsWith(bytes, ICO) && bytes.length >= 22) {
    return "image/x-icon";
  }
  if (startsWith(bytes, BMP) && bytes.length >= 26) {
    return "image/bmp";
  }
  return undefined;
};

/** Media type of a Content-Type header: lowercased, without parameters. */
export const mediaType = (header: string | undefined): string =>
  (header ?? "").split(";", 1)[0]?.trim().toLowerCase() ?? "";

/** Canonical allowed image type for a Content-Type header, or `undefined`. */
export const allowedImageType = (
  header: string | undefined
): ImageType | undefined => {
  const type = mediaType(header);
  const canonical = ALIASES[type] ?? type;
  return IMAGE_TYPES.find((allowed) => allowed === canonical);
};

export type ImageCheck =
  | { readonly ok: true; readonly type: ImageType }
  | { readonly ok: false; readonly reason: "content-type" };

/**
 * The declared type must be an allowed image AND match the magic bytes.
 * Returns the canonical type to send to the browser.
 */
export const checkImage = (
  contentType: string | undefined,
  bytes: Uint8Array
): ImageCheck => {
  const declared = allowedImageType(contentType);
  if (declared === undefined || sniffImageType(bytes) !== declared) {
    return { ok: false, reason: "content-type" };
  }
  return { ok: true, type: declared };
};
