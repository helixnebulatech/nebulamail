// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Builds /sw.js from sw.ts in production builds only, so
// the dev server and its hot reload never get a service worker. The
// worker's cache allowlist is the exact list of hashed files of this
// client build; its version is a hash of that list and of the precached
// public files (the offline page and the icon), so a new icon or offline
// page replaces the old cache too.

import { readFile } from "node:fs/promises";
import path from "node:path";

import type { Plugin } from "vite";
import { transformWithOxc } from "vite";

import { JMAP_PROXY_SESSION_PATH } from "../lib/jmap-proxy.ts";

/** Public files the worker precaches (keep in sync with sw.ts). */
const PRECACHED = ["offline.html", "offline.css", "favicon.svg"] as const;

/** Only build output under assets/ (hashed names) may be cached. */
const STATIC_FILE = /^assets\/[\w.-]+\.(?:css|js|woff2|svg|png|webp)$/u;

const hash = async (text: string): Promise<string> => {
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(text)
  );
  return [...new Uint8Array(digest).slice(0, 8)]
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
};

/** The texts the worker shows (notifications), from messages/*.json. */
const WORKER_MESSAGES = [
  "cal_alert_generic",
  "cal_alert_snooze_10",
  "cal_alert_dismiss",
  "tasks_alert_generic",
  "tasks_alert_done",
  "tasks_reminder_due",
  "sw_mail_many",
  "sw_mail_new",
  "row_no_subject",
] as const;
const LOCALES = ["en", "es"] as const;

const workerMessages = async (
  root: string
): Promise<Record<string, Record<string, string>>> => {
  const entries = await Promise.all(
    LOCALES.map(async (locale) => {
      const all: unknown = JSON.parse(
        await readFile(path.join(root, "messages", `${locale}.json`), "utf-8")
      );
      const table: Record<string, string> = {};
      for (const key of WORKER_MESSAGES) {
        const value: unknown =
          typeof all === "object" && all !== null
            ? Reflect.get(all, key)
            : undefined;
        if (typeof value !== "string") {
          throw new TypeError(`messages/${locale}.json has no "${key}"`);
        }
        table[key] = value;
      }
      return [locale, table] as const;
    })
  );
  return Object.fromEntries(entries);
};

export interface ServiceWorkerOptions {
  /** Where the app lives: "/" or, in the static build, "/nebulamail/". */
  readonly base: string;
  /** The static build: no BFF to ask for tokens. */
  readonly static: boolean;
}

export const serviceWorker = (options: ServiceWorkerOptions): Plugin => {
  let root = "";
  return {
    apply: "build",
    configResolved: (config) => {
      ({ root } = config);
    },
    async generateBundle(_options, bundle) {
      if (this.environment.name !== "client") {
        return;
      }
      const assets = Object.keys(bundle)
        .filter((name) => STATIC_FILE.test(name))
        .toSorted()
        .map((name) => `${options.base}${name}`);
      const precached = await Promise.all(
        PRECACHED.map(
          async (name) =>
            await readFile(path.join(root, "public", name), "utf-8")
        )
      );
      const file = path.join(root, "src/pwa/sw.ts");
      const { code } = await transformWithOxc(
        await readFile(file, "utf-8"),
        file,
        { lang: "ts", target: "es2022" }
      );
      // The license line of the built file, not of this one.
      // REUSE-IgnoreStart
      const header = [
        "// Nebula Mail service worker, built from apps/web/src/pwa/sw.ts",
        "// SPDX-License-Identifier: AGPL-3.0-only",
        "",
      ].join("\n");
      // REUSE-IgnoreEnd
      const messages = await workerMessages(root);
      const source =
        header +
        code
          // The empty export only makes sw.ts a module for TypeScript.
          .replace(/^export \{\};?\s*$/mu, "")
          .replaceAll(/\bSW_MESSAGES\b/gu, JSON.stringify(messages))
          .replaceAll(/\bSW_ASSETS\b/gu, JSON.stringify(assets))
          .replaceAll(/\bSW_BASE\b/gu, JSON.stringify(options.base))
          .replaceAll(
            /\bSW_TOKEN_ENDPOINT\b/gu,
            // `background=1`: a push is not the user using the app.
            JSON.stringify(options.static ? null : "/auth/token?background=1")
          )
          .replaceAll(
            /\bSW_PROXY_SESSION\b/gu,
            JSON.stringify(options.static ? null : JMAP_PROXY_SESSION_PATH)
          )
          .replaceAll(
            /\bSW_VERSION\b/gu,
            JSON.stringify(await hash([...assets, ...precached].join("\n")))
          );
      if (/^\s*(?:import|export)\b/mu.test(source)) {
        throw new Error("sw.ts must not import or export anything");
      }
      this.emitFile({ fileName: "sw.js", source, type: "asset" });
    },
    name: "nebula:service-worker",
  };
};
