// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { decodePunycode, senderDomainWarning } from "./sender-domain";

// Written with escapes: these look like Latin on purpose.
const CYR_A = "\u0430";
const CYR_ER = "\u0440";
const CYR_PALOCHKA = "\u04CF";
const CYR_IE = "\u0435";
const GREEK_OMICRON = "\u03BF";

describe(senderDomainWarning, () => {
  it("says nothing for plain ASCII domains", () => {
    for (const email of ["ana@paypal.com", "x@sub.example.org", "a@b", ""]) {
      expect(senderDomainWarning(email)).toBeNull();
    }
  });

  it("flags one label mixing alphabets (a Cyrillic letter in paypal)", () => {
    expect(senderDomainWarning(`soporte@p${CYR_A}ypal.com`)).toStrictEqual({
      ascii: new URL(`http://p${CYR_A}ypal.com`).hostname,
      kind: "lookalike",
    });
    expect(
      senderDomainWarning(`a@g${GREEK_OMICRON}${GREEK_OMICRON}gle.com`)?.kind
    ).toBe("lookalike");
  });

  it("flags a whole label in Cyrillic under a Latin TLD", () => {
    const apple = `${CYR_A}${CYR_ER}${CYR_ER}${CYR_PALOCHKA}${CYR_IE}`;
    const warning = senderDomainWarning(`id@${apple}.com`);
    expect(warning?.kind).toBe("lookalike");
    expect(warning?.ascii).toMatch(/^xn--[a-z0-9-]+\.com$/u);
  });

  it("only shows how it is written for other international domains", () => {
    expect(senderDomainWarning("info@m\u00FCnchen.de")).toStrictEqual({
      ascii: "xn--mnchen-3ya.de",
      kind: "international",
    });
    // Cyrillic with a Cyrillic TLD (.рф), Japanese mixing kanji and kana.
    expect(
      senderDomainWarning("a@\u043F\u0440\u0438\u043C\u0435\u0440.\u0440\u0444")
        ?.kind
    ).toBe("international");
    expect(senderDomainWarning("a@\u65E5\u672C\u306E\u4F8B.jp")?.kind).toBe(
      "international"
    );
    // Already punycode: the address shown is the punycode itself.
    expect(senderDomainWarning("a@xn--mnchen-3ya.de")).toStrictEqual({
      ascii: "xn--mnchen-3ya.de",
      kind: "international",
    });
  });

  it("flags invisible characters and Latin-looking symbols", () => {
    for (const email of [
      // Zero-width joiner and soft hyphen inside "paypal".
      "a@pay\u200Dpal.com",
      "a@pay\u00ADpal.com",
      // Ideographic number zero (Han) for the o's of google.
      "a@g\u3007\u3007gle.com",
      // The Cyrillic apple with a trailing dot or an ideographic dot.
      "a@\u0430\u0440\u0440\u04CF\u0435.com.",
      "a@\u0430\u0440\u0440\u04CF\u0435\u3002com",
    ]) {
      expect(senderDomainWarning(email)?.kind).toBe("lookalike");
    }
  });

  it("keeps a normal word in its own alphabet international", () => {
    for (const email of [
      // "primer" in Cyrillic under .bg and under .com (и doesn't look Latin).
      "a@\u043F\u0440\u0438\u043C\u0435\u0440.bg",
      "a@\u043F\u0440\u0438\u043C\u0435\u0440.com",
      // "ellinika" in Greek under .gr.
      "a@\u03B5\u03BB\u03BB\u03B7\u03BD\u03B9\u03BA\u03AC.gr",
      // Upper-case punycode.
      "a@XN--MNCHEN-3YA.DE",
    ]) {
      expect(senderDomainWarning(email)?.kind).toBe("international");
    }
    // Full-width Latin letters: really paypal.com, written to look like it.
    expect(
      senderDomainWarning("a@\uFF50\uFF41\uFF59\uFF50\uFF41\uFF4C.com")
    ).toStrictEqual({ ascii: "paypal.com", kind: "lookalike" });
  });

  it("covers what the second review found", () => {
    for (const email of [
      // Dotless i, script a; Lisu letters; Cyrillic apple under .eu and .me.
      "a@g\u0131thub.com",
      "a@p\u0251ypal.com",
      "a@\uA4D1\uA4EE\uA4EC\uA4D1\uA4EE\uA4E1.com",
      "a@\u0430\u0440\u0440\u04CF\u0435.eu",
      "a@\u0430\u0440\u0440\u04CF\u0435.me",
      // The parser would stop at "/": never shown as paypal.com.
      "a@paypal.com/\u65E5\u672C",
      // Huge: refused before parsing.
      `a@${"\u0430.".repeat(100_000)}com`,
    ]) {
      expect(senderDomainWarning(email)?.kind).toBe("lookalike");
    }
    expect(senderDomainWarning("a@paypal.com/\u65E5\u672C")?.ascii).not.toBe(
      "paypal.com"
    );
    for (const email of [
      // Japanese words with the long-vowel mark (Common script).
      "a@\u30B3\u30FC\u30D2\u30FC.jp",
      "a@\u30E9\u30FC\u30E1\u30F3.com",
      // Turkish dotless i under .tr.
      "a@\u0131\u015F\u0131k.com.tr",
    ]) {
      expect(senderDomainWarning(email)?.kind).toBe("international");
    }
  });

  it("answers fast for a huge domain", () => {
    const start = performance.now();
    senderDomainWarning(`a@${"\u0430.".repeat(1_000_000)}com`);
    expect(performance.now() - start).toBeLessThan(200);
  });

  it("reads punycode as a person would once decoded", () => {
    const apple = `${CYR_A}${CYR_ER}${CYR_ER}${CYR_PALOCHKA}${CYR_IE}`;
    for (const unicode of [`${apple}.com`, `p${CYR_A}ypal.com`]) {
      const { hostname } = new URL(`http://${unicode}/`);
      expect(hostname.startsWith("xn--")).toBeTruthy();
      expect(senderDomainWarning(`a@${hostname}`)).toStrictEqual({
        ascii: hostname,
        kind: "lookalike",
      });
      expect(senderDomainWarning(`a@${hostname.toUpperCase()}`)?.kind).toBe(
        "lookalike"
      );
    }
    // Not valid punycode: never trusted.
    expect(senderDomainWarning("a@xn--zzzzzzzzzzzz9.com")?.kind).toBe(
      "lookalike"
    );
  });

  it("decodes punycode labels (RFC 3492)", () => {
    expect(decodePunycode("mnchen-3ya")).toBe("m\u00FCnchen");
    expect(decodePunycode("zckzah")).toBe("\u30C6\u30B9\u30C8");
    for (const bad of ["", "-", "zzzzzzzzzzzz9", "99999999999", "a\u00E9-x"]) {
      expect(() => decodePunycode(bad)).not.toThrow();
    }
    expect(decodePunycode("99999999999")).toBeNull();
  });

  it("only takes the one canonical punycode spelling", () => {
    // punycode.js decodes these, but no encoder writes them.
    expect(decodePunycode("kqzok93g8x6idkn95e")).toBeNull();
    // A lone surrogate ("a\uD800b") is not text.
    expect(decodePunycode("ab-zd9k")).toBeNull();
    expect(senderDomainWarning("a@xn--kqzok93g8x6idkn95e.com")?.kind).toBe(
      "lookalike"
    );
  });

  it("covers what the third review found", () => {
    for (const email of [
      // Roman numeral l, bold maths letters, circled p, Kelvin sign K.
      "a@paypa\u217C.com",
      "a@\u{1D429}\u{1D41A}\u{1D432}\u{1D429}\u{1D41A}\u{1D425}.com",
      "a@\u24DFaypal.com",
      "a@\u212Aodak.com",
      // Greek with a lunate sigma: "cook".
      "a@\u03F2\u03BF\u03BF\u03BA.com",
    ]) {
      expect(senderDomainWarning(email)?.kind).toBe("lookalike");
    }
    // Long ASCII part plus a far code point: valid punycode, same verdict
    // written either way.
    const unicode = `${"a".repeat(40)}\u9F8D.com`;
    const { hostname } = new URL(`http://${unicode}/`);
    expect(decodePunycode(hostname.split(".")[0]?.slice(4) ?? "")).toBe(
      `${"a".repeat(40)}\u9F8D`
    );
    expect(senderDomainWarning(`a@${unicode}`)?.kind).toBe("international");
    // Over 63 octets once encoded: not a real name, either way.
    const long = `${"a".repeat(60)}\u9F8D.com`;
    expect(senderDomainWarning(`a@${long}`)?.kind).toBe("lookalike");
    expect(
      senderDomainWarning(`a@${new URL(`http://${long}/`).hostname}`)?.kind
    ).toBe("lookalike");
    expect(senderDomainWarning(`a@${hostname}`)?.kind).toBe(
      senderDomainWarning(`a@${unicode}`)?.kind
    );
  });

  it("never throws on odd input", () => {
    for (const email of ["@", "a@\u0000", "a@[::1]", "a@..", "a@%zz"]) {
      expect(() => senderDomainWarning(email)).not.toThrow();
    }
  });
});
