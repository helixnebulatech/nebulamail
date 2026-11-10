// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The throwaway Stalwart that serves the static zip: service
// `stalwart-static` of docker/compose.yaml, in its own compose project, so
// nothing here can touch the dev containers.

import path from "node:path";

export const ROOT = path.join(import.meta.dirname, "../../..");

/** Its port and public URL (docker/compose.yaml, `stalwart-static`). */
export const STALWART_URL = "http://localhost:8095";

/** Where the app is mounted (`urlPrefix`), as in docs/static.md. */
export const BASE_PATH = "/nebulamail";

/** The build's default `NEBULA_OAUTH_CLIENT_ID`. */
export const CLIENT_ID = "nebulamail";

export const DOMAIN = "nebulamail.test";
export const USER = `alice@${DOMAIN}`;

/** Restarts it (leaving bootstrap mode needs one). */
export const COMPOSE = [
  "compose",
  "-p",
  "nebula-static-test",
  "-f",
  path.join(ROOT, "docker/compose.yaml"),
  "--profile",
  "static",
] as const;

/**
 * Response headers the guide sets in `Http.responseHeaders` (docs/static.md,
 * «Response headers»); the test checks Stalwart sends them with the app.
 * No Content-Security-Policy: Stalwart replaces its own headers with these,
 * and would lose its login page's `frame-ancestors 'none'`.
 */
export const RESPONSE_HEADERS = {
  "Cross-Origin-Opener-Policy": "same-origin",
  "Permissions-Policy": "camera=(), microphone=(), geolocation=()",
  "Referrer-Policy": "no-referrer",
  "X-Content-Type-Options": "nosniff",
  "X-Frame-Options": "SAMEORIGIN",
} as const;

/** The seeded email (setup.ts). */
export const SUBJECT = "Static build check";
export const BODY_TEXT = "Served by Stalwart alone";
/** A remote image in the seeded email: never loaded (images blocked). */
export const REMOTE_IMAGE = "https://images.example.com/banner.png";

/** Set by setup.ts for the tests (a fresh random password each run). */
export const passwordEnv = "E2E_STATIC_ALICE_PASSWORD";
