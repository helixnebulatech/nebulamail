// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Base64 helpers using only `btoa`/`atob`, so they run in Node, Workers and
// browsers alike.

/** Characters of unpadded base64url. */
export const BASE64URL_PATTERN = /^[A-Za-z0-9_-]*$/u;

const CHUNK = 0x80_00;

/** Standard base64 with padding. */
export const toBase64 = (bytes: Uint8Array): string => {
  let binary = "";
  for (let i = 0; i < bytes.length; i += CHUNK) {
    binary += String.fromCodePoint(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(binary);
};

/** Unpadded base64url. */
export const toBase64Url = (bytes: Uint8Array): string =>
  toBase64(bytes)
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replace(/[=]+$/u, "");

/**
 * Strict unpadded base64url decoding. Returns `null` for anything that is
 * not the canonical encoding of some byte string (wrong alphabet, padding,
 * impossible length, non-zero trailing bits), so every byte string has
 * exactly one accepted encoding.
 */
export const fromBase64Url = (text: string): Uint8Array<ArrayBuffer> | null => {
  if (!BASE64URL_PATTERN.test(text) || text.length % 4 === 1) {
    return null;
  }
  const standard = text.replaceAll("-", "+").replaceAll("_", "/");
  let binary: string;
  try {
    binary = atob(standard + "=".repeat((4 - (standard.length % 4)) % 4));
  } catch {
    return null;
  }
  const bytes = Uint8Array.from(binary, (c) => c.codePointAt(0) ?? 0);
  return toBase64Url(bytes) === text ? bytes : null;
};
