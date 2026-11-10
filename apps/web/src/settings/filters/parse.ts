// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Reads Sieve back into visual rules, but only scripts we wrote: a small
// RFC 5228 tokenizer and parser (any script), then a reader that accepts
// exactly the commands generate.ts emits. Finally the rules are generated
// again and must give the same text: anything edited by hand (even a
// comment) stays a raw script, so we never overwrite someone's work.

import { SCRIPT_MARKER, generateScript, rejects } from "./generate";
import type {
  Action,
  Condition,
  FilterSet,
  MatchField,
  MatchOp,
  Rule,
} from "./model";
import { MATCH_OPS, newRuleId } from "./model";
import { SieveSyntaxError, tokenize } from "./tokenize";
import type { Token } from "./tokenize";

// --- Syntax tree ------------------------------------------------------------

export type Argument =
  | { readonly type: "tag"; readonly value: string }
  | { readonly type: "number"; readonly value: number }
  | { readonly type: "strings"; readonly value: readonly string[] };

export interface Test {
  readonly name: string;
  readonly args: readonly Argument[];
  readonly tests: readonly Test[];
}

export interface Command extends Test {
  /** Comments right before the command. */
  readonly comments: readonly string[];
  readonly block: readonly Command[] | null;
}

const MAX_DEPTH = 32;

class Parser {
  readonly #tokens: readonly Token[];
  #index = 0;
  #comments: string[] = [];

  constructor(tokens: readonly Token[]) {
    this.#tokens = tokens;
  }

  #peek(): Token | undefined {
    while (this.#tokens[this.#index]?.type === "comment") {
      this.#comments.push(this.#tokens[this.#index]?.value.toString() ?? "");
      this.#index += 1;
    }
    return this.#tokens[this.#index];
  }

  #next(): Token {
    const token = this.#peek();
    if (token === undefined) {
      throw new SieveSyntaxError("unexpected end", this.#index);
    }
    this.#index += 1;
    return token;
  }

  #isPunct(value: string): boolean {
    const token = this.#peek();
    return token?.type === "punct" && token.value === value;
  }

  #expect(value: string): void {
    const token = this.#next();
    if (token.type !== "punct" || token.value !== value) {
      throw new SieveSyntaxError(`expected "${value}"`, this.#index);
    }
  }

  #stringList(): string[] {
    const values: string[] = [];
    for (;;) {
      const token = this.#next();
      if (token.type !== "string") {
        throw new SieveSyntaxError("expected a string", this.#index);
      }
      values.push(token.value);
      if (!this.#isPunct(",")) {
        break;
      }
      this.#next();
    }
    this.#expect("]");
    return values;
  }

  #arguments(depth: number): { args: Argument[]; tests: Test[] } {
    const args: Argument[] = [];
    for (;;) {
      const token = this.#peek();
      if (token?.type === "tag" || token?.type === "number") {
        this.#next();
        args.push(token);
      } else if (token?.type === "string") {
        this.#next();
        args.push({ type: "strings", value: [token.value] });
      } else if (token?.type === "punct" && token.value === "[") {
        this.#next();
        args.push({ type: "strings", value: this.#stringList() });
      } else {
        break;
      }
    }
    const token = this.#peek();
    if (token?.type === "identifier") {
      return { args, tests: [this.#test(depth + 1)] };
    }
    if (token?.type === "punct" && token.value === "(") {
      this.#next();
      const tests = [this.#test(depth + 1)];
      while (this.#isPunct(",")) {
        this.#next();
        tests.push(this.#test(depth + 1));
      }
      this.#expect(")");
      return { args, tests };
    }
    return { args, tests: [] };
  }

  #test(depth: number): Test {
    if (depth > MAX_DEPTH) {
      throw new SieveSyntaxError("nested too deep", this.#index);
    }
    const token = this.#next();
    if (token.type !== "identifier") {
      throw new SieveSyntaxError("expected a test", this.#index);
    }
    return { name: token.value, ...this.#arguments(depth) };
  }

  #command(depth: number): Command {
    if (depth > MAX_DEPTH) {
      throw new SieveSyntaxError("nested too deep", this.#index);
    }
    this.#peek();
    const comments = this.#comments;
    this.#comments = [];
    const token = this.#next();
    if (token.type !== "identifier") {
      throw new SieveSyntaxError("expected a command", this.#index);
    }
    const { args, tests } = this.#arguments(depth);
    if (this.#isPunct(";")) {
      this.#next();
      return { args, block: null, comments, name: token.value, tests };
    }
    this.#expect("{");
    const block: Command[] = [];
    while (!this.#isPunct("}")) {
      block.push(this.#command(depth + 1));
    }
    this.#expect("}");
    return { args, block, comments, name: token.value, tests };
  }

  commands(): Command[] {
    const commands: Command[] = [];
    while (this.#peek() !== undefined) {
      commands.push(this.#command(0));
    }
    return commands;
  }
}

/** Any Sieve script → commands (throws SieveSyntaxError). */
export const parseSieve = (text: string): Command[] =>
  new Parser(tokenize(text)).commands();

// --- Our subset → rules -----------------------------------------------------

/** Inside the reader: the script isn't one of ours. */
const fail = (): never => {
  throw new SieveSyntaxError("not one of our scripts", -1);
};

const tags = (args: readonly Argument[]): string[] =>
  args.flatMap((arg) => (arg.type === "tag" ? [arg.value] : []));

const strings = (args: readonly Argument[]): (readonly string[])[] =>
  args.flatMap((arg) => (arg.type === "strings" ? [arg.value] : []));

const single = (list: readonly string[] | undefined): string =>
  list?.length === 1 ? (list[0] ?? fail()) : fail();

const matchOp = (tagList: readonly string[]): MatchOp =>
  MATCH_OPS.find((op) => tagList.includes(op)) ?? fail();

const fieldOf = (headers: readonly string[]): MatchField | null => {
  const key = headers.join(",");
  const fields: Readonly<Record<string, MatchField>> = {
    from: "from",
    "list-id": "listId",
    subject: "subject",
    "to,cc": "recipient",
  };
  return fields[key] ?? null;
};

const readCondition = (test: Test, negate: boolean): Condition => {
  const tagList = tags(test.args);
  const [first, second] = strings(test.args);
  switch (test.name) {
    case "not": {
      const [inner] = test.tests;
      return negate || inner === undefined
        ? fail()
        : readCondition(inner, true);
    }
    case "body": {
      return {
        field: "body",
        header: "",
        kind: "match",
        negate,
        op: matchOp(tagList),
        value: single(first),
      };
    }
    case "address":
    case "header": {
      if (tagList.includes("mime")) {
        return { kind: "attachment", negate };
      }
      const headers = first ?? fail();
      const field = fieldOf(headers) ?? "header";
      return {
        field,
        header: field === "header" ? single(headers) : "",
        kind: "match",
        negate,
        op: matchOp(tagList),
        value: single(second),
      };
    }
    case "size": {
      const number = test.args.find((arg) => arg.type === "number");
      return negate || number?.type !== "number"
        ? fail()
        : {
            kilobytes: Math.trunc(number.value / 1024),
            kind: "size",
            op: tagList.includes("over") ? "over" : "under",
          };
    }
    default: {
      return fail();
    }
  }
};

const readAction = (command: Command): Action => {
  const tagList = tags(command.args);
  const [first, second] = strings(command.args);
  switch (command.name) {
    case "addflag": {
      const flag = single(first);
      if (flag === "\\Seen") {
        return { kind: "markRead" };
      }
      if (flag === "\\Flagged") {
        return { kind: "star" };
      }
      return { keyword: flag, kind: "keyword" };
    }
    case "fileinto": {
      const kind = tagList.includes("copy") ? "copy" : "move";
      // By id with the path as fallback, or by path only (servers without
      // `mailboxid`, like the bridge): the id is found from the path later
      // (`resolveFolders`).
      return tagList.includes("mailboxid")
        ? { kind, mailboxId: single(first), mailboxName: single(second) }
        : { kind, mailboxId: "", mailboxName: single(first) };
    }
    case "redirect": {
      return {
        address: single(first),
        keepCopy: tagList.includes("copy"),
        kind: "forward",
      };
    }
    case "reject": {
      return { kind: "reject", reason: single(first) };
    }
    case "discard":
    case "stop": {
      return { kind: command.name };
    }
    default: {
      return fail();
    }
  }
};

const RULE_COMMENT = /^ Rule: (?<name>".*")$/u;

const readName = (comments: readonly string[]): string => {
  const match = RULE_COMMENT.exec(comments.at(-1) ?? "");
  const parsed: unknown = JSON.parse(match?.groups?.name ?? fail());
  return typeof parsed === "string" ? parsed : fail();
};

/**
 * The actions of a rule, without the guard generate.ts puts around those
 * of a rule that rejects when earlier rules deliver (`if not … { … }`);
 * the comparison with the regenerated script checks it was exactly that.
 */
const unguarded = (block: readonly Command[]): readonly Command[] => {
  const [only] = block;
  if (
    block.length === 1 &&
    only?.name === "if" &&
    only.tests[0]?.name === "not" &&
    only.block?.some((command) => command.name === "reject") === true
  ) {
    return only.block;
  }
  return block;
};

const readRule = (command: Command): Rule => {
  const name = readName(command.comments);
  let target = command;
  let enabled = true;
  const [onlyTest] = command.tests;
  if (onlyTest?.name === "false" && command.block?.length === 1) {
    enabled = false;
    target = command.block[0] ?? fail();
  }
  const [test] = target.tests;
  if (target.name !== "if" || target.block === null || test === undefined) {
    return fail();
  }
  let match: Rule["match"] = "all";
  let tests: readonly Test[] = [test];
  if (test.name === "allof" || test.name === "anyof") {
    match = test.name === "allof" ? "all" : "any";
    ({ tests } = test);
  } else if (test.name === "true") {
    tests = [];
  }
  return {
    actions: unguarded(target.block).map(readAction),
    conditions: tests.map((item) => readCondition(item, false)),
    enabled,
    id: newRuleId(),
    match,
    name,
  };
};

/** LF line breaks (stored scripts have CRLF, older ones LF), no trailing blanks. */
const normalize = (text: string): string =>
  `${text.replaceAll("\r\n", "\n").trimEnd()}\n`;

/** The vacation include, alone or guarded by the reject rules' tests. */
const isVacation = (command: Command | undefined): boolean =>
  command?.name === "include" ||
  (command?.name === "if" &&
    command.block?.length === 1 &&
    command.block[0]?.name === "include");

/** generate.ts adds `stop` to rules that reject: not a user action. */
const withoutAddedStop = (rule: Rule): Rule =>
  rejects(rule)
    ? {
        ...rule,
        actions: rule.actions.filter((action) => action.kind !== "stop"),
      }
    : rule;

/**
 * Our own script → rules; `null` for anything else (another client's
 * script, or ours edited by hand), which the UI shows as raw Sieve.
 */
export const parseFilterScript = (text: string): FilterSet | null => {
  const script = normalize(text);
  if (!script.startsWith(SCRIPT_MARKER)) {
    return null;
  }
  try {
    const commands = parseSieve(script).filter(
      (command) => command.name !== "require"
    );
    let vacation = false;
    if (isVacation(commands[0])) {
      vacation = true;
      commands.shift();
    }
    const rules = commands.map((command) =>
      withoutAddedStop(readRule(command))
    );
    const set: FilterSet = { rules, vacation };
    return normalize(generateScript(set)) === script ? set : null;
  } catch (error) {
    if (error instanceof SieveSyntaxError) {
      return null;
    }
    // JSON.parse of a damaged rule name.
    if (error instanceof SyntaxError) {
      return null;
    }
    throw error;
  }
};
