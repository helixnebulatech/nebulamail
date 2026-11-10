// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Syntax colours for the Sieve editor: our own small
// scanner, no dependency. Unlike tokenize.ts it never throws: half-typed
// text is coloured as far as it goes (an unterminated string or comment
// runs to the end and is marked "invalid"). The pieces always add up to
// the exact text, whitespace included, so the coloured layer lines up
// with the textarea above it.

import type { CodeToken, CodeTokenKind } from "@nebulamail/ui";

/** Control structure (RFC 5228 §3). */
const CONTROL = new Set(["require", "if", "elsif", "else", "stop"]);

/** Tests of RFC 5228 and common extensions. */
const TESTS = new Set([
  "address",
  "allof",
  "anyof",
  "body",
  "currentdate",
  "date",
  "duplicate",
  "envelope",
  "environment",
  "exists",
  "false",
  "hasflag",
  "header",
  "ihave",
  "mailboxexists",
  "mailboxidexists",
  "metadata",
  "metadataexists",
  "not",
  "servermetadata",
  "servermetadataexists",
  "size",
  "spamtest",
  "specialuse_exists",
  "string",
  "true",
  "valid_notify_method",
  "virustest",
]);

/** Actions of RFC 5228 and common extensions. */
const ACTIONS = new Set([
  "addflag",
  "addheader",
  "break",
  "convert",
  "deleteheader",
  "discard",
  "ereject",
  "error",
  "extracttext",
  "fileinto",
  "foreverypart",
  "global",
  "include",
  "keep",
  "notify",
  "redirect",
  "reject",
  "removeflag",
  "replace",
  "return",
  "set",
  "setflag",
  "vacation",
]);

const IDENTIFIER = /[A-Za-z_]\w*/uy;
const NUMBER = /\d+[KMGkmg]?/uy;
const SPACE = /[ \t\r\n]+/uy;

const wordKind = (word: string): CodeTokenKind => {
  const lower = word.toLowerCase();
  if (CONTROL.has(lower)) {
    return "keyword";
  }
  if (TESTS.has(lower)) {
    return "test";
  }
  return ACTIONS.has(lower) ? "action" : "plain";
};

const sticky = (pattern: RegExp, text: string, index: number): number => {
  pattern.lastIndex = index;
  return pattern.exec(text) === null ? index : pattern.lastIndex;
};

/** End of a quoted string from its opening quote, or -1 if unterminated. */
const quotedEnd = (text: string, start: number): number => {
  let index = start + 1;
  while (index < text.length) {
    const char = text.charAt(index);
    if (char === '"') {
      return index + 1;
    }
    index += char === "\\" ? 2 : 1;
  }
  return -1;
};

/** End of a `text:` string (after the line with only "."), or -1. */
const multilineEnd = (text: string, start: number): number => {
  let lineStart = text.indexOf("\n", start);
  while (lineStart !== -1) {
    const next = text.indexOf("\n", lineStart + 1);
    const line = text
      .slice(lineStart + 1, next === -1 ? text.length : next)
      .replace(/\r$/u, "");
    if (line === ".") {
      return next === -1 ? text.length : next + 1;
    }
    lineStart = next;
  }
  return -1;
};

interface Piece {
  readonly kind: CodeTokenKind;
  readonly end: number;
}

const readComment = (text: string, index: number): Piece => {
  if (text.charAt(index) === "#") {
    const lineEnd = text.indexOf("\n", index);
    return { end: lineEnd === -1 ? text.length : lineEnd, kind: "comment" };
  }
  const close = text.indexOf("*/", index + 2);
  return close === -1
    ? { end: text.length, kind: "invalid" }
    : { end: close + 2, kind: "comment" };
};

const readWord = (text: string, index: number): Piece => {
  const tag = text.charAt(index) === ":";
  const end = sticky(IDENTIFIER, text, tag ? index + 1 : index);
  if (end === (tag ? index + 1 : index)) {
    // A character Sieve doesn't have (or a lone ":").
    return { end: index + 1, kind: "invalid" };
  }
  if (tag) {
    return { end, kind: "tag" };
  }
  const word = text.slice(index, end);
  if (word.toLowerCase() === "text" && text.charAt(end) === ":") {
    const close = multilineEnd(text, end + 1);
    return close === -1
      ? { end: text.length, kind: "invalid" }
      : { end: close, kind: "string" };
  }
  return { end, kind: wordKind(word) };
};

const readPiece = (text: string, index: number): Piece => {
  const char = text.charAt(index);
  const space = sticky(SPACE, text, index);
  if (space > index) {
    return { end: space, kind: "plain" };
  }
  if (char === "#" || text.startsWith("/*", index)) {
    return readComment(text, index);
  }
  if (char === '"') {
    const end = quotedEnd(text, index);
    return end === -1
      ? { end: text.length, kind: "invalid" }
      : { end, kind: "string" };
  }
  if ("[](){},;".includes(char)) {
    return { end: index + 1, kind: "punct" };
  }
  if (/\d/u.test(char)) {
    return { end: sticky(NUMBER, text, index), kind: "number" };
  }
  return readWord(text, index);
};

/** Sieve text → coloured pieces that join back into exactly `text`. */
export const highlightSieve = (text: string): CodeToken[] => {
  const tokens: CodeToken[] = [];
  let index = 0;
  while (index < text.length) {
    const { end, kind } = readPiece(text, index);
    const piece = text.slice(index, end);
    const last = tokens.at(-1);
    // Neighbours of the same kind become one span (fewer DOM nodes).
    if (last !== undefined && last.kind === kind) {
      tokens[tokens.length - 1] = { kind, text: last.text + piece };
    } else {
      tokens.push({ kind, text: piece });
    }
    index = end;
  }
  return tokens;
};
