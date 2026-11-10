// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// How long an access token lives when the token response doesn't say.
// RFC 6749 §5.1: `expires_in` is RECOMMENDED, not required ("If omitted,
// the authorization server SHOULD provide the expiration time via other
// means or document the default value"). Stalwart and our bridge always
// send it; another server might not. We can't know its lifetime, so we
// assume a short one and refresh early: 5 minutes. With the refresh
// margins (BFF 2 min, browser 1 min) the token is renewed after 3-4
// minutes; if the server's tokens really live longer, that only costs an
// early refresh, and if they live shorter, the 401 retry still renews it.

import { z } from "zod";

export const DEFAULT_EXPIRES_IN_S = 300;

/** `expires_in` of a token response: a positive integer, or absent. */
export const ExpiresIn = z.number().int().positive().optional();

/** When a token issued at `now` (ms) expires, in ms. */
export const tokenExpiresAt = (
  now: number,
  expiresIn: number | undefined
): number => now + (expiresIn ?? DEFAULT_EXPIRES_IN_S) * 1000;
