// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// IP address classification.
//
// Approach: an allow-list ("only global unicast") with two independent layers.
// An address is public only if BOTH agree:
//   1. Our own explicit tables below (every special-purpose range plus the
//      IPv6 rule "must be inside 2000::/3", the current global unicast space).
//   2. ipaddr.js `range() === "unicast"`.
// Layer 2 alone is not enough: ipaddr.js 2.5.0 reports the IPv4-compatible
// address `::127.0.0.1` as "unicast". Layer 1 alone would trust our tables
// blindly. Addresses that embed an IPv4 (mapped, NAT64, 6to4) are judged by
// the embedded IPv4.

import ipaddr from "ipaddr.js";

declare const publicAddressBrand: unique symbol;

/**
 * An IP address (canonical text form) that passed `checkAddress`. Only this
 * module can produce it, so holding one proves the check happened.
 */
export type PublicAddress = string & { readonly [publicAddressBrand]: true };

export type AddressCheck =
  | {
      readonly ok: true;
      readonly address: PublicAddress;
      readonly family: 4 | 6;
    }
  | {
      readonly ok: false;
      /** Name of the blocked range, or "invalid" if the text is not an IP. */
      readonly range: string;
    };

type Cidr = readonly [ipaddr.IPv4 | ipaddr.IPv6, number];

const cidr = (text: string): Cidr => ipaddr.parseCIDR(text);

/** Blocked IPv4 ranges.1 rule 5, checked in order. */
const BLOCKED_V4: readonly (readonly [string, Cidr])[] = [
  ["this-network", cidr("0.0.0.0/8")],
  ["private", cidr("10.0.0.0/8")],
  ["carrier-grade-nat", cidr("100.64.0.0/10")],
  ["loopback", cidr("127.0.0.0/8")],
  ["link-local", cidr("169.254.0.0/16")],
  ["private", cidr("172.16.0.0/12")],
  ["ietf-protocol", cidr("192.0.0.0/24")],
  ["documentation", cidr("192.0.2.0/24")],
  ["6to4-relay", cidr("192.88.99.0/24")],
  ["private", cidr("192.168.0.0/16")],
  ["benchmarking", cidr("198.18.0.0/15")],
  ["documentation", cidr("198.51.100.0/24")],
  ["documentation", cidr("203.0.113.0/24")],
  ["multicast", cidr("224.0.0.0/4")],
  // 240.0.0.0/4 also contains 255.255.255.255 (broadcast).
  ["reserved", cidr("240.0.0.0/4")],
];

/** Global unicast IPv6 space (IANA). Anything outside is blocked. */
const GLOBAL_UNICAST_V6 = cidr("2000::/3");

/**
 * Blocked IPv6 ranges, checked in order before the 2000::/3 rule. The ones
 * outside 2000::/3 are listed only to give a precise range name in results.
 */
const BLOCKED_V6: readonly (readonly [string, Cidr])[] = [
  // ::/96 holds :: (unspecified), ::1 (loopback) and the deprecated
  // IPv4-compatible addresses ::a.b.c.d (RFC 4291 §2.5.5.1). None is a valid
  // destination, so the whole /96 is blocked instead of extracting the IPv4.
  ["unspecified", cidr("::/128")],
  ["loopback", cidr("::1/128")],
  ["ipv4-compatible", cidr("::/96")],
  // IPv4-translated (RFC 6145/7136), not routable on the Internet.
  ["ipv4-translated", cidr("::ffff:0:0:0/96")],
  // Local-use NAT64 prefix (RFC 8215). Where the IPv4 sits inside it depends
  // on the operator's prefix length, so we cannot extract it reliably: block.
  ["nat64-local", cidr("64:ff9b:1::/48")],
  ["discard", cidr("100::/64")],
  ["unique-local", cidr("fc00::/7")],
  ["link-local", cidr("fe80::/10")],
  ["site-local", cidr("fec0::/10")],
  ["multicast", cidr("ff00::/8")],
  // Teredo hides the client IPv4 (obfuscated) and the relay is arbitrary.
  ["teredo", cidr("2001::/32")],
  // IETF protocol assignments (benchmarking, ORCHID, AMT, ...).
  ["ietf-protocol", cidr("2001::/23")],
  ["documentation", cidr("2001:db8::/32")],
  ["documentation", cidr("3fff::/20")],
];

const IPV4_MAPPED = cidr("::ffff:0:0/96");
const NAT64_WELL_KNOWN = cidr("64:ff9b::/96");
const SIX_TO_FOUR = cidr("2002::/16");

/**
 * CIDR membership. Not written as `address.match(range)` in a condition: a
 * lint autofix (prefer-regexp-test) mistakes ipaddr's `match` for
 * `String#match` and rewrites it into broken code.
 */
const contains = (range: Cidr, address: ipaddr.IPv4 | ipaddr.IPv6): boolean => {
  const [network, bits] = range;
  if (network.kind() !== address.kind()) {
    return false;
  }
  const inside: boolean = address.match(network, bits);
  return inside;
};

const matchFirst = (
  address: ipaddr.IPv4 | ipaddr.IPv6,
  table: readonly (readonly [string, Cidr])[]
): string | undefined => {
  for (const [name, range] of table) {
    if (contains(range, address)) {
      return name;
    }
  }
  return undefined;
};

const allow = (
  address: ipaddr.IPv4 | ipaddr.IPv6,
  family: 4 | 6
): AddressCheck => ({
  // Minting the brand: this is the only place a PublicAddress is created.
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion
  address: address.toString() as PublicAddress,
  family,
  ok: true,
});

const checkV4 = (address: ipaddr.IPv4): AddressCheck => {
  const blocked = matchFirst(address, BLOCKED_V4);
  if (blocked !== undefined) {
    return { ok: false, range: blocked };
  }
  // Second layer (also blocks AS112 and AMT anycast, which is fine here).
  const range = address.range();
  if (range !== "unicast") {
    return { ok: false, range };
  }
  return allow(address, 4);
};

/** IPv4 from two 16-bit IPv6 groups (arithmetic instead of bit operators). */
const v4FromGroups = (hi = 0, lo = 0): ipaddr.IPv4 =>
  new ipaddr.IPv4([
    Math.floor(hi / 256),
    hi % 256,
    Math.floor(lo / 256),
    lo % 256,
  ]);

/** IPv4 from the last 32 bits of an IPv6 address. */
const lowV4 = (parts: readonly number[]): ipaddr.IPv4 =>
  v4FromGroups(parts[6], parts[7]);

/** Judge an address that embeds an IPv4 by that IPv4. */
const viaEmbedded = (
  outer: ipaddr.IPv6,
  inner: ipaddr.IPv4,
  label: string
): AddressCheck => {
  const verdict = checkV4(inner);
  if (!verdict.ok) {
    return { ok: false, range: `${label}:${verdict.range}` };
  }
  return allow(outer, 6);
};

const checkV6 = (address: ipaddr.IPv6): AddressCheck => {
  const { parts } = address;
  if (contains(IPV4_MAPPED, address)) {
    return viaEmbedded(address, lowV4(parts), "ipv4-mapped");
  }
  if (contains(NAT64_WELL_KNOWN, address)) {
    return viaEmbedded(address, lowV4(parts), "nat64");
  }
  if (contains(SIX_TO_FOUR, address)) {
    const inner = v4FromGroups(parts[1], parts[2]);
    return viaEmbedded(address, inner, "6to4");
  }
  const blocked = matchFirst(address, BLOCKED_V6);
  if (blocked !== undefined) {
    return { ok: false, range: blocked };
  }
  if (!contains(GLOBAL_UNICAST_V6, address)) {
    return { ok: false, range: "not-global-unicast" };
  }
  const range = address.range();
  if (range !== "unicast") {
    return { ok: false, range };
  }
  return allow(address, 6);
};

/**
 * Strict IPv6 parsing. ipaddr.js is lenient with a trailing dotted IPv4: it
 * accepts octal/hex parts and silently rewrites `::a.b.c.d` (IPv4-compatible)
 * into `::ffff:a.b.c.d` (IPv4-mapped), i.e. a different address. So we turn a
 * trailing four-part decimal IPv4 into two hex groups ourselves, and only
 * then let ipaddr.js parse pure hex text.
 */
const strictV6 = (text: string): ipaddr.IPv6 | undefined => {
  // Zone ids (fe80::1%eth0) only make sense for link-local addresses.
  if (text.includes("%")) {
    return undefined;
  }
  let hex = text;
  if (text.includes(".")) {
    const cut = text.lastIndexOf(":");
    const tail = text.slice(cut + 1);
    if (!ipaddr.IPv4.isValidFourPartDecimal(tail)) {
      return undefined;
    }
    const [a = 0, b = 0, c = 0, d = 0] = ipaddr.IPv4.parse(tail).octets;
    const groups = `${(a * 256 + b).toString(16)}:${(c * 256 + d).toString(16)}`;
    hex = `${text.slice(0, cut + 1)}${groups}`;
  }
  if (hex.includes(".") || !ipaddr.IPv6.isValid(hex)) {
    return undefined;
  }
  return ipaddr.IPv6.parse(hex);
};

/**
 * Decide whether an IP address is a public destination.
 *
 * Accepts only canonical forms: dotted four-part decimal IPv4 (what WHATWG URL
 * and DNS produce) and IPv6 without brackets or zone id. Octal, hex and short
 * IPv4 forms are rejected as "invalid" here; URL parsing normalizes them first.
 */
export const checkAddress = (text: string): AddressCheck => {
  if (text.includes(":")) {
    const v6 = strictV6(text);
    if (v6 !== undefined) {
      return checkV6(v6);
    }
  } else if (ipaddr.IPv4.isValidFourPartDecimal(text)) {
    return checkV4(ipaddr.IPv4.parse(text));
  }
  return { ok: false, range: "invalid" };
};
