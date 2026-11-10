// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// RFC 5228 §2 lexical structure: comments, strings (quoted and `text:`),
// numbers with K/M/G, tags, identifiers and punctuation.

// --- Tokens -----------------------------------------------------------------

export type Token =
  | { readonly type: "identifier"; readonly value: string }
  | { readonly type: "tag"; readonly value: string }
  | { readonly type: "string"; readonly value: string }
  | { readonly type: "number"; readonly value: number }
  | { readonly type: "comment"; readonly value: string }
  | { readonly type: "punct"; readonly value: string };

export class SieveSyntaxError extends Error {
  readonly offset: number;
  constructor(message: string, offset: number) {
    super(message);
    this.name = "SieveSyntaxError";
    this.offset = offset;
  }
}

const IDENTIFIER = /[A-Za-z_][\w]*/uy;
const NUMBER = /(?<digits>\d+)(?<unit>[KMG]?)/iuy;
const MULTIPLIERS: Readonly<Record<string, number>> = {
  "": 1,
  g: 1024 ** 3,
  k: 1024,
  m: 1024 ** 2,
};

/** Quoted string body from `start` (after the quote); RFC 5228 §2.4.2. */
const readQuoted = (
  text: string,
  start: number
): { readonly value: string; readonly end: number } => {
  let value = "";
  let index = start;
  while (index < text.length) {
    const char = text.charAt(index);
    if (char === '"') {
      return { end: index + 1, value };
    }
    if (char === "\\") {
      // Any escaped character stands for itself.
      value += text.charAt(index + 1);
      index += 2;
    } else {
      value += char;
      index += 1;
    }
  }
  throw new SieveSyntaxError("unterminated string", start);
};

/** `text:` multi-line string (RFC 5228 §2.4.2): until a line with ".". */
const readMultiline = (
  text: string,
  start: number
): { readonly value: string; readonly end: number } => {
  const lineEnd = text.indexOf("\n", start);
  if (lineEnd === -1) {
    throw new SieveSyntaxError("unterminated text:", start);
  }
  const lines: string[] = [];
  let index = lineEnd + 1;
  while (index <= text.length) {
    const next = text.indexOf("\n", index);
    const raw = text.slice(index, next === -1 ? text.length : next);
    const line = raw.replace(/\r$/u, "");
    if (line === ".") {
      return {
        end: next === -1 ? text.length : next + 1,
        value: lines.join("\n"),
      };
    }
    // Dot-stuffing: a leading ".." stands for ".".
    lines.push(line.startsWith("..") ? line.slice(1) : line);
    if (next === -1) {
      break;
    }
    index = next + 1;
  }
  throw new SieveSyntaxError("unterminated text:", start);
};

interface Read {
  readonly token: Token | null;
  readonly end: number;
}

/** `#…` to the end of the line, or a block comment. */
const readComment = (text: string, index: number): Read => {
  if (text.charAt(index) === "#") {
    const lineEnd = text.indexOf("\n", index);
    const stop = lineEnd === -1 ? text.length : lineEnd;
    const value = text.slice(index + 1, stop).replace(/\r$/u, "");
    return { end: stop, token: { type: "comment", value } };
  }
  const close = text.indexOf("*/", index + 2);
  if (close === -1) {
    throw new SieveSyntaxError("unterminated comment", index);
  }
  const value = text.slice(index + 2, close);
  return { end: close + 2, token: { type: "comment", value } };
};

const readNumber = (text: string, index: number): Read => {
  NUMBER.lastIndex = index;
  const match = NUMBER.exec(text);
  const digits = Number(match?.groups?.digits ?? Number.NaN);
  const unit = MULTIPLIERS[(match?.groups?.unit ?? "").toLowerCase()] ?? 1;
  if (match === null || !Number.isSafeInteger(digits * unit)) {
    throw new SieveSyntaxError("bad number", index);
  }
  return {
    end: NUMBER.lastIndex,
    token: { type: "number", value: digits * unit },
  };
};

/** A tag (`:contains`), an identifier, or a `text:` string. */
const readWord = (text: string, index: number): Read => {
  const tag = text.charAt(index) === ":";
  IDENTIFIER.lastIndex = tag ? index + 1 : index;
  const match = IDENTIFIER.exec(text);
  if (match === null) {
    throw new SieveSyntaxError(`unexpected "${text.charAt(index)}"`, index);
  }
  const word = match[0].toLowerCase();
  if (tag) {
    return { end: IDENTIFIER.lastIndex, token: { type: "tag", value: word } };
  }
  if (word === "text" && text.charAt(IDENTIFIER.lastIndex) === ":") {
    const { end, value } = readMultiline(text, IDENTIFIER.lastIndex + 1);
    return { end, token: { type: "string", value } };
  }
  return {
    end: IDENTIFIER.lastIndex,
    token: { type: "identifier", value: word },
  };
};

/** One token (or `null` for whitespace) at `index`, and where it ends. */
const readToken = (text: string, index: number): Read => {
  const char = text.charAt(index);
  if (char === " " || char === "\t" || char === "\r" || char === "\n") {
    return { end: index + 1, token: null };
  }
  if (char === "#" || text.startsWith("/*", index)) {
    return readComment(text, index);
  }
  if (char === '"') {
    const { end, value } = readQuoted(text, index + 1);
    return { end, token: { type: "string", value } };
  }
  if ("[](){},;".includes(char)) {
    return { end: index + 1, token: { type: "punct", value: char } };
  }
  return /\d/u.test(char) ? readNumber(text, index) : readWord(text, index);
};

/** Any Sieve text → tokens, comments included (throws SieveSyntaxError). */
export const tokenize = (text: string): Token[] => {
  const tokens: Token[] = [];
  let index = 0;
  while (index < text.length) {
    const { end, token } = readToken(text, index);
    if (token !== null) {
      tokens.push(token);
    }
    index = end;
  }
  return tokens;
};
