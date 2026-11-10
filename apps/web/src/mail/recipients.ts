// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// What the user types in To/Cc/Bcc → recipients. Accepts "ana@x.com",
// "Ana Pérez <ana@x.com>" and lists separated by commas, semicolons or new
// lines (a paste from a spreadsheet or another client).

import type { Identity, Recipient } from "./compose";

const LINE_BREAK = /[\n\r\u2028\u2029]/u;

/**
 * "Name <address>" → its two parts, or null. By hand, not with a regex
 * like `^(.*?)\s*<([^<>]*)>$`, which backtracks quadratically: a hostile
 * `mailto:` with 60,000 spaces took seconds. Same
 * result: the last `<`, nothing but the closing `>` after it, and a name
 * on one line.
 */
const splitAngle = (
  text: string
): { readonly name: string; readonly email: string } | null => {
  if (!text.endsWith(">")) {
    return null;
  }
  const open = text.lastIndexOf("<");
  const email = text.slice(open + 1, -1);
  const name = text.slice(0, Math.max(open, 0)).trimEnd();
  return open === -1 || email.includes(">") || LINE_BREAK.test(name)
    ? null
    : { email, name };
};

const unquote = (text: string): string =>
  text
    .trim()
    .replace(/^"(?<inner>.*)"$/u, "$<inner>")
    .trim();

/** One address, with or without a display name; null if empty. */
export const parseRecipient = (text: string): Recipient | null => {
  const trimmed = text.trim();
  if (trimmed === "") {
    return null;
  }
  const match = splitAngle(trimmed);
  if (match === null) {
    return { email: trimmed.replace(/^mailto:/iu, "") };
  }
  const email = match.email.trim();
  const name = unquote(match.name);
  return name === "" ? { email } : { email, name };
};

/**
 * Splits typed text into finished entries and the part still being typed.
 * Separators inside quotes or angle brackets don't count ("Pérez, Ana"
 * <ana@x.com> is one entry).
 */
export const splitRecipientInput = (
  text: string
): { readonly entries: readonly Recipient[]; readonly rest: string } => {
  const entries: Recipient[] = [];
  let current = "";
  let quoted = false;
  let angle = false;
  for (const char of text) {
    if (char === '"') {
      quoted = !quoted;
    } else if (char === "<" && !quoted) {
      angle = true;
    } else if (char === ">" && !quoted) {
      angle = false;
    }
    if (!quoted && !angle && /[,;\n]/u.test(char)) {
      const parsed = parseRecipient(current);
      if (parsed !== null) {
        entries.push(parsed);
      }
      current = "";
    } else {
      current += char;
    }
  }
  return { entries, rest: current.trimStart() };
};

/** Same address (case-insensitive), for de-duplicating recipients. */
export const sameAddress = (a: string, b: string): boolean =>
  a.trim().toLowerCase() === b.trim().toLowerCase();

/** Adds recipients that aren't there yet. */
export const mergeRecipients = (
  current: readonly Recipient[],
  added: readonly Recipient[]
): Recipient[] => {
  // A set, not a search per address: a reply-all to a To of 40,000
  // addresses (the sender writes it) must not take minutes.
  const result = [...current];
  const seen = new Set(
    current.map((recipient) => recipient.email.trim().toLowerCase())
  );
  for (const recipient of added) {
    const key = recipient.email.trim().toLowerCase();
    if (!seen.has(key)) {
      seen.add(key);
      result.push(recipient);
    }
  }
  return result;
};

/**
 * The Bcc when the sender changes from `previous` to `next`: RFC 8621 §6
 * Identity `bcc` is what the client SHOULD add to emails from it, so the
 * previous identity's addresses go and the next one's are added, once. The
 * ones the user added stay. `previous` null: the first sender of a new
 * email.
 */
export const swapIdentityBcc = (
  bcc: readonly Recipient[],
  previous: Pick<Identity, "bcc"> | null,
  next: Pick<Identity, "bcc"> | null
): Recipient[] => {
  const old = new Set(
    (previous?.bcc ?? []).map((recipient) =>
      recipient.email.trim().toLowerCase()
    )
  );
  return mergeRecipients(
    bcc.filter((recipient) => !old.has(recipient.email.trim().toLowerCase())),
    next?.bcc ?? []
  );
};
