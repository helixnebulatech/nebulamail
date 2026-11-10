// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

/**
 * The push streams of each client (tab), by key: a new one with the same
 * key ends the previous one. The server can't count on hearing that the
 * browser left (Vercel never says so), so a tab that reconnects after a
 * network drop would otherwise pile up streams until each times out.
 */
export class SupersededStreams {
  readonly #live = new Map<string, () => void>();

  /**
   * Registers a stream under `key`, ending (`end`) the one there was. The
   * returned function forgets it (when it ends by itself); it never
   * forgets a newer one.
   */
  claim(key: string, end: () => void): () => void {
    const previous = this.#live.get(key);
    this.#live.set(key, end);
    previous?.();
    return () => {
      if (this.#live.get(key) === end) {
        this.#live.delete(key);
      }
    };
  }

  /** Registered streams (tests). */
  get size(): number {
    return this.#live.size;
  }
}
