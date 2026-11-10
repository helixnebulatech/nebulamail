// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Adapts a net-guard fetch (pinned on Node, DoH-checked on
// Workers) to the core's `FetchImage` contract. The core's per-email budget
// and tracker check on redirects are passed through to net-guard, which
// applies them on every hop.

import type { GuardedFetch } from "@nebulamail/net-guard";

import { cacheHeadersOf } from "./cache-policy";
import type { FetchImage } from "./config";

export const fetchImageWith =
  (guarded: GuardedFetch): FetchImage =>
  async (url, signal, options) => {
    const result = await guarded(url, {
      expect: "image",
      signal,
      ...(options === undefined
        ? {}
        : { allowRedirect: options.allowRedirect, budget: options.budget }),
    });
    if (!result.ok) {
      return { error: result.reason };
    }
    if (result.status < 200 || result.status > 299) {
      return { error: "upstream" };
    }
    if (result.imageType === undefined) {
      return { error: "content-type" };
    }
    return {
      bytes: result.body,
      // RFC 9111: the core decides from these whether to keep it.
      cacheHeaders: cacheHeadersOf(result.headers),
      type: result.imageType,
    };
  };
