// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Tests don't load the TanStack Start or StyleX plugins from vite.config.ts:
// they start dev servers that keep the test process alive. Paraglide does
// load: it only compiles the messages (src/paraglide, not committed), so a
// test run never depends on an earlier build having made them.

import { paraglideVitePlugin } from "@inlang/paraglide-js";
import type { Plugin } from "vitest/config";
import { defineConfig } from "vitest/config";

import { FRAME_PAGE } from "./src/lib/frame-page.ts";

/** The BFF build's values (vite.config.ts `define`). */
const BFF_ENV: Readonly<Record<string, string>> = {
  NEBULA_FRAME_PAGE: JSON.stringify(FRAME_PAGE),
  NEBULA_IMAGE_PROXY_ORIGIN: JSON.stringify(""),
  NEBULA_OAUTH_CLIENT_ID: JSON.stringify("nebulamail"),
  NEBULA_OAUTH_SCOPE: JSON.stringify(""),
  NEBULA_STATIC: "false",
  NEBULA_STYLE_NONCE: JSON.stringify(""),
  NEBULA_UNSUBSCRIBE_PAGE: "null",
};

// Not inside a string (`"import.meta.env.NEBULA_…": …` keys of a define
// table, src/static/plugin.ts), as Vite's `define` doesn't either.
const ENV_REFERENCE =
  /(?<!["'`])\bimport\.meta\.env\.(?<name>NEBULA_[A-Z_]+)\b/gu;

/**
 * Replaces `import.meta.env.NEBULA_*` in the source, as Vite's `define`
 * does in a build. Vitest's own `define` for `import.meta.env.*` sets them
 * at run time as strings: `NEBULA_STATIC` was `"false"` (truthy, so the
 * tests ran the static build's branches) and `NEBULA_UNSUBSCRIBE_PAGE`
 * was `"null"`. An unknown name fails the test run.
 */
const defineBffEnv = (): Plugin => ({
  enforce: "pre",
  name: "nebula-bff-env",
  transform(code, id) {
    if (
      id.includes("/node_modules/") ||
      !code.includes("import.meta.env.NEBULA_")
    ) {
      return null;
    }
    return {
      code: code.replaceAll(ENV_REFERENCE, (_, name: string) => {
        const value = BFF_ENV[name];
        if (value === undefined) {
          throw new Error(`vitest.config.ts: no test value for ${name}`);
        }
        return `(${value})`;
      }),
      map: null,
    };
  },
});

export default defineConfig({
  // Same options as vite.config.ts.
  plugins: [
    defineBffEnv(),
    paraglideVitePlugin({
      cookieName: "wm-locale",
      outdir: "./src/paraglide",
      outputStructure: "message-modules",
      project: "./project.inlang",
      strategy: ["cookie", "preferredLanguage", "baseLocale"],
    }),
  ],
  resolve: { tsconfigPaths: true },
  test: { environment: "node", include: ["src/**/*.test.{ts,tsx}"] },
});
