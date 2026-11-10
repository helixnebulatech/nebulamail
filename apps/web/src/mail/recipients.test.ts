// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import {
  mergeRecipients,
  parseRecipient,
  splitRecipientInput,
  swapIdentityBcc,
} from "./recipients";

describe(parseRecipient, () => {
  it("reads bare addresses and display names", () => {
    expect(parseRecipient(" ana@x.com ")).toStrictEqual({ email: "ana@x.com" });
    expect(parseRecipient('"Pérez, Ana" <ana@x.com>')).toStrictEqual({
      email: "ana@x.com",
      name: "Pérez, Ana",
    });
    expect(parseRecipient("<ana@x.com>")).toStrictEqual({ email: "ana@x.com" });
    expect(parseRecipient("mailto:ana@x.com")).toStrictEqual({
      email: "ana@x.com",
    });
    expect(parseRecipient("   ")).toBeNull();
  });
});

describe(splitRecipientInput, () => {
  it("splits on commas, semicolons and new lines", () => {
    expect(splitRecipientInput("a@x.com, b@y.com;c@z.com\nd")).toStrictEqual({
      entries: [
        { email: "a@x.com" },
        { email: "b@y.com" },
        { email: "c@z.com" },
      ],
      rest: "d",
    });
  });

  it("keeps separators inside quotes and angle brackets", () => {
    expect(splitRecipientInput('"Pérez, Ana" <ana@x.com>, bo')).toStrictEqual({
      entries: [{ email: "ana@x.com", name: "Pérez, Ana" }],
      rest: "bo",
    });
  });

  it("ignores empty entries", () => {
    expect(splitRecipientInput(",, ;")).toStrictEqual({
      entries: [],
      rest: "",
    });
  });
});

const at = (local: string): string => [local, "example.com"].join("@");

describe(swapIdentityBcc, () => {
  const work = { bcc: [{ email: at("archive") }] };
  const personal = { bcc: [{ email: at("me"), name: "Me" }] };

  it("adds the first sender's Bcc once", () => {
    expect([
      swapIdentityBcc([], null, work),
      swapIdentityBcc([{ email: at("ARCHIVE") }], null, work),
      swapIdentityBcc([], null, { bcc: [] }),
      swapIdentityBcc([], null, {}),
    ]).toStrictEqual([
      [{ email: at("archive") }],
      [{ email: at("ARCHIVE") }],
      [],
      [],
    ]);
  });

  it("replaces the previous identity's Bcc, keeping the user's own", () => {
    const mine = { email: at("boss") };

    expect(
      swapIdentityBcc([{ email: at("archive") }, mine], work, personal)
    ).toStrictEqual([mine, { email: at("me"), name: "Me" }]);
    // The user had removed it: nothing to remove, the new one comes.
    expect(swapIdentityBcc([mine], work, personal)).toStrictEqual([
      mine,
      { email: at("me"), name: "Me" },
    ]);
  });
});

describe(mergeRecipients, () => {
  it("skips addresses already there, ignoring case", () => {
    expect(
      mergeRecipients(
        [{ email: "Ana@x.com" }],
        [{ email: "ana@X.com" }, { email: "bob@y.com" }]
      )
    ).toStrictEqual([{ email: "Ana@x.com" }, { email: "bob@y.com" }]);
  });
});

describe("hostile input", () => {
  it("parses and merges in linear time", () => {
    const start = performance.now();
    expect(
      parseRecipient(`mailto:a${" ".repeat(200_000)}b`)?.email
    ).toHaveLength(200_002);
    expect(parseRecipient(`x ${"<".repeat(200_000)}`)).not.toBeNull();
    const many = Array.from({ length: 40_000 }, (_, i) => ({
      email: `u${String(i)}@x.example`,
    }));
    expect(mergeRecipients(many.slice(0, 10), [...many, ...many])).toHaveLength(
      40_000
    );
    expect(performance.now() - start).toBeLessThan(1000);
  });
});
