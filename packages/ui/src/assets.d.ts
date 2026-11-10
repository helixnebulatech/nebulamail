// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Vite's `?url` imports (font-files.ts), typed without pulling vite/client
// into this package: the app that bundles it has those types.

declare module "*.woff2?url" {
  const url: string;
  export default url;
}
