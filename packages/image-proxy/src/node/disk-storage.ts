// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// File-system `ImageStorage` for Node/Docker.
//
// - File names are `sha256(key)`, never derived from a URL, so nothing about
//   the user's mail is visible in a directory listing.
// - Writes are atomic: a unique temp file in the same directory, then
//   `rename`. Readers see the old file or the new one, never half of one.
// - Each file starts with a small header (type, expiry), so the TTL does not
//   depend on mtime. Expired entries are deleted on read and by `sweep()`.
// - Optional size cap (the Node server always sets one): when exceeded,
//   files are deleted until usage is under 90 % of the cap, fairly:
//   1. from the account that uses the most bytes, its oldest first (a user
//      who fills the cache mostly evicts themself);
//   2. only when no account's cache is left, the shared entries (preloaded
//      images and email token records, `*preload` / `*mail`), oldest first.
// - The header names the owner as a keyed hash of the account hash
//   (HMAC under `owner.key`, a random key of this directory), so files of
//   one account cannot be grouped by anyone who only reads the directory.
// - An in-memory index (owner, size, mtime) is built once from the headers
//   and kept up to date, so an eviction never reopens every file.

import { createHash, createHmac, randomBytes, randomUUID } from "node:crypto";
import {
  mkdir,
  open,
  readdir,
  readFile,
  rename,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import path from "node:path";

import type { ImageStorage, StoredImage } from "../config";
import { PRELOAD_ACCOUNT } from "../preload/hook";
import { RECORD_ACCOUNT } from "../preload/records";
import { storedType } from "../stored-type";

export interface DiskStorageOptions {
  /** Total bytes on disk before eviction (see above). Default: no cap. */
  readonly maxBytes?: number;
  readonly now?: () => number;
  /** Called when the cap had to evict shared (preloaded) entries. */
  readonly onSharedEvicted?: (files: number) => void;
}

export interface DiskStorage extends ImageStorage {
  /** Delete expired entries and stale temp files. Returns how many files were removed. */
  readonly sweep: () => Promise<number>;
  /** Bytes currently used by finished entries. */
  readonly usage: () => Promise<number>;
}

const MAGIC = Buffer.from("WMIMG1\n", "latin1");
const HEADER_LIMIT = 256;
const FILE_NAME = /^[0-9a-f]{64}\.img$/u;
const TEMP_NAME = /^[0-9a-f]{64}\.[0-9a-f-]{36}\.tmp$/u;
const SHARD = /^[0-9a-f]{2}$/u;
/** Temp files older than this are leftovers of a crash. */
const STALE_TEMP_MS = 60 * 60 * 1000;

/** Who an entry counts for when the cap evicts. */
interface Owner {
  /** Keyed hash of the account hash; "legacy" for files without one. */
  readonly owner: string;
  /** Preloaded images and email records: evicted only after the rest. */
  readonly shared: boolean;
}

interface Header extends Owner {
  readonly type: string;
  readonly expiresAt: number;
}

const OWNER = /^[0-9a-f]{16}$/u;
const LEGACY: Owner = { owner: "legacy", shared: false };
const KEY_FILE = "owner.key";

const sha256Hex = (text: string): string =>
  createHash("sha256").update(text, "utf-8").digest("hex");

/** Account hashes of the shared entries (`storageKey("*preload", …)`…). */
const SHARED_ACCOUNTS = new Set(
  [PRELOAD_ACCOUNT, RECORD_ACCOUNT].map(sha256Hex)
);

const encode = (
  image: StoredImage,
  expiresAt: number,
  owner: Owner
): Buffer => {
  const header = Buffer.from(
    JSON.stringify({
      expiresAt,
      owner: owner.owner,
      shared: owner.shared,
      type: image.type,
    } satisfies Header),
    "utf-8"
  );
  const length = Buffer.alloc(2);
  length.writeUInt16BE(header.length);
  return Buffer.concat([MAGIC, length, header, image.bytes]);
};

const HEADER_BYTES = MAGIC.length + 2 + HEADER_LIMIT;

/** The header of a file (its first `HEADER_BYTES` are enough), or null. */
const decodeHeader = (
  file: Buffer
): (Header & { readonly bodyStart: number }) | null => {
  if (
    file.length < MAGIC.length + 2 ||
    !file.subarray(0, MAGIC.length).equals(MAGIC)
  ) {
    return null;
  }
  const length = file.readUInt16BE(MAGIC.length);
  const start = MAGIC.length + 2;
  if (length > HEADER_LIMIT || file.length < start + length) {
    return null;
  }
  let header: unknown;
  try {
    header = JSON.parse(file.subarray(start, start + length).toString("utf-8"));
  } catch {
    return null;
  }
  if (
    typeof header !== "object" ||
    header === null ||
    !("type" in header) ||
    !("expiresAt" in header) ||
    typeof header.type !== "string" ||
    typeof header.expiresAt !== "number" ||
    storedType(header.type) !== header.type
  ) {
    return null;
  }
  // Files written before the owner existed count as one "legacy" account.
  const owner =
    "owner" in header &&
    typeof header.owner === "string" &&
    OWNER.test(header.owner) &&
    "shared" in header &&
    typeof header.shared === "boolean"
      ? { owner: header.owner, shared: header.shared }
      : LEGACY;
  return {
    ...owner,
    bodyStart: start + length,
    expiresAt: header.expiresAt,
    type: header.type,
  };
};

const decode = (file: Buffer): (Header & { bytes: Uint8Array }) | null => {
  const header = decodeHeader(file);
  return header === null
    ? null
    : {
        bytes: new Uint8Array(file.subarray(header.bodyStart)),
        expiresAt: header.expiresAt,
        owner: header.owner,
        shared: header.shared,
        type: header.type,
      };
};

/** The first bytes of a file: enough for its header. */
const readHead = async (filePath: string): Promise<Buffer | null> => {
  let handle;
  try {
    handle = await open(filePath, "r");
    const buffer = Buffer.alloc(HEADER_BYTES);
    const { bytesRead } = await handle.read(buffer, 0, HEADER_BYTES, 0);
    return buffer.subarray(0, bytesRead);
  } catch {
    return null;
  } finally {
    await handle?.close();
  }
};

const isMissing = (error: unknown): boolean =>
  typeof error === "object" &&
  error !== null &&
  "code" in error &&
  error.code === "ENOENT";

interface Entry {
  readonly filePath: string;
  readonly size: number;
  readonly mtimeMs: number;
  readonly temp: boolean;
}

const listEntries = async (dir: string): Promise<Entry[]> => {
  let shards: string[];
  try {
    shards = await readdir(dir);
  } catch (error) {
    if (isMissing(error)) {
      return [];
    }
    throw error;
  }
  const nested = await Promise.all(
    shards
      .filter((shard) => SHARD.test(shard))
      .map(async (shard) => {
        const names = await readdir(path.join(dir, shard)).catch(() => []);
        const entries = await Promise.all(
          names
            .filter((name) => FILE_NAME.test(name) || TEMP_NAME.test(name))
            .map(async (name) => {
              const filePath = path.join(dir, shard, name);
              const info = await stat(filePath).catch(() => null);
              return info === null
                ? null
                : {
                    mtimeMs: info.mtimeMs,
                    filePath,
                    size: info.size,
                    temp: TEMP_NAME.test(name),
                  };
            })
        );
        return entries.filter((entry) => entry !== null);
      })
  );
  return nested.flat();
};

interface OwnerGroup {
  bytes: number;
  /** Oldest first. */
  readonly files: string[];
}

interface Indexed extends Owner {
  readonly size: number;
  readonly mtimeMs: number;
}

/** The directory's key for owner hashes, created (0600) on first use. */
const ownerKey = async (dir: string): Promise<Buffer> => {
  const file = path.join(dir, KEY_FILE);
  await mkdir(dir, { mode: 0o700, recursive: true });
  try {
    await writeFile(file, randomBytes(32).toString("hex"), {
      flag: "wx",
      mode: 0o600,
    });
  } catch (error) {
    if (
      !(typeof error === "object" && error !== null && "code" in error) ||
      error.code !== "EEXIST"
    ) {
      throw error;
    }
  }
  const raw = await readFile(file, "utf-8");
  const text = raw.trim();
  if (!/^[0-9a-f]{64}$/u.test(text)) {
    throw new Error("disk storage: unreadable owner key");
  }
  return Buffer.from(text, "hex");
};

/** The account with the most bytes left, and the next one's bytes. */
const biggest = (
  groups: Iterable<OwnerGroup>
): { top: OwnerGroup | undefined; second: number } => {
  let top: OwnerGroup | undefined;
  let second = 0;
  for (const group of groups) {
    if (group.files.length > 0) {
      if (top === undefined || group.bytes > top.bytes) {
        second = Math.max(second, top?.bytes ?? 0);
        top = group;
      } else {
        second = Math.max(second, group.bytes);
      }
    }
  }
  return { second, top };
};

const KEY = /^v1:(?<account>[0-9a-f]{64}):[0-9a-f]{64}$/u;

export const createDiskStorage = (
  dir: string,
  options: DiskStorageOptions = {}
): DiskStorage => {
  const now = options.now ?? Date.now;
  const { maxBytes } = options;
  if (
    maxBytes !== undefined &&
    (!Number.isSafeInteger(maxBytes) || maxBytes <= 0)
  ) {
    throw new Error("disk storage: maxBytes must be a positive integer");
  }
  let key: Promise<Buffer> | undefined;
  /** File path → owner, size, mtime of every finished entry. */
  let index: Promise<Map<string, Indexed>> | undefined;
  let total = 0;
  let evicting: Promise<void> | undefined;

  const location = (
    storageKey: string
  ): { shard: string; filePath: string } => {
    const name = sha256Hex(storageKey);
    const shard = path.join(dir, name.slice(0, 2));
    return { filePath: path.join(shard, `${name}.img`), shard };
  };

  const ownerOf = async (storageKey: string): Promise<Owner> => {
    const account = KEY.exec(storageKey)?.groups?.account;
    if (account === undefined) {
      return LEGACY;
    }
    key ??= ownerKey(dir);
    const owner = createHmac("sha256", await key)
      .update(account, "utf-8")
      .digest("hex")
      .slice(0, 16);
    return { owner, shared: SHARED_ACCOUNTS.has(account) };
  };

  /** Built once from the files' headers (one small read each). */
  const loadIndex = async (): Promise<Map<string, Indexed>> => {
    const map = new Map<string, Indexed>();
    total = 0;
    for (const entry of await listEntries(dir)) {
      if (!entry.temp) {
        // One at a time: this should not flood the disk.
        // oxlint-disable-next-line no-await-in-loop
        const head = await readHead(entry.filePath);
        const header = head === null ? null : decodeHeader(head);
        const owner = header ?? LEGACY;
        map.set(entry.filePath, {
          mtimeMs: entry.mtimeMs,
          owner: owner.owner,
          shared: owner.shared,
          size: entry.size,
        });
        total += entry.size;
      }
    }
    return map;
  };

  const entries = async (): Promise<Map<string, Indexed>> => {
    index ??= loadIndex();
    return await index;
  };

  const forget = (map: Map<string, Indexed>, filePath: string): void => {
    const old = map.get(filePath);
    if (old !== undefined) {
      map.delete(filePath);
      total -= old.size;
    }
  };

  const remove = async (filePath: string): Promise<void> => {
    await rm(filePath, { force: true });
    if (index !== undefined) {
      forget(await index, filePath);
    }
  };

  /** Down to 90 % of `cap`: the biggest account first, shared entries last. */
  const evict = async (cap: number): Promise<void> => {
    const map = await entries();
    const target = Math.floor(cap * 0.9);
    const over = (): boolean => total > target;
    const byOwner = new Map<string, OwnerGroup>();
    const shared: string[] = [];
    for (const [filePath, entry] of [...map].toSorted(
      ([, a], [, b]) => a.mtimeMs - b.mtimeMs
    )) {
      if (entry.shared) {
        shared.push(filePath);
      } else {
        const group = byOwner.get(entry.owner) ?? { bytes: 0, files: [] };
        group.bytes += entry.size;
        group.files.push(filePath);
        byOwner.set(entry.owner, group);
      }
    }
    let { top, second } = biggest(byOwner.values());
    while (over() && top !== undefined) {
      // Its oldest file; the next round may pick another account.
      const filePath = top.files.shift();
      if (filePath !== undefined) {
        top.bytes -= map.get(filePath)?.size ?? 0;
        // oxlint-disable-next-line no-await-in-loop
        await remove(filePath);
      }
      if (top.bytes < second || top.files.length === 0) {
        ({ top, second } = biggest(byOwner.values()));
      }
    }
    let sharedEvicted = 0;
    for (const filePath of shared) {
      if (!over()) {
        break;
      }
      // oxlint-disable-next-line no-await-in-loop
      await remove(filePath);
      sharedEvicted += 1;
    }
    if (sharedEvicted > 0) {
      options.onSharedEvicted?.(sharedEvicted);
    }
  };

  return {
    delete: async (storageKey) => {
      await remove(location(storageKey).filePath);
    },

    get: async (storageKey) => {
      const { filePath } = location(storageKey);
      let file: Buffer;
      try {
        file = await readFile(filePath);
      } catch (error) {
        if (isMissing(error)) {
          return null;
        }
        throw error;
      }
      const entry = decode(file);
      if (entry === null || entry.expiresAt <= now()) {
        await remove(filePath);
        return null;
      }
      return {
        bytes: entry.bytes,
        expiresAt: entry.expiresAt,
        type: entry.type,
      };
    },

    put: async (storageKey, image, ttlMs) => {
      const { filePath, shard } = location(storageKey);
      await mkdir(shard, { mode: 0o700, recursive: true });
      const owner = await ownerOf(storageKey);
      const data = encode(image, now() + ttlMs, owner);
      const temp = `${filePath.slice(0, -".img".length)}.${randomUUID()}.tmp`;
      try {
        await writeFile(temp, data, { flag: "wx", mode: 0o600 });
        await rename(temp, filePath);
      } catch (error) {
        await rm(temp, { force: true });
        throw error;
      }
      if (maxBytes === undefined && index === undefined) {
        return;
      }
      const map = await entries();
      forget(map, filePath);
      map.set(filePath, { ...owner, mtimeMs: now(), size: data.length });
      total += data.length;
      if (
        maxBytes !== undefined &&
        total > maxBytes &&
        evicting === undefined
      ) {
        evicting = evict(maxBytes).finally(() => {
          evicting = undefined;
        });
        await evicting;
      }
    },

    sweep: async () => {
      let removed = 0;
      const t = now();
      for (const entry of await listEntries(dir)) {
        let expired = entry.temp && t - entry.mtimeMs > STALE_TEMP_MS;
        if (!entry.temp) {
          // Sequential on purpose: a sweep should not flood the disk.
          // oxlint-disable-next-line no-await-in-loop
          const file = await readFile(entry.filePath).catch(() => null);
          const decoded = file === null ? null : decode(file);
          expired = decoded === null || decoded.expiresAt <= t;
        }
        if (expired) {
          // oxlint-disable-next-line no-await-in-loop
          await remove(entry.filePath);
          removed += 1;
        }
      }
      return removed;
    },

    usage: async () => {
      await entries();
      return total;
    },
  };
};
