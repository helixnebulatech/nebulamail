// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// TEST-ONLY factory. Not listed in package.json `exports`, so other packages
// cannot import it, and `createBunFetchInternal` refuses the hooks unless
// running under Vitest.
//
// Why it is safe: the guard still runs unchanged on the URL and on the
// resolver's answers. `dial` only changes where the connection for an
// ALREADY VALIDATED public address really goes (a local server), and `ca`
// adds a test CA so local TLS servers can prove a name. `Host`, the SNI and
// the name the certificate is checked against stay the original hostname.

import type { GuardedFetch } from "../fetch-types";
import type { GuardedFetchOptions } from "../http/hops";
import { createBunFetchInternal } from "./fetch";
import type { BunTestHooks } from "./fetch";

export const createBunFetchForTests = (
  options: GuardedFetchOptions & BunTestHooks
): GuardedFetch =>
  createBunFetchInternal(options, {
    dial: options.dial,
    ...(options.ca === undefined ? {} : { ca: options.ca }),
  });
