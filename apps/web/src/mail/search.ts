// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Search box syntax → JMAP Email/query filter (search runs on
// the server). Operators in Spanish and English, like most nebulamails:
//
//   de:bob  from:bob   para:ana  to:ana   cc:x   asunto:"informe anual"
//   tiene:adjunto  has:attachment   es:noleido  is:unread   es:destacado
//   antes:2026-10-01  before:…   despues:2026-09-01  after:…
//
// Everything else is free text (`text`: headers and body). Quoted phrases
// stay together and keep their quotes in `text`: Stalwart and the bridge
// search each word and "quoted phrase" on its own, all of them required.
// Unknown operators are treated as text, so "hora:10" still finds what the
// user typed.

import type {
  EmailFilter,
  EmailFilterCondition,
} from "@nebulamail/jmap-client";

type Field = "from" | "to" | "cc" | "subject";

// Maps, not objects: a search for "constructor" or "__proto__:x" must not
// find something on Object.prototype.
const FIELDS: ReadonlyMap<string, Field> = new Map<string, Field>([
  ["asunto", "subject"],
  ["cc", "cc"],
  ["de", "from"],
  ["from", "from"],
  ["para", "to"],
  ["subject", "subject"],
  ["to", "to"],
]);

const FLAGS: ReadonlyMap<string, EmailFilterCondition> = new Map<
  string,
  EmailFilterCondition
>([
  ["es:destacado", { hasKeyword: "$flagged" }],
  ["es:leido", { hasKeyword: "$seen" }],
  ["es:noleido", { notKeyword: "$seen" }],
  ["has:attachment", { hasAttachment: true }],
  ["is:flagged", { hasKeyword: "$flagged" }],
  ["is:read", { hasKeyword: "$seen" }],
  ["is:starred", { hasKeyword: "$flagged" }],
  ["is:unread", { notKeyword: "$seen" }],
  ["tiene:adjunto", { hasAttachment: true }],
]);

const DATES: ReadonlyMap<string, "before" | "after"> = new Map<
  string,
  "before" | "after"
>([
  ["after", "after"],
  ["antes", "before"],
  ["before", "before"],
  ["despues", "after"],
  ["después", "after"],
]);

/** Splits on spaces, keeping "quoted phrases" (also after an operator). */
export const tokenize = (query: string): string[] =>
  [...query.matchAll(/(?:[^\s"]+:)?"[^"]*"?|[^\s]+/gu)].map(
    (match) => match[0]
  );

const unquote = (value: string): string =>
  value.replaceAll(/^"|"$/gu, "").trim();

/** A free-text token: a phrase of several words keeps (closed) quotes. */
const freeText = (token: string): string => {
  if (!token.startsWith('"')) {
    return token;
  }
  const phrase = unquote(token).replaceAll(/\s+/gu, " ");
  return /\s/u.test(phrase) ? `"${phrase}"` : phrase;
};

const DATE = /^(?<date>\d{4}-\d{2}-\d{2})$/u;

/** `YYYY-MM-DD` as the start of that day in UTC, or null if not a real date. */
const toUtcDate = (value: string): string | null => {
  const date = DATE.exec(value)?.groups?.date;
  if (date === undefined) {
    return null;
  }
  const parsed = new Date(`${date}T00:00:00Z`);
  return Number.isNaN(parsed.getTime()) ||
    !parsed.toISOString().startsWith(date)
    ? null
    : parsed.toISOString().replace(".000Z", "Z");
};

const parseToken = (token: string): EmailFilterCondition | string => {
  const flag = FLAGS.get(token.toLowerCase().normalize("NFC"));
  if (flag !== undefined) {
    return flag;
  }
  // The name is normalised on its own: lower-casing and NFC can change the
  // length ("İ", a decomposed "é"), so the index is the original token's.
  const separator = token.indexOf(":");
  if (separator > 0) {
    const name = token.slice(0, separator).toLowerCase().normalize("NFC");
    const value = unquote(token.slice(separator + 1));
    const field = FIELDS.get(name);
    if (field !== undefined && value !== "") {
      return { [field]: value };
    }
    const dateField = DATES.get(name);
    const date = dateField === undefined ? null : toUtcDate(value);
    if (dateField !== undefined && date !== null) {
      return { [dateField]: date };
    }
  }
  return freeText(token);
};

/**
 * Returns the filter for a search, or null for an empty query. Repeated
 * operators (two `de:`) are combined with AND.
 */
export const parseSearch = (query: string): EmailFilter | null => {
  const conditions: EmailFilterCondition[] = [];
  const words: string[] = [];
  for (const token of tokenize(query)) {
    const parsed = parseToken(token);
    if (typeof parsed === "string") {
      if (parsed !== "") {
        words.push(parsed);
      }
    } else {
      conditions.push(parsed);
    }
  }
  if (words.length > 0) {
    conditions.push({ text: words.join(" ") });
  }
  if (conditions.length === 0) {
    return null;
  }
  const [only] = conditions;
  return conditions.length === 1 && only !== undefined
    ? only
    : { conditions, operator: "AND" };
};
