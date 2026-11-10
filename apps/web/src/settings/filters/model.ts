// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The visual filters: a list of rules, each with conditions and actions.
// generate.ts turns them into Sieve, parse.ts reads our own scripts back.
// Everything the user types ends up inside a Sieve string, so the checks
// here are for helpful messages; safety comes from the quoting in
// sieve-string.ts (Sieve injection is impossible whatever passes here).

export const MATCH_OPS = ["contains", "is", "matches", "regex"] as const;
export type MatchOp = (typeof MATCH_OPS)[number];

export const MATCH_FIELDS = [
  "from",
  "recipient",
  "subject",
  "body",
  "listId",
  "header",
] as const;
export type MatchField = (typeof MATCH_FIELDS)[number];

export type Condition =
  | {
      readonly kind: "match";
      readonly field: MatchField;
      /** Header name, only for `field: "header"` ("" otherwise). */
      readonly header: string;
      readonly op: MatchOp;
      readonly value: string;
      readonly negate: boolean;
    }
  | {
      readonly kind: "size";
      readonly op: "over" | "under";
      readonly kilobytes: number;
    }
  | { readonly kind: "attachment"; readonly negate: boolean };

export type ConditionKind = Condition["kind"];

export type Action =
  | {
      readonly kind: "move" | "copy";
      readonly mailboxId: string;
      /** Fallback if the id is gone (Stalwart then looks the name up). */
      readonly mailboxName: string;
    }
  | { readonly kind: "markRead" | "star" | "discard" | "stop" }
  | { readonly kind: "keyword"; readonly keyword: string }
  | {
      readonly kind: "forward";
      readonly address: string;
      /** Also keep it here (`redirect :copy`); otherwise only forward. */
      readonly keepCopy: boolean;
    }
  | { readonly kind: "reject"; readonly reason: string };

export type ActionKind = Action["kind"];

/** Order of the actions in the script (flags first: they apply to the copies filed after them). */
export const ACTION_ORDER: readonly ActionKind[] = [
  "markRead",
  "star",
  "keyword",
  "copy",
  "move",
  "forward",
  "reject",
  "discard",
  "stop",
];

export interface Rule {
  /** Local only (React keys); not saved in the script. */
  readonly id: string;
  readonly name: string;
  readonly enabled: boolean;
  readonly match: "all" | "any";
  readonly conditions: readonly Condition[];
  readonly actions: readonly Action[];
}

export interface FilterSet {
  readonly rules: readonly Rule[];
  /**
   * Run the vacation response first. Stalwart runs one script at a time
   * and the vacation response is its own script, so ours includes it
   * (filtros).
   */
  readonly vacation: boolean;
}

export const MAX_RULE_NAME = 100;
export const MAX_VALUE = 1000;
export const MAX_KILOBYTES = 10_000_000;

let counter = 0;
/** A local id for a new rule. */
export const newRuleId = (): string => {
  counter += 1;
  return `rule-${String(counter)}-${Math.random().toString(36).slice(2, 8)}`;
};

export const emptyRule = (name: string): Rule => ({
  actions: [],
  conditions: [
    {
      field: "from",
      header: "",
      kind: "match",
      negate: false,
      op: "contains",
      value: "",
    },
  ],
  enabled: true,
  id: newRuleId(),
  match: "all",
  name,
});

/** Sorts the actions in script order (see ACTION_ORDER). */
export const sortActions = (actions: readonly Action[]): Action[] =>
  actions.toSorted(
    (a, b) => ACTION_ORDER.indexOf(a.kind) - ACTION_ORDER.indexOf(b.kind)
  );

// --- Checks -----------------------------------------------------------------

export type Problem =
  | "noActions"
  | "emptyValue"
  | "badHeader"
  | "badRegex"
  | "badSize"
  | "badKeyword"
  | "badAddress"
  | "noMailbox"
  | "conflict"
  | "tooManyRedirects";

// RFC 5322 field-name: printable ASCII except ":".
const HEADER_NAME = /^[!-9;-~]{1,76}$/u;
// JMAP keywords (RFC 8621 §4.1.1) without the system ones we set ourselves.
const KEYWORD = /^[\w$.-]{1,64}$/u;
// Good enough for a forwarding target; the server checks it again.
const ADDRESS =
  /^[^\s"(),:;<>@[\\\]]+@[^\s"(),:;<>@[\\\]]+\.[^\s"(),:;<>@[\\\]]+$/u;

export const isHeaderName = (name: string): boolean => HEADER_NAME.test(name);
export const isKeyword = (keyword: string): boolean => KEYWORD.test(keyword);
export const isAddress = (address: string): boolean =>
  address.length <= 254 && ADDRESS.test(address);

const isRegex = (source: string): boolean => {
  try {
    // Sieve uses POSIX extended regexes; JavaScript's catch the usual typos.
    // oxlint-disable-next-line no-new, prefer-regex-literals -- only checks that it compiles.
    new RegExp(source, "u");
    return true;
  } catch {
    return false;
  }
};

const conditionProblems = (condition: Condition): Problem[] => {
  switch (condition.kind) {
    case "match": {
      const problems: Problem[] = [];
      if (condition.value.trim() === "") {
        problems.push("emptyValue");
      }
      if (condition.field === "header" && !isHeaderName(condition.header)) {
        problems.push("badHeader");
      }
      if (condition.op === "regex" && !isRegex(condition.value)) {
        problems.push("badRegex");
      }
      return problems;
    }
    case "size": {
      return Number.isSafeInteger(condition.kilobytes) &&
        condition.kilobytes > 0 &&
        condition.kilobytes <= MAX_KILOBYTES
        ? []
        : ["badSize"];
    }
    case "attachment": {
      return [];
    }
    default: {
      return condition satisfies never;
    }
  }
};

const actionProblems = (action: Action): Problem[] => {
  switch (action.kind) {
    case "move":
    case "copy": {
      return action.mailboxId === "" ? ["noMailbox"] : [];
    }
    case "keyword": {
      return isKeyword(action.keyword) ? [] : ["badKeyword"];
    }
    case "forward": {
      return isAddress(action.address) ? [] : ["badAddress"];
    }
    case "markRead":
    case "star":
    case "discard":
    case "stop":
    case "reject": {
      return [];
    }
    default: {
      return action satisfies never;
    }
  }
};

/** What's wrong with a rule, for the editor (empty: fine). */
export const ruleProblems = (rule: Rule): Problem[] => {
  const problems = [
    ...rule.conditions.flatMap(conditionProblems),
    ...rule.actions.flatMap(actionProblems),
  ];
  const kinds = new Set(rule.actions.map((action) => action.kind));
  if (rule.actions.length === 0) {
    problems.push("noActions");
  }
  // Sieve can't reject or discard a message and also deliver it.
  const ends = (kinds.has("reject") ? 1 : 0) + (kinds.has("discard") ? 1 : 0);
  const delivers = kinds.has("move") || kinds.has("copy");
  if (ends > 1 || (ends === 1 && (delivers || kinds.has("forward")))) {
    problems.push("conflict");
  }
  return [...new Set(problems)];
};

/** Forwarding actions of the enabled rules (Stalwart allows one per message). */
export const forwardCount = (set: FilterSet): number =>
  set.rules
    .filter((rule) => rule.enabled)
    .reduce(
      (sum, rule) =>
        sum + rule.actions.filter((action) => action.kind === "forward").length,
      0
    );

/** Removes what can't live in a single-line value (controls, bad UTF-16). */
export const cleanLine = (text: string, max: number): string =>
  text
    .toWellFormed()
    // oxlint-disable-next-line no-control-regex -- removing control characters is the point.
    .replaceAll(/[\u0000-\u001F\u007F-\u009F\u2028\u2029]/gu, " ")
    .slice(0, max);
