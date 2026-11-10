// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// The resolvers of the pinned adapters (Node, Bun and Deno all have
// `node:dns`). The default of every one of them is `cancelableResolver`
// (c-ares). Every answer is still validated by the guard.

import { Resolver as DnsResolver, lookup } from "node:dns/promises";

import { GuardError } from "../errors";
import type { DnsAnswer, Resolver } from "../resolve";

/**
 * OS resolver: `getaddrinfo` via `dns.lookup`, both families, no reordering.
 * Not cancelable: an abort only stops waiting; the lookup goes on. Opt-in
 * only, the default of no adapter.
 */
export const systemResolver: Resolver = async (hostname, signal) => {
  if (signal?.aborted === true) {
    throw new GuardError("timeout");
  }
  const answers = await lookup(hostname, { all: true, order: "verbatim" });
  const result: { A: string[]; AAAA: string[] } = { A: [], AAAA: [] };
  for (const { address, family } of answers) {
    (family === 6 ? result.AAAA : result.A).push(address);
  }
  return result satisfies DnsAnswer;
};

export interface CancelableResolverOptions {
  /** DNS servers (`ip` or `ip:port`); default: the system's (`/etc/resolv.conf`). */
  readonly servers?: readonly string[];
  /** Per try, in milliseconds: 2000. Bun rounds it up to whole seconds. */
  readonly timeoutMs?: number;
  /** Tries per server: 2. */
  readonly tries?: number;
}

/** Read through a function: it changes while the lookup waits. */
const isAborted = (signal: AbortSignal | undefined): boolean =>
  signal?.aborted === true;

const addressesOf = (
  settled: PromiseSettledResult<string[]>
): readonly string[] => (settled.status === "fulfilled" ? settled.value : []);

/**
 * A resolver on c-ares (`dns.promises.Resolver`), not `getaddrinfo`: it does
 * not hold a libuv threadpool thread, and an abort cancels the lookup
 * (`Resolver#cancel`), so a hung DNS server of some sender can't starve
 * WebCrypto, zlib or `fs`. A and AAAA are independent: one family that fails
 * or times out does not matter if the other answers. It does not read
 * `/etc/hosts` (the guard only accepts public addresses anyway).
 *
 * The same in Node, Deno and Bun. Bun rounds `timeoutMs` up to whole seconds
 * (the default, 2000, is exact); the total deadline still cuts it with
 * `cancel()`.
 */
export const createCancelableResolver =
  (options: CancelableResolverOptions = {}): Resolver =>
  async (hostname, signal) => {
    if (isAborted(signal)) {
      throw new GuardError("timeout");
    }
    const dns = new DnsResolver({
      timeout: options.timeoutMs ?? 2000,
      tries: options.tries ?? 2,
    });
    if (options.servers !== undefined) {
      dns.setServers([...options.servers]);
    }
    const cancel = () => {
      dns.cancel();
    };
    signal?.addEventListener("abort", cancel, { once: true });
    try {
      const [a, aaaa] = await Promise.allSettled([
        dns.resolve4(hostname),
        dns.resolve6(hostname),
      ]);
      // ECANCELLED: the deadline, not a DNS failure.
      if (isAborted(signal)) {
        throw new GuardError("timeout");
      }
      if (a.status === "rejected" && aaaa.status === "rejected") {
        throw new GuardError("dns");
      }
      return { A: addressesOf(a), AAAA: addressesOf(aaaa) } satisfies DnsAnswer;
    } finally {
      signal?.removeEventListener("abort", cancel);
    }
  };

/** `createCancelableResolver()` with the system's DNS servers. */
export const cancelableResolver: Resolver = createCancelableResolver();

/**
 * Whether this runtime's `dns.promises.Resolver` can be cancelled. Without
 * it a deadline would leave lookups running, so the adapters refuse to start
 * on the default resolver (fail closed).
 */
export const hasCancelableResolver = (): boolean =>
  typeof (DnsResolver.prototype as { cancel?: unknown }).cancel === "function";
