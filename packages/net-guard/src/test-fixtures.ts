// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Test fixtures shared by the core and Node adapter tests.

/** Resolve on a later microtask, like a real resolver. */
export const later = async <T>(value: T): Promise<T> => {
  await Promise.resolve();
  return value;
};

/** Reject on a later microtask, like a failing resolver. */
export const fail = async (message = "ENOTFOUND"): Promise<never> => {
  await Promise.resolve();
  throw new Error(message);
};

export const bytes = (...parts: (string | readonly number[])[]) =>
  Uint8Array.from(
    parts.flatMap((p) =>
      typeof p === "string"
        ? Array.from({ length: p.length }, (_, i) => p.codePointAt(i) ?? 0)
        : [...p]
    )
  );
const pad = (n: number) => Array.from({ length: n }, () => 0);

/** Minimal headers of each supported image format. */
export const SAMPLES = {
  avif: bytes([0, 0, 0, 0x1c], "ftypavif", [0, 0, 0, 0], "avifmif1miaf"),
  avifCompatible: bytes([0, 0, 0, 0x18], "ftypmif1", [0, 0, 0, 0], "miafavif"),
  bmp: bytes("BM", pad(30)),
  gif: bytes("GIF89a", pad(10)),
  ico: bytes([0, 0, 1, 0, 1, 0], pad(20)),
  jpeg: bytes([0xff, 0xd8, 0xff, 0xe0], pad(10)),
  png: bytes([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a], pad(10)),
  webp: bytes("RIFF", [0, 0, 0, 0], "WEBPVP8 ", pad(4)),
} as const;
