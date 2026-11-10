// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Host names that never lead to a public host (like Stalwart 1.0 does before
// resolving). The twin of `DomainName::is_non_public` in the Rust crate:
// change both together.
//
// Why: the IP check alone trusts the resolver. With split-horizon DNS an
// internal name (`grafana.internal`, `intranet`) may resolve to an address
// that looks public but is only reachable from inside, and getaddrinfo sends
// `.local` names to mDNS on the LAN. These names are refused before any
// lookup, as `dns`: they have no answer in the public DNS.
//
// A static list instead of the Public Suffix List: no dependency to keep up
// to date, and the PSL would also refuse `.test` and `.example`, which every
// test suite here uses as stand-in public names. Those two are left out on
// purpose: RFC 6761 §6.2/§6.5 keeps them out of the root, so a real resolver
// answers NXDOMAIN (already `dns`) and getaddrinfo never sends them to mDNS.

/** Top-level labels that are never a public DNS name. */
const NON_PUBLIC_TLDS: ReadonlySet<string> = new Set([
  // RFC 6761 §6.3: loopback.
  "localhost",
  // RFC 6761 §6.4: APIs should answer "no such name" at once.
  "invalid",
  // RFC 6762: multicast DNS on the local link.
  "local",
  // RFC 7686: Tor.
  "onion",
  // RFC 9476: names of non-DNS systems.
  "alt",
  // RFC 3172 infrastructure, with home.arpa (RFC 8375), in-addr.arpa and
  // ip6.arpa (reverse DNS): never a web host.
  "arpa",
  // ICANN 2024: reserved for private use, never delegated.
  "internal",
  // ICANN name collision (2018): never delegated, widely used inside networks.
  "corp",
  "home",
  "mail",
  // Common private conventions (routers, Linux defaults, intranets).
  "lan",
  "intranet",
  "localdomain",
]);

/**
 * True if `hostname` (a URL host name, not an IP literal) can only be a
 * local or special-use name: a single label (`intranet`, `localhost`) or one
 * under a suffix in the list above. Case and trailing dots are ignored.
 */
export const isNonPublicName = (hostname: string): boolean => {
  let end = hostname.length;
  while (end > 0 && hostname[end - 1] === ".") {
    end -= 1;
  }
  const name = hostname.slice(0, end).toLowerCase();
  const dot = name.lastIndexOf(".");
  if (dot === -1) {
    return true;
  }
  return NON_PUBLIC_TLDS.has(name.slice(dot + 1));
};
