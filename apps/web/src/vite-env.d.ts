// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

/// <reference types="vite/client" />

// Build-time constants (vite.config.ts `define`).
/**
 * Chrome's SharedWorker option (session/browser.ts): the worker outlives
 * its last page for a while. Not in TypeScript's DOM types yet.
 */
interface WorkerOptions {
  extendedLifetime?: boolean;
}

interface ImportMetaEnv {
  /** The static build served by Stalwart. */
  readonly NEBULA_STATIC: boolean;
  /** Static build: the public OAuth client registered in Stalwart. */
  readonly NEBULA_OAUTH_CLIENT_ID: string;
  /** Static build: the OAuth `scope` (NEBULA_OAUTH_SERVICES, lib/oauth-scope.ts). */
  readonly NEBULA_OAUTH_SCOPE: string;
  /**
   * Static build: the fixed CSP nonce of index.html, for `<style>` only
   * (scripts go by hash). New for every build.
   */
  readonly NEBULA_STYLE_NONCE: string;
  /** Static build: the image proxy's origin, or "" (none). */
  readonly NEBULA_IMAGE_PROXY_ORIGIN: string;
  /**
   * Static build: the hashed name of the zip's frame page for emails with
   * images straight from the sender ("email-frame.<hash>.html"); null in
   * the BFF build, which has no such page.
   */
  readonly NEBULA_FRAME_PAGE: string | null;
  /** Static build: the hashed name of the zip's unsubscribe page; else null. */
  readonly NEBULA_UNSUBSCRIBE_PAGE: string | null;
}
