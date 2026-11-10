// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Look-alike sender domains (like CVE-2019-15237 in Roundcube). With SMTPUTF8 a From can be
// `soporte@pаypal.com` with a Cyrillic "а": it looks like the real one,
// and DMARC passes for it (it is a real domain the attacker owns).
//
// Without a full confusables table, a simplified version of the rule
// browsers use for the address bar (Chrome's IDN policy):
//   - any domain not written in plain ASCII is shown with how it is really
//     written (punycode);
//   - it is a look-alike when a label mixes alphabets (`pаypal`), when it
//     has invisible characters or symbols that look like Latin letters
//     (`g〇〇gle`), or when every letter of a label is a Cyrillic, Greek…
//     letter that looks Latin (`аррӏе.com`) and the top-level domain isn't
//     one where that alphabet is normal (`.ru`, `.bg`, `.gr`…).
// A word in another alphabet (`пример.com`, `ελληνικά.gr`) or Latin with
// accents (`münchen.de`) is just international.

// The trailing slash: plain "punycode" is Node's deprecated built-in.
import { decode, encode } from "punycode/";

export type SenderDomainWarning =
  | {
      /** International (IDN) domain: it may not be the one it looks like. */
      readonly kind: "international";
      /** How it is really written (`xn--…`, or the ASCII it maps to). */
      readonly ascii: string;
    }
  | {
      /** Mixes alphabets, or imitates Latin letters: very likely a fake. */
      readonly kind: "lookalike";
      readonly ascii: string;
    };

type Script =
  | "latin"
  | "cyrillic"
  | "greek"
  | "armenian"
  | "cjk"
  | "lisu"
  | "cherokee"
  | "other";

const SCRIPTS: readonly (readonly [Script, RegExp])[] = [
  ["latin", /\p{Script=Latin}/u],
  ["cyrillic", /\p{Script=Cyrillic}/u],
  ["greek", /\p{Script=Greek}/u],
  ["armenian", /\p{Script=Armenian}/u],
  [
    "cjk",
    /[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Hangul}]/u,
  ],
  ["lisu", /\p{Script=Lisu}/u],
  ["cherokee", /\p{Script=Cherokee}/u],
];

/** Alphabets made mostly of Latin-looking letters (ꓑꓮꓬ, ᏢᎪᎽ): always fake. */
const ALL_LATIN_LOOKING: ReadonlySet<Script> = new Set(["lisu", "cherokee"]);

/**
 * Latin letters that pass for others (dotless ı, script ɑ and ɡ, small
 * capitals…): `gıthub`, `pɑypal`. Accents (é, ü) are normal and not here.
 */
const ODD_LATIN = /[ȷĸſƖǀ\u0250-\u02AF\u1D00-\u1D2B]/u;
/** Dotless ı: a normal letter in Turkish and Azerbaijani names. */
const DOTLESS_I = "ı";
const DOTLESS_I_TLDS: ReadonlySet<string> = new Set(["tr", "az"]);

/**
 * Letters that look like Latin ones (lower case; the domain is lowered
 * first). After Chrome's lists for whole-script confusables.
 */
const LATIN_LOOKING: Readonly<Partial<Record<Script, RegExp>>> = {
  armenian: /^[ագզհոռսցքօ]+$/u,
  cyrillic: /^[авекмнорстухіјѕԁӏԛԝһьъҽпгѵѡ]+$/u,
  greek: /^[αικνορτυχεηωϲϳ]+$/u,
};

/** Top-level domains where that alphabet is the normal one. */
const OWN_TLDS: Readonly<Partial<Record<Script, ReadonlySet<string>>>> = {
  armenian: new Set(["am", "xn--y9a3aq"]),
  cyrillic: new Set([
    // Chrome's list (url_formatter spoof checks): shared TLDs like .eu
    // or .me are not, a fake "apple" in Cyrillic is too easy there.
    "ru",
    "su",
    "by",
    "bg",
    "kz",
    "ua",
    "uz",
    "xn--p1ai",
    "xn--90ais",
    "xn--90ae",
    "xn--80ao21a",
    "xn--d1alf",
    "xn--90a3ac",
    "xn--j1amh",
    "xn--l1acc",
    "xn--e1a4c",
  ]),
  greek: new Set(["gr", "cy", "xn--qxam"]),
};

/** CJK symbols that look like Latin letters or a hyphen (〇, 一, ー…). */
const CJK_LATIN_LOOKING = /[〇一丨丿乙亅ーㄧㄨノヽ]/u;

const INVISIBLE = /[\p{Default_Ignorable_Code_Point}\p{Cf}]/u;
/** Letters of an alphabet (ー, ・ and marks are Common/Inherited: shared). */
const LETTER = /(?![\p{Script=Common}\p{Script=Inherited}])\p{L}/u;
/** Characters that end a host name for the URL parser (`a.com/日本`). */
const HOST_END = /[/\\?#@:[\]%\s]/u;
/** RFC 1035: 253 characters. Longer is never a real name (and parsing it is slow). */
const MAX_DOMAIN = 253;
/** Shown at most (the tooltip). */
const MAX_SHOWN = 80;
const DOTS = /[.。．｡]/u;

const scriptOf = (char: string): Script =>
  SCRIPTS.find(([, pattern]) => pattern.test(char))?.[0] ?? "other";

const lettersOf = (label: string): string => {
  let letters = "";
  for (const char of label) {
    if (LETTER.test(char)) {
      letters += char;
    }
  }
  return letters;
};

const scriptsOf = (letters: string): ReadonlySet<Script> => {
  const found = new Set<Script>();
  for (const char of letters) {
    found.add(scriptOf(char));
  }
  return found;
};

/** Several alphabets in one label (Latin with CJK is normal, 日本語abc). */
const mixes = (scripts: ReadonlySet<Script>): boolean =>
  scripts.size > 1 &&
  !(scripts.size === 2 && scripts.has("latin") && scripts.has("cjk"));

const looksLatin = (label: string, tld: string): boolean => {
  if (
    CJK_LATIN_LOOKING.test(label) &&
    scriptsOf(lettersOf(label)).has("latin")
  ) {
    return true;
  }
  const letters = lettersOf(label);
  if (
    ODD_LATIN.test(label) ||
    (label.includes(DOTLESS_I) && !DOTLESS_I_TLDS.has(tld))
  ) {
    return true;
  }
  const [script] = scriptsOf(letters);
  if (script === undefined || scriptsOf(letters).size !== 1) {
    return false;
  }
  if (ALL_LATIN_LOOKING.has(script)) {
    return true;
  }
  const pattern = LATIN_LOOKING[script];
  return (
    pattern !== undefined &&
    pattern.test(letters) &&
    OWN_TLDS[script]?.has(tld) !== true
  );
};

/** The domain with each character outside ASCII as `\u{…}`. */
const escaped = (domain: string): string => {
  let out = "";
  for (const char of domain) {
    out += /^[!-~]$/u.test(char)
      ? char
      : `\\u{${(char.codePointAt(0) ?? 0).toString(16).toUpperCase()}}`;
  }
  return out;
};

/**
 * Decodes one punycode label (without `xn--`, RFC 3492) with punycode.js;
 * null if it isn't valid. The browser has no toUnicode of its own. Its
 * errors (overflow, a non-ASCII basic part, a cut-off number) become null,
 * and so do a lone surrogate and a non-canonical label, which it lets
 * through.
 */
export const decodePunycode = (label: string): string | null => {
  try {
    const decoded = decode(label);
    // Only the one canonical spelling: punycode.js also decodes labels
    // that no encoder writes (browsers reject them).
    return decoded.isWellFormed() && encode(decoded) === label ? decoded : null;
  } catch {
    return null;
  }
};

/** RFC 1035 §2.3.4: a label is at most 63 octets. */
const MAX_LABEL = 63;

/** The labels as a person reads them: `xn--` ones decoded; null if invalid. */
const readableLabels = (host: string): readonly string[] | null => {
  const labels: string[] = [];
  for (const label of host.split(".")) {
    if (label.length > MAX_LABEL) {
      return null;
    }
    if (label.startsWith("xn--")) {
      const decoded = decodePunycode(label.slice(4));
      if (decoded === null || decoded === "") {
        return null;
      }
      labels.push(decoded.toLowerCase());
    } else if (label !== "") {
      labels.push(label);
    }
  }
  return labels;
};

/**
 * Why the domain of a sender's address deserves a look, or null when it
 * is plain ASCII. `email` is the address as the server gives it (Unicode
 * or punycode). Never throws.
 */
export const senderDomainWarning = (
  email: string
): SenderDomainWarning | null => {
  const written = email.slice(email.lastIndexOf("@") + 1).trim();
  // Before lower-casing: the Kelvin sign (U+212A) becomes a plain "k".
  const ascii = /^[!-~]*$/u.test(written);
  const domain = written.toLowerCase();
  // ASCII without punycode: nothing to say (fast path, most emails).
  if (ascii && !/(?:^|\.)xn--/u.test(domain)) {
    return null;
  }
  // The URL parser does IDNA (UTS #46): Unicode to punycode, with the
  // same mapping the browser and the DNS use. It refuses invisible
  // characters; full-width letters become plain ones.
  // Too long, or the parser would stop early: shown escaped, never as the
  // name it starts with (`paypal.com/日本` is not paypal.com).
  if (
    domain.length > MAX_DOMAIN ||
    HOST_END.test(domain) ||
    INVISIBLE.test(domain)
  ) {
    return { ascii: escaped(domain.slice(0, MAX_SHOWN)), kind: "lookalike" };
  }
  const parsed = URL.parse(`http://${domain}/`)?.hostname ?? null;
  if (parsed === null || parsed === "") {
    return { ascii: escaped(domain), kind: "lookalike" };
  }
  // A label longer than DNS allows can't be a real name, written either way.
  if (parsed.split(".").some((label) => label.length > MAX_LABEL)) {
    return { ascii: parsed.slice(0, MAX_SHOWN), kind: "lookalike" };
  }
  // Written with other characters but really a plain ASCII name
  // (ｐａｙｐａｌ, 𝐩𝐚𝐲𝐩𝐚𝐥, paypaⅼ with U+217C, Kodak with a Kelvin K): it
  // only looks like that name on purpose.
  if (!ascii && !/(?:^|\.)xn--/u.test(parsed)) {
    return { ascii: parsed, kind: "lookalike" };
  }
  // Already punycode, or as the parser left it: analysed decoded, as a
  // person would read it once shown in Unicode (other apps, the browser).
  const labels = ascii
    ? readableLabels(parsed)
    : domain.split(DOTS).filter((label) => label !== "");
  if (labels === null || labels.some((label) => INVISIBLE.test(label))) {
    return { ascii: parsed.slice(0, MAX_SHOWN), kind: "lookalike" };
  }
  const tld = parsed.split(".").findLast((label) => label !== "") ?? "";
  const lookalike = labels.some(
    (label) => mixes(scriptsOf(lettersOf(label))) || looksLatin(label, tld)
  );
  return { ascii: parsed, kind: lookalike ? "lookalike" : "international" };
};
