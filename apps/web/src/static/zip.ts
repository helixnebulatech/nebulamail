// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// A minimal, reproducible zip writer (deflate, no extras) for the static
// build: the same files always give the same bytes, so the published
// SHA-256 can be checked by rebuilding. Deflate comes from
// fflate, pinned in package.json: node:zlib gives other bytes in Node and
// in Bun (and may change between versions).

import { deflateSync } from "fflate";

export interface ZipEntry {
  /** Path inside the archive, with `/` separators. */
  readonly name: string;
  readonly data: Uint8Array;
}

const CRC_TABLE = Array.from({ length: 256 }, (_, index) => {
  let crc = index;
  for (let bit = 0; bit < 8; bit += 1) {
    // oxlint-disable-next-line no-bitwise -- CRC-32.
    crc = crc & 1 ? 0xed_b8_83_20 ^ (crc >>> 1) : crc >>> 1;
  }
  // oxlint-disable-next-line no-bitwise -- CRC-32.
  return crc >>> 0;
});

export const crc32 = (data: Uint8Array): number => {
  let crc = 0xff_ff_ff_ff;
  for (const byte of data) {
    // oxlint-disable-next-line no-bitwise -- CRC-32.
    crc = (CRC_TABLE[(crc ^ byte) & 0xff] ?? 0) ^ (crc >>> 8);
  }
  // oxlint-disable-next-line no-bitwise -- CRC-32.
  return (crc ^ 0xff_ff_ff_ff) >>> 0;
};

/** 1980-01-01 00:00 in MS-DOS format: a fixed date, for reproducible zips. */
const DOS_TIME = 0;
const DOS_DATE = 0x21;
const UTF8_FLAG = 0x08_00;

/** Segments of safe characters, never "." or "..", no empty ones. */
const SAFE_NAME =
  /^(?!\.{1,2}(?:\/|$))[\w.@+-]+(?:\/(?!\.{1,2}(?:\/|$))[\w.@+-]+)*$/u;

export const createZip = (entries: readonly ZipEntry[]): Uint8Array => {
  const sorted = entries.toSorted((a, b) => (a.name < b.name ? -1 : 1));
  for (const [i, entry] of sorted.entries()) {
    if (sorted[i + 1]?.name === entry.name) {
      throw new Error(`zip: ${entry.name} twice`);
    }
  }
  const locals: Uint8Array[] = [];
  const centrals: Uint8Array[] = [];
  let offset = 0;
  for (const entry of sorted) {
    if (!SAFE_NAME.test(entry.name)) {
      throw new Error(`zip: unsafe file name ${entry.name}`);
    }
    const name = new TextEncoder().encode(entry.name);
    const compressed = deflateSync(entry.data, { level: 9 });
    const crc = crc32(entry.data);
    const local = new Uint8Array(30 + name.length);
    const view = new DataView(local.buffer);
    view.setUint32(0, 0x04_03_4b_50, true);
    view.setUint16(4, 20, true);
    view.setUint16(6, UTF8_FLAG, true);
    view.setUint16(8, 8, true);
    view.setUint16(10, DOS_TIME, true);
    view.setUint16(12, DOS_DATE, true);
    view.setUint32(14, crc, true);
    view.setUint32(18, compressed.length, true);
    view.setUint32(22, entry.data.length, true);
    view.setUint16(26, name.length, true);
    view.setUint16(28, 0, true);
    local.set(name, 30);

    const central = new Uint8Array(46 + name.length);
    const cview = new DataView(central.buffer);
    cview.setUint32(0, 0x02_01_4b_50, true);
    cview.setUint16(4, 20, true);
    cview.setUint16(6, 20, true);
    cview.setUint16(8, UTF8_FLAG, true);
    cview.setUint16(10, 8, true);
    cview.setUint16(12, DOS_TIME, true);
    cview.setUint16(14, DOS_DATE, true);
    cview.setUint32(16, crc, true);
    cview.setUint32(20, compressed.length, true);
    cview.setUint32(24, entry.data.length, true);
    cview.setUint16(28, name.length, true);
    cview.setUint32(42, offset, true);
    central.set(name, 46);

    locals.push(local, compressed);
    centrals.push(central);
    offset += local.length + compressed.length;
  }
  const centralSize = centrals.reduce((sum, part) => sum + part.length, 0);
  const end = new Uint8Array(22);
  const eview = new DataView(end.buffer);
  eview.setUint32(0, 0x06_05_4b_50, true);
  eview.setUint16(8, sorted.length, true);
  eview.setUint16(10, sorted.length, true);
  eview.setUint32(12, centralSize, true);
  eview.setUint32(16, offset, true);
  const parts = [...locals, ...centrals, end];
  const total = parts.reduce((sum, part) => sum + part.length, 0);
  const out = new Uint8Array(total);
  let position = 0;
  for (const part of parts) {
    out.set(part, position);
    position += part.length;
  }
  return out;
};
