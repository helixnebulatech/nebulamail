// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// How many long-lived proxy streams (push, downloads, uploads) may be open
// at once: per session and in the whole process. Without a cap, a script on
// the page (or a loop) could open thousands and use up this server's
// sockets and the JMAP server's connections.

/** Per session: a few tabs with push, and parallel attachment transfers. */
export const MAX_STREAMS_PER_SESSION = 12;
/** In this process, whatever the sessions. */
export const MAX_STREAMS_TOTAL = 1024;

export class StreamLimits {
  readonly #open = new Map<string, number>();
  readonly #perSession: number;
  readonly #max: number;
  #total = 0;

  constructor(perSession = MAX_STREAMS_PER_SESSION, max = MAX_STREAMS_TOTAL) {
    this.#perSession = perSession;
    this.#max = max;
  }

  /**
   * Takes a slot for `key` (the session), or `undefined` if it has none
   * left. The returned function gives it back; calling it again does
   * nothing.
   */
  acquire(key: string): (() => void) | undefined {
    const used = this.#open.get(key) ?? 0;
    if (used >= this.#perSession || this.#total >= this.#max) {
      return undefined;
    }
    this.#open.set(key, used + 1);
    this.#total += 1;
    let released = false;
    return () => {
      if (released) {
        return;
      }
      released = true;
      this.#total -= 1;
      const left = (this.#open.get(key) ?? 1) - 1;
      if (left <= 0) {
        this.#open.delete(key);
      } else {
        this.#open.set(key, left);
      }
    };
  }

  /** Open streams of `key` (tests). */
  openFor(key: string): number {
    return this.#open.get(key) ?? 0;
  }
}

/**
 * The response with its slot given back when the body ends, fails or is
 * cancelled (the browser went away). Without a body, at once.
 */
export const releasingWhenDone = (
  response: Response,
  release: () => void
): Response => {
  if (response.body === null) {
    release();
    return response;
  }
  const reader = response.body.getReader();
  const body = new ReadableStream<Uint8Array>({
    cancel: async (reason) => {
      release();
      await reader.cancel(reason);
    },
    pull: async (controller) => {
      try {
        const { done, value } = await reader.read();
        if (done) {
          release();
          controller.close();
          return;
        }
        controller.enqueue(value);
      } catch (error) {
        release();
        controller.error(error);
      }
    },
  });
  return new Response(body, {
    headers: response.headers,
    status: response.status,
    statusText: response.statusText,
  });
};
