// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The only way user text gets into a Sieve script: as a quoted string
// (RFC 5228 §2.4.2). Inside one, only `"` and `\` are special, so escaping
// both makes it impossible to close the string and write commands.
//
// One more trap: Stalwart expands `${name}` (RFC 5229 variables) in every
// string, even when the script doesn't `require "variables"` (checked live:
// `fileinto "A ${x} B"` filed into "A  B"). Writing the brace
// as `\{` keeps the text literal there, and means the same `{` everywhere
// else (an unknown escape is just the character, RFC 5228 §2.4.2).

/** A Sieve quoted string with exactly the characters of `text`. */
export const quote = (text: string): string => {
  const escaped = text
    .toWellFormed()
    .replaceAll("\\", "\\\\")
    .replaceAll('"', '\\"')
    .replaceAll("${", "$\\{");
  return `"${escaped}"`;
};

const unicodeEscape = (code: number): string =>
  `\\u${code.toString(16).padStart(4, "0")}`;

/** A comment-safe, ASCII-only JSON string (a name inside a `#` comment). */
export const commentJson = (text: string): string => {
  const json = JSON.stringify(text.toWellFormed());
  let out = "";
  // Anything outside printable ASCII becomes \uXXXX (a surrogate pair
  // above U+FFFF), so the comment has no line break of any kind; "$" too,
  // so not even a comment has a `${`.
  for (const char of json) {
    const code = char.codePointAt(0) ?? 0;
    if (code >= 0x20 && code <= 0x7e && code !== 0x24) {
      out += char;
    } else if (code > 0xff_ff) {
      const offset = code - 0x1_00_00;
      out += unicodeEscape(0xd8_00 + Math.floor(offset / 0x4_00));
      out += unicodeEscape(0xdc_00 + (offset % 0x4_00));
    } else {
      out += unicodeEscape(code);
    }
  }
  return out;
};

/**
 * Every line break as CRLF, as Sieve requires: "CR and LF can only appear
 * in CRLF pairs" (RFC 5228 §8.1). Text areas give LF; a lone CR becomes
 * CRLF too.
 */
export const withCrlf = (script: string): string =>
  script.replaceAll(/\r\n|\r|\n/gu, "\r\n");
