// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Sender verification from the Authentication-Results header our server
// added (RFC 8601). Dangers and failure cases of the green
// badge: docs/verified-sender.md — read it before changing this file.
//
// What "pass" means: DMARC passed for the domain in From, so that domain
// really sent it. Only DMARC checks that DKIM/SPF match the From domain;
// "DKIM and SPF pass" alone can be for the attacker's own domain, so it
// gives no badge. The pass must be for the From we show: a `dmarc=pass`
// whose `header.from` (lowercase, no final dot, IDNA) is exactly the domain
// of the only From address. A pass for another domain, or without
// `header.from`, is ignored (two From headers, or a From we parse
// differently from the server, would otherwise borrow another domain's
// pass). It never means the person is honest: look-alike domains, free mail
// accounts and hacked accounts all pass. "fail": any DMARC fail, or a check
// failed and nothing passed. Anything else: no verdict (no badge rather
// than a wrong one). More than one From address: no "pass" (DMARC checks
// one domain).
//
// Trust model (checked against Stalwart): Stalwart puts
// its own header on top (authserv-id = its hostname) but keeps the ones
// senders write, even ones claiming its name. By default it only adds its
// header to mail arriving on port 25, so mail from a local user would have
// the sender's header on top; docker/setup.ts turns on
// `addAuthResultsHeader` for every path ("localhost; none" on local mail).
// So: read only the topmost header, and only if its authserv-id is our
// server's hostname. Not covered, and so the green badge is off unless the
// user turns it on in Settings or the admin forces it with
// VERIFIED_SENDER_BADGE (shownVerdict, use-verified-badge.ts):
// - servers that don't add their header to all mail (a local user can
//   forge "pass");
// - mail received before the server option was turned on;
// - mail imported into the mailbox (IMAP APPEND, Email/import, migrations),
//   which keeps whatever headers it had.
// Warnings on "fail" are always shown: a forged fail only makes an email
// look worse. But no warning doesn't mean the email is safe.

export type AuthVerdict = "pass" | "fail";

type Method = "dkim" | "spf" | "dmarc";

const METHODS: readonly Method[] = ["dkim", "spf", "dmarc"];

const FAILURES = new Set(["fail", "permerror", "softfail"]);

/**
 * One lexical token of the header (RFC 8601 §2.2): a word (RFC 2045 token,
 * dot-atom), the content of a quoted-string, or one of the specials that
 * structure it. Folding white space and comments only separate tokens.
 */
type Token =
  | { readonly kind: "word" | "quoted"; readonly text: string }
  | { readonly kind: ";" | "=" | "@" };

const SPACE = /\s/u;

/**
 * The index after the comment that opens at `start` (RFC 5322 §3.2.2:
 * nested, with quoted-pairs), or -1 if it isn't closed. Indexes are UTF-16
 * units: every delimiter is ASCII, so a surrogate pair never matches one.
 */
const commentEnd = (header: string, start: number): number => {
  let depth = 1;
  let index = start + 1;
  while (depth > 0 && index < header.length) {
    const char = header[index];
    index += char === "\\" ? 2 : 1;
    if (char === "(") {
      depth += 1;
    } else if (char === ")") {
      depth -= 1;
    }
  }
  return depth === 0 ? index : -1;
};

/**
 * The quoted-string that opens at `start` (RFC 5322 §3.2.4): its content
 * with quoted-pairs undone, and the index after it; null if not closed.
 */
const quotedString = (
  header: string,
  start: number
): { readonly text: string; readonly next: number } | null => {
  let text = "";
  let index = start + 1;
  while (index < header.length) {
    const char = header[index] ?? "";
    if (char === '"') {
      return { next: index + 1, text };
    }
    if (char === "\\") {
      index += 1;
      text += header[index] ?? "";
    } else {
      text += char;
    }
    index += 1;
  }
  return null;
};

/**
 * Splits the header into tokens. Quoted-strings and comments are read in
 * one pass, each with its quoted-pairs (RFC 5322 §3.2.1), so a `;`, `(` or
 * `"` inside one never ends a result or hides the next. Null if a quote or
 * comment isn't closed, or a `)` closes nothing: the header is then not
 * trusted at all.
 */
const lex = (header: string): Token[] | null => {
  const tokens: Token[] = [];
  let word = "";
  const endWord = (): void => {
    if (word !== "") {
      tokens.push({ kind: "word", text: word });
      word = "";
    }
  };
  let index = 0;
  while (index < header.length) {
    const char = header[index] ?? "";
    if (char === "(" || char === '"') {
      endWord();
      const quoted = char === '"' ? quotedString(header, index) : null;
      const next =
        char === "(" ? commentEnd(header, index) : (quoted?.next ?? -1);
      if (next === -1) {
        return null;
      }
      if (quoted !== null) {
        tokens.push({ kind: "quoted", text: quoted.text });
      }
      index = next;
      continue;
    }
    if (char === ")") {
      return null;
    }
    if (char === ";" || char === "=" || char === "@") {
      endWord();
      tokens.push({ kind: char });
    } else if (SPACE.test(char)) {
      endWord();
    } else {
      word += char;
    }
    index += 1;
  }
  endWord();
  return tokens;
};

/** One `ptype.property=pvalue` of a result (RFC 8601 §2.3). */
interface Property {
  /** `ptype.property`, lowercase (`header.from`, `smtp.mailfrom`). */
  readonly name: string;
  /** The pvalue: a value, or `local-part@domain` / `@domain`. */
  readonly value: string;
}

/** One resinfo: `method=result` and its properties (RFC 8601 §2.2). */
interface ResInfo {
  readonly method: string;
  readonly result: string;
  readonly properties: readonly Property[];
}

/** The parsed header: authserv-id and every result. */
interface Parsed {
  /** Lowercase, or "" if missing. */
  readonly authservId: string;
  readonly results: readonly ResInfo[];
}

const KEYWORD = /^[a-z\d](?:[a-z\d-]*[a-z\d])?$/iu;

const PROPERTY_NAME = /^[a-z\d-]+\.[a-z\d-]+$/iu;

/** The text of a word or quoted-string token, else undefined. */
const textOf = (token: Token | undefined): string | undefined =>
  token?.kind === "word" || token?.kind === "quoted" ? token.text : undefined;

/**
 * Reads a pvalue at `start` (RFC 8601 §2.2: `value / [[local-part] "@"]
 * domain-name`). Returns the value and the index after it.
 */
const readPvalue = (
  tokens: readonly Token[],
  start: number
): { readonly value: string; readonly next: number } | null => {
  const first = tokens[start];
  if (first?.kind === "@") {
    const domain = textOf(tokens[start + 1]);
    return domain === undefined
      ? null
      : { next: start + 2, value: `@${domain}` };
  }
  const text = textOf(first);
  if (text === undefined) {
    return null;
  }
  const domain = textOf(tokens[start + 2]);
  if (tokens[start + 1]?.kind === "@" && domain !== undefined) {
    return { next: start + 3, value: `${text}@${domain}` };
  }
  return { next: start + 1, value: text };
};

/** One resinfo from its tokens; null for `none` or anything unreadable. */
const readResInfo = (tokens: readonly Token[]): ResInfo | null => {
  // methodspec: method ["/" version] "=" result. "/" is a word character
  // here (header.b values contain it), so the version stays in the word.
  const method = (textOf(tokens[0]) ?? "").split("/")[0] ?? "";
  const result = tokens[2]?.kind === "word" ? tokens[2].text : "";
  if (
    tokens[0]?.kind !== "word" ||
    tokens[1]?.kind !== "=" ||
    !KEYWORD.test(method) ||
    !KEYWORD.test(result)
  ) {
    return null;
  }
  const properties: Property[] = [];
  let index = 3;
  while (index < tokens.length) {
    const name =
      tokens[index]?.kind === "word" ? textOf(tokens[index]) : undefined;
    const pvalue =
      name !== undefined && tokens[index + 1]?.kind === "="
        ? readPvalue(tokens, index + 2)
        : null;
    if (name === undefined || pvalue === null) {
      index += 1;
      continue;
    }
    // reasonspec ("reason=value") isn't a property; skip it too.
    if (PROPERTY_NAME.test(name)) {
      properties.push({ name: name.toLowerCase(), value: pvalue.value });
    }
    index = pvalue.next;
  }
  return {
    method: method.toLowerCase(),
    properties,
    result: result.toLowerCase(),
  };
};

/**
 * Parses an Authentication-Results value (RFC 8601 §2.2): authserv-id
 * (a value: token or quoted-string), optional version, then resinfos split
 * on the `;` that are outside quoted-strings and comments.
 */
const parse = (header: string): Parsed | null => {
  const tokens = lex(header);
  if (tokens === null) {
    return null;
  }
  const segments: Token[][] = [[]];
  for (const token of tokens) {
    if (token.kind === ";") {
      segments.push([]);
    } else {
      segments.at(-1)?.push(token);
    }
  }
  const [head = [], ...resinfos] = segments;
  return {
    authservId: (textOf(head[0]) ?? "").toLowerCase(),
    results: resinfos.flatMap((part) => {
      const info = readResInfo(part);
      return info === null ? [] : [info];
    }),
  };
};

/**
 * Results per method. A method that appears more than once (two DKIM
 * signatures) counts as passed if any instance passed.
 */
export const parseAuthResults = (
  header: string
): Partial<Record<Method, string>> => {
  const results: Partial<Record<Method, string>> = {};
  for (const { method, result } of parse(header)?.results ?? []) {
    const key = METHODS.find((known) => known === method);
    if (key !== undefined && results[key] !== "pass") {
      results[key] = result;
    }
  }
  return results;
};

/** The authserv-id: the first token before the first ";" (lowercase). */
export const authservId = (header: string): string | null => {
  const id = parse(header)?.authservId ?? "";
  return id === "" ? null : id;
};

/** One `dmarc=…` result and the domain it was for (`header.from`). */
interface DmarcResult {
  readonly result: string;
  /** Normalized `header.from`, or null if missing or not a domain. */
  readonly headerFrom: string | null;
}

const HOST_FORBIDDEN = /[\s@/\\:?#[\]%\p{Cc}]/u;

const ASCII_HOST = /^(?:[a-z\d-]+\.)*[a-z\d-]+$/u;

/**
 * A domain in the form we compare: trimmed, lowercase, one final dot
 * removed and in ASCII (IDNA, via the URL parser). Null if it isn't a
 * host name.
 */
export const normalizeDomain = (raw: string): string | null => {
  let domain = raw.trim().toLowerCase();
  if (domain.endsWith(".")) {
    domain = domain.slice(0, -1);
  }
  if (domain === "" || HOST_FORBIDDEN.test(domain)) {
    return null;
  }
  try {
    const { hostname } = new URL(`http://${domain}/`);
    return ASCII_HOST.test(hostname) ? hostname : null;
  } catch {
    return null;
  }
};

/** The normalized domain of an address (after the last "@"). */
export const fromDomain = (address: string): string | null => {
  const at = address.lastIndexOf("@");
  return at === -1 ? null : normalizeDomain(address.slice(at + 1));
};

/** The value of the first property `name` of a result, if any. */
const property = (info: ResInfo, name: string): string | undefined =>
  info.properties.find((found) => found.name === name)?.value;

/** Every `dmarc=…` result of the header. */
const dmarcResults = (results: readonly ResInfo[]): DmarcResult[] =>
  results.flatMap((info) => {
    if (info.method !== "dmarc") {
      return [];
    }
    const value = property(info, "header.from");
    return [
      {
        headerFrom: value === undefined ? null : normalizeDomain(value),
        result: info.result,
      },
    ];
  });

/**
 * The verdict for an email. `from`: the addresses of its From header (what
 * the reader shows).
 */
export const authVerdict = (
  header: string | null,
  serverHostname: string,
  from: readonly string[]
): AuthVerdict | null => {
  if (header === null || header.trim() === "") {
    return null;
  }
  const parsed = parse(header);
  if (
    parsed === null ||
    parsed.authservId === "" ||
    parsed.authservId !== serverHostname.toLowerCase()
  ) {
    return null;
  }
  const dmarc = dmarcResults(parsed.results);
  if (dmarc.some(({ result }) => result === "fail")) {
    return "fail";
  }
  const [only, ...others] = from;
  const domain =
    only === undefined || others.length > 0 ? null : fromDomain(only);
  if (
    domain !== null &&
    dmarc.some(
      ({ headerFrom, result }) => result === "pass" && headerFrom === domain
    )
  ) {
    return "pass";
  }
  const { dkim, spf } = parseAuthResults(header);
  const failed = [dkim, spf].some(
    (result) => result !== undefined && FAILURES.has(result)
  );
  const passed = dkim === "pass" || spf === "pass";
  return failed && !passed ? "fail" : null;
};

/** A DKIM signature our server verified (`dkim=pass header.d=… header.s=… header.b=…`). */
export interface DkimPass {
  /** Signing domain (`header.d`), lowercase. */
  readonly domain: string;
  /** Selector (`header.s`), if the server wrote it. */
  readonly selector: string | null;
  /** First characters of the signature (`header.b`, RFC 6008), if written. */
  readonly signature: string | null;
}

/**
 * DKIM signatures that passed, from the topmost Authentication-Results,
 * and only if our server wrote it (same trust rule as `authVerdict`).
 */
export const dkimPasses = (
  header: string | null,
  serverHostname: string
): DkimPass[] => {
  const parsed = header === null ? null : parse(header);
  if (
    parsed === null ||
    parsed.authservId === "" ||
    parsed.authservId !== serverHostname.toLowerCase()
  ) {
    return [];
  }
  return parsed.results.flatMap((info) => {
    if (info.method !== "dkim" || info.result !== "pass") {
      return [];
    }
    const d = property(info, "header.d");
    const s = property(info, "header.s");
    const b = property(info, "header.b");
    return d === undefined
      ? []
      : [
          {
            domain: d.toLowerCase(),
            selector: s?.toLowerCase() ?? null,
            signature: b ?? null,
          },
        ];
  });
};

/**
 * What the UI may show: "fail" always; "pass" only if the badge is on
 * (setting or VERIFIED_SENDER_BADGE): the server adds its header to all mail.
 */
export const shownVerdict = (
  verdict: AuthVerdict | null,
  badgeEnabled: boolean
): AuthVerdict | null => (verdict === "pass" && !badgeEnabled ? null : verdict);
