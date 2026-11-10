// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// A first-come queue for the session's `maxConcurrentRequests` (RFC 8620
// §2): a client may not go over it, and a server may refuse what does.

import { JmapError } from "./errors";

export class Slots {
  #active = 0;
  readonly #waiting: ((value: undefined) => void)[] = [];

  /** Waits for a slot; `release` gives it back. */
  async acquire(limit: number, signal: AbortSignal | undefined): Promise<void> {
    if (this.#active < limit) {
      this.#active += 1;
      return;
    }
    const { promise, reject, resolve } = Promise.withResolvers<undefined>();
    const onAbort = (): void => {
      const place = this.#waiting.indexOf(resolve);
      // Gone if its turn has just come: the request itself then aborts.
      if (place !== -1) {
        this.#waiting.splice(place, 1);
      }
      reject(new JmapError({ kind: "aborted" }));
    };
    this.#waiting.push(resolve);
    signal?.addEventListener("abort", onAbort, { once: true });
    try {
      await promise;
    } finally {
      signal?.removeEventListener("abort", onAbort);
    }
  }

  /** The next in line takes the slot over. */
  release(): void {
    const wake = this.#waiting.shift();
    if (wake === undefined) {
      this.#active -= 1;
    } else {
      wake(undefined);
    }
  }
}
