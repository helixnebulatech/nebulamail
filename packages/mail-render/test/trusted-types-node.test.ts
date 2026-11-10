// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// @vitest-environment node

// vite.config.ts (apps/web) loads `@nebulamail/mail-render/trusted-types`
// with Node itself (type stripping, no bundler) for the static build's
// CSP. Node doesn't resolve relative imports without an extension, so a
// local import there breaks the production build. Import it the same way.

import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const packageDir = fileURLToPath(new URL("..", import.meta.url));

describe("trusted-types under plain Node", () => {
  it("loads without a bundler", () => {
    const output = execFileSync(
      "node",
      [
        "--input-type=module",
        "-e",
        "const m = await import('@nebulamail/mail-render/trusted-types'); console.log(JSON.stringify(m.TRUSTED_TYPES_POLICIES));",
      ],
      { cwd: packageDir, encoding: "utf-8" }
    );
    expect(JSON.parse(output)).toContain("nebula-mail-srcdoc");
  });
});
