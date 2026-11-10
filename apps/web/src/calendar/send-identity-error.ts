// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

/**
 * The user has no sending identity with the address an iTIP message must
 * come from (the invited address, or the organizer's): nothing is sent
 * (RFC 6047 §3, RFC 5546 §6.1: the other side matches From with it).
 */
export class NoSendingIdentityError extends Error {
  override readonly name = "NoSendingIdentityError";
  readonly address: string;
  constructor(address: string) {
    super(`No sending identity for ${address}`);
    this.address = address;
  }
}
