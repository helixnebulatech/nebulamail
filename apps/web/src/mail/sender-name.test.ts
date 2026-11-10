// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { senderNameWarning } from "./sender-name";

/** Builds an address (keeps the test free of address literals). */
const at = (local: string, domain: string): string => `${local}@${domain}`;

describe(senderNameWarning, () => {
  it("no longer judges brand names (no brand list)", () => {
    expect(senderNameWarning("PayPal", at("x", "attacker.example"))).toBeNull();
    expect(
      senderNameWarning("PayPal México", at("service", "paypal.com.mx"))
    ).toBeNull();
  });

  it("leaves ordinary names alone", () => {
    for (const name of [
      "Ana Ruiz",
      "Pablo Paypale",
      "Café Ingenio",
      "",
      null,
      undefined,
    ]) {
      expect(senderNameWarning(name, at("x", "example.com"))).toBeNull();
    }
  });

  it("flags an address in the name that isn't the real one", () => {
    expect(
      senderNameWarning(
        at("service", "paypal.com"),
        at("x", "attacker.example")
      )
    ).toStrictEqual({ kind: "address", shown: at("service", "paypal.com") });
    // Any other address, even from the same domain.
    expect(
      senderNameWarning(
        `Ana (${at("ana", "example.com")})`,
        at("no-reply", "example.com")
      )
    ).toMatchObject({ kind: "address", shown: at("ana", "example.com") });
    // Full-width ＠, mailto: and an invisible character don't hide it.
    expect(
      senderNameWarning(
        "mailto:boss\u200B＠corp.example",
        at("x", "attacker.example")
      )
    ).toMatchObject({ kind: "address", shown: at("boss", "corp.example") });
  });

  it("a name that repeats the real address is fine", () => {
    const real = at("Ana.Ruiz", "example.com");
    expect(senderNameWarning(real.toLowerCase(), real)).toBeNull();
    expect(senderNameWarning(`<${real}>`, real)).toBeNull();
  });

  it("padding with invisible characters doesn't hide the address", () => {
    expect(
      senderNameWarning(
        `${"\u2066".repeat(5000)}${at("boss", "corp.example")}`,
        at("x", "example.net")
      )
    ).toMatchObject({ kind: "address" });
  });

  it("is quick and capped with huge names", () => {
    const huge = `${"a".repeat(200_000)}@${"b.".repeat(100_000)}`;
    const start = performance.now();
    senderNameWarning(huge, at("x", "example.com"));
    senderNameWarning("Pay ".repeat(100_000), at("x", "example.com"));
    senderNameWarning("@".repeat(200_000), at("x", "example.com"));
    expect(performance.now() - start).toBeLessThan(200);
    const shown = senderNameWarning(
      `${"a".repeat(100)}@example.com`,
      at("x", "example.com")
    );
    expect(shown?.shown.length).toBeLessThanOrEqual(80);
  });
});
