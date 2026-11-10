// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// DNS validation. The resolver is injected so the core
// stays runtime-agnostic (Node dns, DoH in Workers, fakes in tests).

import { checkAddress } from "./address";
import type { PublicAddress } from "./address";
import { isNonPublicName } from "./host-name";
import type { BlockReason } from "./types";
import { parseTargetUrl, PROXY_POLICY } from "./url";
import type { ParsedTarget, UrlPolicy } from "./url";

export interface DnsAnswer {
  readonly A: readonly string[];
  readonly AAAA: readonly string[];
}

/**
 * Resolve BOTH A and AAAA. Reject (throw) on failure; NXDOMAIN may also be an
 * empty answer. `signal` aborts the lookup (the fetch deadline); a resolver
 * that ignores it is still cut off by the caller.
 */
export type Resolver = (
  hostname: string,
  signal?: AbortSignal
) => Promise<DnsAnswer>;

declare const validatedBrand: unique symbol;

/**
 * A URL whose every resolved address is public. Only `resolveAndCheck`
 * produces it. Adapters must connect to one of `addresses` and nothing else.
 */
export interface ValidatedTarget extends ParsedTarget {
  /** Validated addresses, IPv4 first, deduplicated. Never empty. */
  readonly addresses: readonly [PublicAddress, ...PublicAddress[]];
  readonly [validatedBrand]: true;
}

export type GuardResult =
  | { readonly ok: true; readonly target: ValidatedTarget }
  | { readonly ok: false; readonly reason: BlockReason };

const block = (reason: BlockReason): GuardResult => ({ ok: false, reason });

const seal = (
  target: ParsedTarget,
  addresses: readonly [PublicAddress, ...PublicAddress[]]
): GuardResult => {
  // Minting the brand: only this function creates a ValidatedTarget.
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion
  const validated = {
    ...target,
    addresses: Object.freeze([...addresses]),
  } as unknown as ValidatedTarget;
  return { ok: true, target: Object.freeze(validated) };
};

/**
 * Resolve a parsed target and validate every address. If ANY address is not
 * public the whole target is rejected: a mixed answer is an
 * attack or a misconfiguration, and we cannot know which IP the OS would pick.
 * IP literals were already validated by `parseTargetUrl`; the resolver is not called.
 * Neither is it for single-label or special-use names (`isNonPublicName`): `dns`.
 */
export const resolveAndCheck = async (
  target: ParsedTarget,
  resolver: Resolver
): Promise<GuardResult> => {
  if (target.host.kind === "ip") {
    return seal(target, [target.host.address]);
  }
  // Single-label and special-use names are never public: refused before
  // asking any resolver (so not even mDNS on the LAN sees them).
  if (isNonPublicName(target.host.name)) {
    return block("dns");
  }

  let answer: DnsAnswer;
  try {
    answer = await resolver(target.host.name);
  } catch {
    return block("dns");
  }

  const seen = new Set<string>();
  const addresses: PublicAddress[] = [];
  for (const raw of [...answer.A, ...answer.AAAA]) {
    const verdict = checkAddress(raw);
    if (!verdict.ok) {
      // Garbage from the resolver is a DNS failure, not a private IP.
      return block(verdict.range === "invalid" ? "dns" : "private-ip");
    }
    if (!seen.has(verdict.address)) {
      seen.add(verdict.address);
      addresses.push(verdict.address);
    }
  }

  const [first, ...rest] = addresses;
  if (first === undefined) {
    return block("dns");
  }
  return seal(target, [first, ...rest]);
};

/** `parseTargetUrl` + `resolveAndCheck`: the full static and DNS guard. */
export const guardUrl = async (
  input: string | URL,
  resolver: Resolver,
  policy: UrlPolicy = PROXY_POLICY
): Promise<GuardResult> => {
  const parsed = parseTargetUrl(input, policy);
  if (!parsed.ok) {
    return parsed;
  }
  return await resolveAndCheck(parsed.target, resolver);
};
