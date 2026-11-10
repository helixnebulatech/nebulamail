// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// TEST-ONLY factory. Not listed in package.json `exports`, so other packages
// cannot import it, and `createGuardedFetchInternal` refuses a dial override
// unless running under Vitest.
//
// Why it is safe: the guard still runs unchanged on the URL and on the
// resolver's answers. `dial` only changes where the TCP connection for an
// ALREADY VALIDATED public address really goes, so tests can map e.g.
// 93.184.215.14:80 to a node:http server on 127.0.0.1. Tests also record the
// `address` argument, which proves which validated IP the adapter dialed.

import type { Dial } from "./connector";
import { createGuardedFetchInternal } from "./fetch";
import type { GuardedFetch, GuardedFetchOptions } from "./fetch";

export const createGuardedFetchForTests = (
  options: GuardedFetchOptions & { readonly dial: Dial }
): GuardedFetch => createGuardedFetchInternal(options, options.dial);
