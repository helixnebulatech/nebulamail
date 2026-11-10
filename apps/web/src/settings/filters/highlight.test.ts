// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { generateScript } from "./generate";
import { highlightSieve } from "./highlight";

const kinds = (text: string) =>
  highlightSieve(text)
    .filter((token) => token.text.trim() !== "")
    .map((token) => [token.kind, token.text]);

describe(highlightSieve, () => {
  it("colours each part of a script", () => {
    expect(
      kinds(
        'require ["fileinto"];\nif header :contains "subject" "x" { fileinto "A"; stop; } # done'
      )
    ).toStrictEqual(
      [
        ["keyword", "require"],
        ["punct", "["],
        ["string", '"fileinto"'],
        ["punct", "];"],
        ["keyword", "if"],
        ["test", "header"],
        ["tag", ":contains"],
        ["string", '"subject"'],
        ["plain", " "],
        ["string", '"x"'],
        ["punct", "{"],
        ["action", "fileinto"],
        ["string", '"A"'],
        ["punct", ";"],
        ["keyword", "stop"],
        ["punct", ";"],
        ["punct", "}"],
        ["comment", "# done"],
      ].filter(([, text]) => text?.trim() !== "")
    );
  });

  it("handles numbers, block comments, escapes and text: strings", () => {
    expect(
      kinds(
        'if size :over 100K { reject text:\nNo .\n..dots\n.\n; } /* c */ "a\\"b"'
      )
    ).toStrictEqual([
      ["keyword", "if"],
      ["test", "size"],
      ["tag", ":over"],
      ["number", "100K"],
      ["punct", "{"],
      ["action", "reject"],
      ["string", "text:\nNo .\n..dots\n.\n"],
      ["punct", ";"],
      ["punct", "}"],
      ["comment", "/* c */"],
      ["string", '"a\\"b"'],
    ]);
  });

  it("marks unfinished text instead of failing", () => {
    expect(kinds('fileinto "Inbox')).toStrictEqual([
      ["action", "fileinto"],
      ["invalid", '"Inbox'],
    ]);
    expect(kinds("/* open")).toStrictEqual([["invalid", "/* open"]]);
    expect(kinds("keep @")).toStrictEqual([
      ["action", "keep"],
      ["invalid", "@"],
    ]);
  });

  it("always joins back into exactly the text", () => {
    const samples = [
      "",
      "\r\n\t ",
      "text:\r\nline\r\n.\r\n",
      "if anyof (true, false) { keep; }",
      '"unterminated \\',
      generateScript({ rules: [], vacation: true }),
      "ünïcödé :tägs 12G # ok",
    ];
    for (const text of samples) {
      expect(
        highlightSieve(text)
          .map((token) => token.text)
          .join("")
      ).toBe(text);
    }
    // Random text, including every special character.
    const alphabet = 'ab:"\\#/*\n 1K[]{}();,.text';
    for (let round = 0; round < 300; round += 1) {
      let text = "";
      for (let index = 0; index < 40; index += 1) {
        text += alphabet.charAt(Math.floor(Math.random() * alphabet.length));
      }
      expect(
        highlightSieve(text)
          .map((token) => token.text)
          .join("")
      ).toBe(text);
    }
  });
});
