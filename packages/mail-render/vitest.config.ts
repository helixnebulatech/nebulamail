// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    projects: [
      {
        test: {
          // Default DOM for the pipeline tests; DOM-free modules (trackers)
          // opt into `node` per file to prove they do not need a DOM.
          environment: "jsdom",
          include: ["test/**/*.test.ts"],
          name: "mail-render",
        },
      },
      {
        test: {
          // Proton's own upstream tests (jest-style globals), run unmodified
          // against our DOMPurify install to check the vendored copy behaves
          // as upstream expects.
          environment: "jsdom",
          globals: true,
          include: ["vendor/proton-sanitize/src/**/*.test.ts"],
          name: "proton-upstream",
        },
      },
    ],
  },
});
