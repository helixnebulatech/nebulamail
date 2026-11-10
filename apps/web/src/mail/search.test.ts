// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { parseSearch, tokenize } from "./search";

describe(tokenize, () => {
  it("keeps quoted phrases together, also after an operator", () => {
    expect(
      tokenize('asunto:"informe anual" de:bob "hola mundo" x')
    ).toStrictEqual(['asunto:"informe anual"', "de:bob", '"hola mundo"', "x"]);
  });
});

describe(parseSearch, () => {
  it.each([
    ["", null],
    ["   ", null],
    ["factura", { text: "factura" }],
    ["de:bob", { from: "bob" }],
    ["from:bob", { from: "bob" }],
    ['asunto:"informe anual"', { subject: "informe anual" }],
    ["tiene:adjunto", { hasAttachment: true }],
    ["IS:UNREAD", { notKeyword: "$seen" }],
    ["es:destacado", { hasKeyword: "$flagged" }],
    ["antes:2026-10-01", { before: "2026-10-01T00:00:00Z" }],
    ["después:2026-09-01", { after: "2026-09-01T00:00:00Z" }],
  ])("%j", (query, expected) => {
    expect(parseSearch(query)).toStrictEqual(expected);
  });

  it("combines operators and free text with AND", () => {
    expect(
      parseSearch('de:bob tiene:adjunto factura "de octubre"')
    ).toStrictEqual({
      conditions: [
        { from: "bob" },
        { hasAttachment: true },
        { text: 'factura "de octubre"' },
      ],
      operator: "AND",
    });
  });

  it("keeps the quotes of a phrase, closing them if needed", () => {
    expect(parseSearch('"plan  anual" "q3')).toStrictEqual({
      text: '"plan anual" q3',
    });
    expect(parseSearch('"hola mundo')).toStrictEqual({
      text: '"hola mundo"',
    });
    expect(parseSearch('""')).toBeNull();
  });

  it("treats unknown operators, empty values and invalid dates as text", () => {
    expect(parseSearch("hora:10 de: antes:2026-02-30")).toStrictEqual({
      text: "hora:10 de: antes:2026-02-30",
    });
  });

  it("finds nothing on Object.prototype", () => {
    for (const query of [
      "constructor",
      "__proto__",
      "toString",
      "constructor:foo",
      "__proto__:foo",
      "constructor foo",
    ]) {
      expect(parseSearch(query)).toStrictEqual({ text: query });
    }
  });

  it("reads the operator name apart from the value", () => {
    // "después" written decomposed (e + U+0301): NFC makes the name shorter.
    expect(parseSearch("despue\u0301s:2026-09-01")).toStrictEqual({
      after: "2026-09-01T00:00:00Z",
    });
    expect(parseSearch("DE:Bob")).toStrictEqual({ from: "Bob" });
    // "İ" lower-cases to two characters.
    expect(parseSearch("\u0130:x")).toStrictEqual({ text: "\u0130:x" });
    expect(parseSearch("\u0130\u0130de:x")).toStrictEqual({
      text: "\u0130\u0130de:x",
    });
  });
});
