// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { defineConfig } from "oxlint";
import core from "ultracite/oxlint/core";
import react from "ultracite/oxlint/react";
import tanstack from "ultracite/oxlint/tanstack";
import vitest from "ultracite/oxlint/vitest";

export default defineConfig({
  env: {
    ...core.env,
    builtin: true,
  },
  extends: [core, react, tanstack, vitest],
  ignorePatterns: [
    ...core.ignorePatterns,
    "**/routeTree.gen.ts",
    // Agent worktrees: separate checkouts with their own config.
    ".claude/worktrees/**",
    "apps/web/src/env.ts",
    "apps/web/src/paraglide/**",
    "**/dist/**",
    "**/dist-static/**",
    "bridge/**",
    // Vendored code stays byte-identical to upstream; the hostile-email
    // fixtures are attack vectors that formatting would change.
    "packages/mail-render/vendor/**",
    "security/hostile-emails/*.html",
    "security/hostile-emails/*.json",
  ],
  options: { typeAware: true },
  overrides: [
    {
      // Every dropdown is the Select of @nebulamail/ui, with the look of our
      // menus; the browser's own list doesn't match it.
      files: ["apps/web/src/**/*.tsx"],
      rules: {
        "react/forbid-elements": [
          "error",
          {
            forbid: [
              {
                element: "select",
                message: "Use Select from @nebulamail/ui.",
              },
            ],
          },
        ],
      },
    },
  ],
  rules: {
    // Security rules that must never be relaxed.
    "no-eval": "error",
    "no-implied-eval": "error",
    "no-new-func": "error",
    "react/no-danger": "error",
    "react/jsx-no-target-blank": "error",
    // TanStack Router redirects and answers 404 by throwing `redirect()` /
    // `notFound()` (plain objects, not Errors). Allow only those types.
    "typescript/only-throw-error": [
      "error",
      {
        allow: [
          {
            from: "package",
            name: "Redirect",
            package: "@tanstack/router-core",
          },
          {
            from: "package",
            name: "NotFoundError",
            package: "@tanstack/router-core",
          },
        ],
      },
    ],
  },
});
