// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Regenerates src/trackers/data.generated.ts from the vendored, unmodified
// MailTrackerBlocker source (vendor/mail-trackers/MTBBlockedMessage.m).
//
// Usage: bun scripts/generate-trackers.ts [--check]
//   --check  exit 1 if the generated file is out of date (for CI).
//
// The upstream list is an Objective-C dictionary literal inside
// `+ (NSDictionary*)getTrackerDict`. We tokenize it (strings, comments and
// punctuation) instead of using regexes over the source, so commented-out
// rules stay out and escaped characters are decoded exactly once.

import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";

const here = import.meta.dirname;
const root = path.join(here, "..");
const sourcePath = path.join(root, "vendor/mail-trackers/MTBBlockedMessage.m");
const outputPath = path.join(root, "src/trackers/data.generated.ts");

// Keep in sync with vendor/mail-trackers/README.md.
const UPSTREAM_REPO = "apparition47/MailTrackerBlocker";
const UPSTREAM_COMMIT = "7463682a39af055a488539a3d0aa7898437b33f5";

type Token =
  | { readonly kind: "string"; readonly value: string }
  | { readonly kind: "punct"; readonly value: string };

const OBJC_ESCAPES: Readonly<Record<string, string>> = {
  '"': '"',
  "'": "'",
  "\\": "\\",
  n: "\n",
  r: "\r",
  t: "\t",
};

const readString = (
  src: string,
  start: number
): { value: string; end: number } => {
  let value = "";
  let i = start;
  while (i < src.length) {
    const ch = src[i];
    if (ch === '"') {
      return { end: i + 1, value };
    }
    if (ch === "\\") {
      const next = src[i + 1] ?? "";
      const decoded = OBJC_ESCAPES[next];
      if (decoded === undefined) {
        throw new Error(`Unsupported escape \\${next} at offset ${i}`);
      }
      value += decoded;
      i += 2;
    } else {
      value += ch;
      i += 1;
    }
  }
  throw new Error("Unterminated string literal");
};

const tokenize = (src: string): Token[] => {
  const tokens: Token[] = [];
  let i = 0;
  while (i < src.length) {
    const ch = src[i] ?? "";
    const next = src[i + 1] ?? "";
    if (/\s/u.test(ch)) {
      i += 1;
    } else if (ch === "/" && next === "/") {
      const end = src.indexOf("\n", i);
      i = end === -1 ? src.length : end + 1;
    } else if (ch === "/" && next === "*") {
      const end = src.indexOf("*/", i + 2);
      if (end === -1) {
        throw new Error("Unterminated block comment");
      }
      i = end + 2;
    } else if (ch === "@" && next === '"') {
      const { value, end } = readString(src, i + 2);
      tokens.push({ kind: "string", value });
      i = end;
    } else if (ch === "@" && (next === "[" || next === "{")) {
      tokens.push({ kind: "punct", value: next });
      i += 2;
    } else if ("[]{}:,;".includes(ch)) {
      tokens.push({ kind: "punct", value: ch });
      i += 1;
    } else {
      throw new Error(
        `Unexpected character ${JSON.stringify(ch)} at offset ${i}`
      );
    }
  }
  return tokens;
};

const extractDictionarySource = (src: string): string => {
  const marker = "+ (NSDictionary*)getTrackerDict {";
  const start = src.indexOf(marker);
  if (start === -1) {
    throw new Error("getTrackerDict not found: upstream format changed");
  }
  const open = src.indexOf("return @{", start);
  const close = src.indexOf("\n    };", open);
  if (open === -1 || close === -1) {
    throw new Error("Dictionary literal not found: upstream format changed");
  }
  return src.slice(open + "return ".length, close + "\n    }".length);
};

type Rules = [service: string, patterns: string[]][];

const expect = (
  tokens: readonly Token[],
  index: number,
  value: string
): void => {
  const token = tokens[index];
  if (token?.kind !== "punct" || token.value !== value) {
    throw new Error(
      `Expected ${value} at token ${index}, got ${JSON.stringify(token)}`
    );
  }
};

const parseRules = (tokens: readonly Token[]): Rules => {
  const rules: Rules = [];
  expect(tokens, 0, "{");
  let i = 1;
  while (i < tokens.length) {
    const key = tokens[i];
    if (key?.kind === "punct" && key.value === "}") {
      return rules;
    }
    if (key?.kind !== "string") {
      throw new Error(`Expected service name at token ${i}`);
    }
    expect(tokens, i + 1, ":");
    expect(tokens, i + 2, "[");
    i += 3;
    const patterns: string[] = [];
    while (tokens[i]?.kind === "string") {
      const token = tokens[i];
      if (token?.kind === "string") {
        patterns.push(token.value);
      }
      i += 1;
      const sep = tokens[i];
      if (sep?.kind === "punct" && sep.value === ",") {
        i += 1;
      }
    }
    expect(tokens, i, "]");
    i += 1;
    const sep = tokens[i];
    if (sep?.kind === "punct" && sep.value === ",") {
      i += 1;
    }
    rules.push([key.value, patterns]);
  }
  throw new Error("Unterminated dictionary");
};

const render = (rules: Rules): string => {
  const patternCount = rules.reduce(
    (sum, [, patterns]) => sum + patterns.length,
    0
  );
  const body = rules
    .map(
      ([service, patterns]) =>
        `  [${JSON.stringify(service)}, ${JSON.stringify(patterns)}],`
    )
    .join("\n");
  return `// SPDX-FileCopyrightText: 2020 Aaron Lee
// SPDX-License-Identifier: BSD-3-Clause

// GENERATED by scripts/generate-trackers.ts from
// https://github.com/${UPSTREAM_REPO}/blob/${UPSTREAM_COMMIT}/Source/MTBBlockedMessage.m
// Do not edit by hand. Patterns are ICU regexes, matched case-insensitively.

export const MTB_SOURCE = {
  commit: "${UPSTREAM_COMMIT}",
  repository: "https://github.com/${UPSTREAM_REPO}",
} as const;

export const MTB_PATTERN_COUNT = ${patternCount};

export const MTB_RULES: readonly (readonly [service: string, patterns: readonly string[]])[] = [
${body}
];
`;
};

const main = (): void => {
  const source = readFileSync(sourcePath, "utf-8");
  const rules = parseRules(tokenize(extractDictionarySource(source)));
  for (const [service, patterns] of rules) {
    if (patterns.length === 0) {
      throw new Error(`Service ${service} has no patterns`);
    }
    for (const pattern of patterns) {
      // Throws on ICU syntax that JavaScript does not understand.
      const regex = new RegExp(pattern, "iu");
      if (regex.source === "") {
        throw new Error(`Empty pattern for ${service}`);
      }
    }
  }
  const output = render(rules);
  if (process.argv.includes("--check")) {
    const current = readFileSync(outputPath, "utf-8");
    if (current !== output) {
      process.stderr.write(
        "data.generated.ts is out of date; run bun scripts/generate-trackers.ts\n"
      );
      process.exit(1);
    }
    return;
  }
  writeFileSync(outputPath, output);
  const count = rules.reduce((sum, [, patterns]) => sum + patterns.length, 0);
  process.stdout.write(
    `Wrote ${rules.length} services, ${count} patterns to ${outputPath}\n`
  );
};

main();
