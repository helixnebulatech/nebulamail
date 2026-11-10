// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Shared test fixtures.

import { parseTicketKeys, signTicket } from "./tickets/index";
import type { TicketClaims, TicketKeys } from "./tickets/index";

/** Fixed test keys (never use outside tests). */
export const KEY_1 = `1:${"A".repeat(43)}`;
export const KEY_2 = `2:${"B".repeat(42)}A`;
export const KEY_3 = `3:${"C".repeat(42)}A`;

export const NOW = 1_800_000_000_000;
export const APP_ORIGIN = "https://mail.example.com";
export const PROXY = "https://img.example.com";

export const keys = async (value = KEY_1): Promise<TicketKeys> =>
  await parseTicketKeys(value);

export const claims = (
  overrides: Partial<TicketClaims> = {}
): TicketClaims => ({
  accountId: "acc1",
  emailId: "mail1",
  expiresAt: NOW + 5 * 60 * 1000,
  url: "https://images.example.com/a.png",
  ...overrides,
});

export const ticket = async (
  k: TicketKeys,
  overrides: Partial<TicketClaims> = {}
): Promise<string> => await signTicket(k, claims(overrides), NOW);

const pad = (n: number): number[] => Array.from({ length: n }, () => 0);

/** Minimal valid image headers, padded to `size` bytes. */
export const png = (size = 32): Uint8Array =>
  Uint8Array.from([
    0x89,
    0x50,
    0x4e,
    0x47,
    0x0d,
    0x0a,
    0x1a,
    0x0a,
    ...pad(Math.max(0, size - 8)),
  ]);
export const gif = (size = 32): Uint8Array =>
  Uint8Array.from([
    0x47,
    0x49,
    0x46,
    0x38,
    0x39,
    0x61,
    ...pad(Math.max(0, size - 6)),
  ]);
export const text = (value: string): Uint8Array =>
  new TextEncoder().encode(value);
