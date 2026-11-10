// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Encrypts small JSON payloads for cookies with AES-256-GCM from WebCrypto,
// the only AEAD available in every runtime (Node, Bun, Workers).
//
// Format: `<key version>.<iv>.<ciphertext+tag>`, base64url.
// The additional data binds each value to its key version and purpose, so a
// login cookie can't be replayed as a session cookie.

const IV_BYTES = 12;
const KEY_BYTES = 32;

/** Parsed `SESSION_KEYS`. The first key encrypts; every key decrypts. */
export interface SealKeys {
  readonly current: number;
  readonly keys: ReadonlyMap<number, CryptoKey>;
}

export type Purpose = "login" | "session";

const encoder = new TextEncoder();
const decoder = new TextDecoder();

const toBase64Url = (bytes: Uint8Array): string =>
  btoa(String.fromCodePoint(...bytes))
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replace(/[=]+$/u, "");

const fromBase64Url = (text: string): Uint8Array<ArrayBuffer> | undefined => {
  if (!/^[\w-]*$/u.test(text)) {
    return undefined;
  }
  try {
    const binary = atob(text.replaceAll("-", "+").replaceAll("_", "/"));
    return Uint8Array.from(binary, (char) => char.codePointAt(0) ?? 0);
  } catch {
    return undefined;
  }
};

const additionalData = (
  purpose: Purpose,
  version: number
): Uint8Array<ArrayBuffer> =>
  encoder.encode(`nebulamail:${purpose}:v${version}`);

/**
 * Parses `"2:<base64url>,1:<base64url>"`. Throws on any malformed entry: a
 * bad key must stop the server from starting, not fail later per request.
 */
export const parseSealKeys = async (value: string): Promise<SealKeys> => {
  const entries = value.split(",").map((entry) => {
    const match = /^(?<version>\d{1,6}):(?<key>[\w-]+)$/u.exec(entry.trim());
    const version = Number(match?.groups?.version);
    const raw = fromBase64Url(match?.groups?.key ?? "");
    if (
      match === null ||
      !Number.isSafeInteger(version) ||
      raw?.length !== KEY_BYTES
    ) {
      throw new Error(
        `SESSION_KEYS: each entry must be "<version>:<${KEY_BYTES} bytes in base64url>"`
      );
    }
    return { raw, version };
  });
  const versions = entries.map((entry) => entry.version);
  if (new Set(versions).size !== versions.length) {
    throw new Error("SESSION_KEYS: a version appears twice");
  }
  const [first] = entries;
  if (first === undefined) {
    throw new Error("SESSION_KEYS is empty");
  }
  const keys = await Promise.all(
    entries.map(async ({ raw, version }) => {
      const key = await crypto.subtle.importKey("raw", raw, "AES-GCM", false, [
        "encrypt",
        "decrypt",
      ]);
      return [version, key] as const;
    })
  );
  return { current: first.version, keys: new Map(keys) };
};

export const seal = async (
  keys: SealKeys,
  purpose: Purpose,
  payload: unknown
): Promise<string> => {
  const key = keys.keys.get(keys.current);
  if (key === undefined) {
    throw new Error("Current session key missing");
  }
  const iv = crypto.getRandomValues(new Uint8Array(IV_BYTES));
  const ciphertext = await crypto.subtle.encrypt(
    {
      additionalData: additionalData(purpose, keys.current),
      iv,
      name: "AES-GCM",
    },
    key,
    encoder.encode(JSON.stringify(payload))
  );
  return `${keys.current}.${toBase64Url(iv)}.${toBase64Url(new Uint8Array(ciphertext))}`;
};

/**
 * Returns the decrypted JSON, or `undefined` if the value is malformed, was
 * made with an unknown key, or was tampered with. Callers must still validate
 * the shape and the timestamps.
 */
export const unseal = async (
  keys: SealKeys,
  purpose: Purpose,
  token: string
): Promise<unknown> => {
  const parts = token.split(".");
  if (parts.length !== 3) {
    return undefined;
  }
  const [versionText = "", ivText = "", dataText = ""] = parts;
  const version = /^\d{1,6}$/u.test(versionText)
    ? Number(versionText)
    : Number.NaN;
  const key = keys.keys.get(version);
  const iv = fromBase64Url(ivText);
  const data = fromBase64Url(dataText);
  if (key === undefined || iv?.length !== IV_BYTES || data === undefined) {
    return undefined;
  }
  try {
    const plain = await crypto.subtle.decrypt(
      { additionalData: additionalData(purpose, version), iv, name: "AES-GCM" },
      key,
      data
    );
    return JSON.parse(decoder.decode(plain)) as unknown;
  } catch {
    return undefined;
  }
};

/** Random URL-safe string, for `state` and the PKCE verifier. */
export const randomToken = (bytes = 32): string =>
  toBase64Url(crypto.getRandomValues(new Uint8Array(bytes)));

/** PKCE S256 challenge (RFC 7636). */
export const pkceChallenge = async (verifier: string): Promise<string> =>
  toBase64Url(
    new Uint8Array(
      await crypto.subtle.digest("SHA-256", encoder.encode(verifier))
    )
  );

/** Constant-time comparison of two strings of the same length. */
export const safeEqual = (left: string, right: string): boolean => {
  const a = encoder.encode(left);
  const b = encoder.encode(right);
  if (a.length !== b.length) {
    return false;
  }
  let diff = 0;
  for (const [index, byte] of a.entries()) {
    // oxlint-disable-next-line no-bitwise -- constant-time comparison needs bit operations.
    diff |= byte ^ (b[index] ?? 0);
  }
  return diff === 0;
};
