// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { defineConfig } from "oxfmt";
import ultracite from "ultracite/oxfmt";

export default defineConfig({
  ...ultracite,
  ignorePatterns: [
    ...ultracite.ignorePatterns,
    "**/routeTree.gen.ts",
    // Agent worktrees: separate checkouts with their own config.
    ".claude/worktrees/**",
    "apps/web/src/env.ts",
    "apps/web/src/paraglide/**",
    "**/dist-static/**",
    "bridge/**",
    "LICENSES/**",
    "bun.lock",
    // Vendored code stays byte-identical to upstream; the hostile-email
    // fixtures are attack vectors that formatting would change.
    "packages/mail-render/vendor/**",
    "security/hostile-emails/*.html",
    "security/hostile-emails/*.json",
  ],
});
