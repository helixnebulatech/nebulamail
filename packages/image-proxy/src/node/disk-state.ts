// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Preload state on disk: `<dir>/<kind>/<hex>.json`, written
// to a temp file and renamed (atomic), `0600` files in `0700` folders. The
// names are the record keys (hashes only). Kept in its own folder, which
// the image sweep and the size cap of `disk-storage.ts` never touch.

import { randomUUID } from "node:crypto";
import {
  mkdir,
  readdir,
  readFile,
  rename,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import path from "node:path";

import type { PreloadStateStore } from "../preload/state";
import { checkStateKey, MAX_STATE_BYTES, stateKeyOf } from "../preload/state";

const isMissing = (error: unknown): boolean =>
  typeof error === "object" &&
  error !== null &&
  "code" in error &&
  error.code === "ENOENT";

export const createDiskStateStore = (dir: string): PreloadStateStore => {
  const location = (key: string): { folder: string; file: string } => {
    const [kind = "", hex = ""] = checkStateKey(key).split(":");
    const folder = path.join(dir, kind);
    return { file: path.join(folder, `${hex}.json`), folder };
  };
  return {
    delete: async (key) => {
      await rm(location(key).file, { force: true });
    },
    get: async (key) => {
      const { file } = location(key);
      try {
        const info = await stat(file);
        if (info.size > MAX_STATE_BYTES) {
          return null;
        }
        return await readFile(file, "utf-8");
      } catch (error) {
        if (isMissing(error)) {
          return null;
        }
        throw error;
      }
    },
    list: async function* list(kind) {
      let names: string[];
      try {
        names = await readdir(path.join(dir, kind));
      } catch (error) {
        if (isMissing(error)) {
          return;
        }
        throw error;
      }
      for (const name of names) {
        // Temp files of an interrupted write (`<hex>.json.<uuid>.tmp`) and
        // anything else in the folder are not keys.
        const key = stateKeyOf(kind, name, ".json");
        if (key !== undefined) {
          yield key;
        }
      }
    },
    put: async (key, value) => {
      const data = Buffer.from(value, "utf-8");
      if (data.length > MAX_STATE_BYTES) {
        throw new RangeError("image proxy: state record too large");
      }
      const { file, folder } = location(key);
      await mkdir(folder, { mode: 0o700, recursive: true });
      const temp = `${file}.${randomUUID()}.tmp`;
      try {
        await writeFile(temp, data, { flag: "wx", mode: 0o600 });
        await rename(temp, file);
      } catch (error) {
        await rm(temp, { force: true });
        throw error;
      }
    },
  };
};
