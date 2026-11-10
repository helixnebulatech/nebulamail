// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Visual filters → Sieve (RFC 5228 and extensions). The output is meant to
// be read by people and parsed back by parse.ts: one `if` per rule, with a
// `# Rule: "name"` comment; disabled rules sit inside `if false { … }`.
// Every value goes through `quote` (sieve-string.ts).

import type { Action, Condition, FilterSet, Rule } from "./model";
import { MAX_RULE_NAME, MAX_VALUE, cleanLine, sortActions } from "./model";
import { commentJson, quote } from "./sieve-string";

/** The name of the script our visual editor writes. */
export const OUR_SCRIPT_NAME = "nebula-mail-filters";

/** First line of our scripts: how parse.ts recognizes them. */
export const SCRIPT_MARKER = "# Nebula Mail filters (format 1).";
const HEADER = [
  SCRIPT_MARKER,
  "# Made in Settings > Filters. If you change this script by hand, Nebula",
  "# Mail shows it as a script instead of rules and never overwrites it",
  "# without asking.",
];

/** Header names of each field (Sieve compares them case-insensitively). */
const FIELD_HEADERS = {
  from: ["from"],
  listId: ["list-id"],
  recipient: ["to", "cc"],
  subject: ["subject"],
} as const;

const stringList = (values: readonly string[]): string =>
  values.length === 1
    ? quote(values[0] ?? "")
    : `[${values.map(quote).join(", ")}]`;

const value = (text: string): string => quote(cleanLine(text, MAX_VALUE));

const testFor = (condition: Condition): string => {
  switch (condition.kind) {
    case "match": {
      const op = `:${condition.op}`;
      const not = condition.negate ? "not " : "";
      if (condition.field === "body") {
        return `${not}body :text ${op} ${value(condition.value)}`;
      }
      const headers =
        condition.field === "header"
          ? [cleanLine(condition.header, 76)]
          : FIELD_HEADERS[condition.field];
      // Addresses for exact and wildcard matches ("*@shop.com"); the whole
      // header (name and address) for "contains" and regexes.
      const test =
        (condition.field === "from" || condition.field === "recipient") &&
        (condition.op === "is" || condition.op === "matches")
          ? "address"
          : "header";
      return `${not}${test} ${op} ${stringList(headers)} ${value(condition.value)}`;
    }
    case "size": {
      return `size :${condition.op} ${String(Math.trunc(condition.kilobytes))}K`;
    }
    case "attachment": {
      const not = condition.negate ? "not " : "";
      return `${not}header :mime :anychild :contains "content-disposition" "attachment"`;
    }
    default: {
      return condition satisfies never;
    }
  }
};

/**
 * How folders are named in `fileinto`: by id (`:mailboxid`, RFC 9042, with
 * the path as fallback) where the server has `mailboxid` (Stalwart's JMAP),
 * else by path alone (the bridge: its mailbox ids aren't the server's).
 */
export interface GenerateOptions {
  readonly mailboxIds: boolean;
}

const BY_ID: GenerateOptions = { mailboxIds: true };

/** A folder action written with `:mailboxid` (else by path only). */
const byId = (
  action: Extract<Action, { kind: "move" | "copy" }>,
  options: GenerateOptions
): boolean => options.mailboxIds && action.mailboxId !== "";

const commandFor = (action: Action, options: GenerateOptions): string => {
  switch (action.kind) {
    case "markRead": {
      return 'addflag "\\\\Seen";';
    }
    case "star": {
      return 'addflag "\\\\Flagged";';
    }
    case "keyword": {
      return `addflag ${value(action.keyword.toLowerCase())};`;
    }
    case "copy":
    case "move": {
      const copy = action.kind === "copy" ? ":copy " : "";
      return byId(action, options)
        ? `fileinto ${copy}:mailboxid ${value(action.mailboxId)} ${value(action.mailboxName)};`
        : `fileinto ${copy}${value(action.mailboxName)};`;
    }
    case "forward": {
      const copy = action.keepCopy ? ":copy " : "";
      return `redirect ${copy}${value(action.address)};`;
    }
    case "reject": {
      return `reject ${value(action.reason)};`;
    }
    case "discard": {
      return "discard;";
    }
    case "stop": {
      return "stop;";
    }
    default: {
      return action satisfies never;
    }
  }
};

const ruleTest = (rule: Rule): string => {
  const tests = rule.conditions.map(testFor);
  if (tests.length === 0) {
    return "true";
  }
  if (tests.length === 1) {
    return tests[0] ?? "true";
  }
  return `${rule.match === "all" ? "allof" : "anyof"} (${tests.join(", ")})`;
};

/**
 * A rule that rejects ends the script: a later `reject`, `fileinto` or
 * `redirect` would be a run-time error (RFC 5429 §2.4: one reject per
 * script, and it is not recommended with delivery), and a run-time error
 * means implicit keep (RFC 5228 §2.10.6). parse.ts drops this `stop` again.
 */
export const rejects = (rule: Rule): boolean =>
  rule.actions.some((action) => action.kind === "reject");

/** Moves, copies or forwards: the message is delivered somewhere. */
const delivers = (rule: Rule): boolean =>
  rule.actions.some(
    (action) =>
      action.kind === "move" ||
      action.kind === "copy" ||
      action.kind === "forward"
  );

/**
 * The tests of the enabled rules before `index` that deliver the message
 * (`fileinto`, `redirect`) and go on (no `stop`: one that stops never
 * reaches a later rule). A rule that rejects only acts when none of them
 * matched: rejecting a message that was already filed or forwarded is NOT
 * RECOMMENDED (RFC 5429 §2.4) and a server may refuse it at run time,
 * which keeps it in the inbox (RFC 5228 §2.10.6). So the earlier rule
 * wins, as with "stop". Sieve tests have no side effects, so testing them
 * again is safe.
 */
const deliveredBefore = (rules: readonly Rule[], index: number): string[] =>
  rules
    .slice(0, index)
    .filter(
      (rule) =>
        rule.enabled &&
        delivers(rule) &&
        !rule.actions.some((action) => action.kind === "stop")
    )
    .map(ruleTest);

const anyOf = (tests: readonly string[]): string =>
  tests.length === 1 ? (tests[0] ?? "true") : `anyof (${tests.join(", ")})`;

const ruleLines = (
  rule: Rule,
  options: GenerateOptions,
  earlier: readonly string[]
): string[] => {
  const actions = sortActions(rule.actions);
  if (rejects(rule) && !actions.some((action) => action.kind === "stop")) {
    actions.push({ kind: "stop" });
  }
  const commands = actions.map((action) => commandFor(action, options));
  // parse.ts recognizes and drops this guard (`if not … { … }`).
  const body =
    rejects(rule) && earlier.length > 0
      ? [
          `if not ${anyOf(earlier)} {`,
          ...commands.map((line) => `  ${line}`),
          "}",
        ]
      : commands;
  const block = [
    `if ${ruleTest(rule)} {`,
    ...body.map((line) => `  ${line}`),
    "}",
  ];
  const comment = `# Rule: ${commentJson(cleanLine(rule.name, MAX_RULE_NAME))}`;
  return rule.enabled
    ? [comment, ...block]
    : [comment, "if false {", ...block.map((line) => `  ${line}`), "}"];
};

const addFolderExtensions = (
  needed: Set<string>,
  action: Extract<Action, { kind: "move" | "copy" }>,
  options: GenerateOptions
): void => {
  if (action.kind === "copy") {
    needed.add("copy");
  }
  needed.add("fileinto");
  if (byId(action, options)) {
    needed.add("mailboxid");
  }
};

/** The extensions a set of rules needs, sorted (RFC 5228 §3.2). */
export const requiredExtensions = (
  set: FilterSet,
  options: GenerateOptions = BY_ID
): string[] => {
  const needed = new Set<string>();
  if (set.vacation) {
    needed.add("include");
  }
  for (const rule of set.rules) {
    for (const condition of rule.conditions) {
      if (condition.kind === "match") {
        if (condition.field === "body") {
          needed.add("body");
        }
        if (condition.op === "regex") {
          needed.add("regex");
        }
      } else if (condition.kind === "attachment") {
        needed.add("mime");
      }
    }
    for (const action of rule.actions) {
      switch (action.kind) {
        case "markRead":
        case "star":
        case "keyword": {
          needed.add("imap4flags");
          break;
        }
        case "copy":
        case "move": {
          addFolderExtensions(needed, action, options);
          break;
        }
        case "forward": {
          if (action.keepCopy) {
            needed.add("copy");
          }
          break;
        }
        case "reject": {
          needed.add("reject");
          break;
        }
        case "discard":
        case "stop": {
          break;
        }
        default: {
          action satisfies never;
        }
      }
    }
  }
  return [...needed].toSorted();
};

const INCLUDE_VACATION = 'include :personal :optional "vacation";';

/**
 * The vacation response runs first, but never for a message that an
 * enabled rule rejects: "reject" MUST be incompatible with "vacation"
 * (RFC 5429 §2.4), so both would be a run-time error and an implicit keep
 * (RFC 5228 §2.10.6). The test is that of the rejecting rules; one that an
 * earlier `stop` skips also skips the vacation response (no error either).
 */
const vacationLines = (set: FilterSet): string[] => {
  // When a rule really rejects: its test, and none of the earlier rules
  // that deliver matched (`deliveredBefore`).
  const tests = set.rules.flatMap((rule, index) => {
    if (!rule.enabled || !rejects(rule)) {
      return [];
    }
    const earlier = deliveredBefore(set.rules, index);
    return [
      earlier.length === 0
        ? ruleTest(rule)
        : `allof (${ruleTest(rule)}, not ${anyOf(earlier)})`,
    ];
  });
  if (tests.length === 0) {
    return [INCLUDE_VACATION];
  }
  return [`if not ${anyOf(tests)} {`, `  ${INCLUDE_VACATION}`, "}"];
};

/**
 * Sieve line breaks are CRLF: "CR and LF can only appear in CRLF pairs"
 * (RFC 5228 §8.1).
 */
export const CRLF = "\r\n";

/** The whole script, ending with a line break (CRLF). */
export const generateScript = (
  set: FilterSet,
  options: GenerateOptions = BY_ID
): string => {
  const lines = [...HEADER];
  const extensions = requiredExtensions(set, options);
  if (extensions.length > 0) {
    lines.push("", `require ${stringList(extensions)};`);
  }
  if (set.vacation) {
    lines.push(
      "",
      "# Vacation response (Stalwart keeps it in its own script).",
      ...vacationLines(set)
    );
  }
  for (const [index, rule] of set.rules.entries()) {
    lines.push(
      "",
      ...ruleLines(rule, options, deliveredBefore(set.rules, index))
    );
  }
  return `${lines.join(CRLF)}${CRLF}`;
};
