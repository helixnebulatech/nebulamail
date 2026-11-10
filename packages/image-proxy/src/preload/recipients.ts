// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Recipients of preloaded mail: the per-recipient quota and
// the opt-out list are keyed by a keyed hash of the address, never by the
// address itself.
//
// - Normalized first: trimmed, `<>` removed, lower case, and the `+tag` of
//   the local part dropped (Stalwart delivers `alice+news@x` to `alice@x`),
//   so an opt-out for `alice@x` also covers `alice+news@x`. Over-matching
//   is the safe direction: at worst two addresses share a quota.
// - Hashed with HMAC-SHA256 under a key that never sits next to the hashes
//   (it comes from `IMAGE_PRELOAD_INDEX_KEY` or `IMAGE_PRELOAD_SECRET`), so
//   a leaked bucket does not let anyone confirm a guessed address.

const encoder = new TextEncoder();

/** Longest address we consider (RFC 5321: 64 + 1 + 255). */
const MAX_ADDRESS_LENGTH = 320;
const DOMAIN = "nebulamail/image-proxy/recipient-key\0";

/** `alice+tag@Example.org` → `alice@example.org`; null if not an address. */
export const normalizeAddress = (address: string): string | null => {
  const bare = address.trim().replace(/^<(?<inner>.*)>$/u, "$<inner>");
  if (bare.length === 0 || bare.length > MAX_ADDRESS_LENGTH) {
    return null;
  }
  const at = bare.lastIndexOf("@");
  if (at <= 0 || at === bare.length - 1) {
    return null;
  }
  const local = bare.slice(0, at);
  const domain = bare.slice(at + 1).toLowerCase();
  // No spaces, controls or a second `@` (quoted local parts are refused):
  // a refused address is never preloaded for, which is the safe side.
  // oxlint-disable-next-line no-control-regex -- refusing control characters is the point.
  if (/[\s\u0000-\u001F\u007F]/u.test(bare) || bare.indexOf("@") !== at) {
    return null;
  }
  const plus = local.indexOf("+");
  const base = (plus > 0 ? local.slice(0, plus) : local).toLowerCase();
  return `${base}@${domain}`;
};

/** Keyed hash of a normalized address: 64 hex characters. */
export type RecipientHasher = (normalized: string) => Promise<string>;

export const createRecipientHasher = (secret: string): RecipientHasher => {
  const key = (async () => {
    const material = await crypto.subtle.digest(
      "SHA-256",
      encoder.encode(`${DOMAIN}${secret}`)
    );
    return await crypto.subtle.importKey(
      "raw",
      material,
      { hash: "SHA-256", name: "HMAC" },
      false,
      ["sign"]
    );
  })();
  return async (normalized) => {
    const mac = await crypto.subtle.sign(
      "HMAC",
      await key,
      encoder.encode(normalized)
    );
    return Array.from(new Uint8Array(mac), (b) =>
      b.toString(16).padStart(2, "0")
    ).join("");
  };
};

/** Normalized, deduplicated hashes of `addresses` (invalid ones dropped). */
export const hashRecipients = async (
  hasher: RecipientHasher,
  addresses: readonly string[]
): Promise<string[]> => {
  const normalized = new Set<string>();
  for (const address of addresses) {
    const value = normalizeAddress(address);
    if (value !== null) {
      normalized.add(value);
    }
  }
  return await Promise.all([...normalized].map(hasher));
};
