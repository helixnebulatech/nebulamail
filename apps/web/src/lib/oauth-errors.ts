// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Which OAuth token errors end the session (RFC 6749 §5.2). Shared by the
// BFF (server/auth/handlers.ts) and the static version's worker
// (session/worker.ts), so both log out for the same reasons only.

/** The refresh token (or this client) is no longer valid. */
export const SESSION_ENDING_ERRORS: ReadonlySet<string> = new Set([
  "invalid_grant",
  "invalid_client",
  "unauthorized_client",
]);

/**
 * True when a token endpoint error means "log in again". Anything else
 * (`invalid_request`, a 429's `rate_limited`, a server error…) may pass.
 */
export const endsSession = (error: string): boolean =>
  SESSION_ENDING_ERRORS.has(error);
