// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { MailboxId } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import type { MailboxSummary } from "../../mail/api";
import { ALL_RIGHTS } from "../../mail/rights";
import { generateScript, requiredExtensions } from "./generate";
import { folderOptions, resolveFolders } from "./mailboxes";
import type { Action, Condition, FilterSet, Rule } from "./model";
import { cleanLine, forwardCount, ruleProblems } from "./model";
import type { Command } from "./parse";
import { parseFilterScript, parseSieve } from "./parse";
import { commentJson, quote, withCrlf } from "./sieve-string";
import { SieveSyntaxError, tokenize } from "./tokenize";

const rule = (overrides: Partial<Rule> = {}): Rule => ({
  actions: [{ kind: "markRead" }],
  conditions: [
    {
      field: "from",
      header: "",
      kind: "match",
      negate: false,
      op: "contains",
      value: "news@shop.example",
    },
  ],
  enabled: true,
  id: "x",
  match: "all",
  name: "Newsletters",
  ...overrides,
});

const set = (rules: readonly Rule[], vacation = false): FilterSet => ({
  rules,
  vacation,
});

/** Ids are local; compare the rest. */
const withoutIds = (filters: FilterSet | null) =>
  filters === null
    ? null
    : { ...filters, rules: filters.rules.map(({ id: _id, ...rest }) => rest) };

const match = (
  field: Extract<Condition, { kind: "match" }>["field"],
  op: Extract<Condition, { kind: "match" }>["op"],
  value: string,
  extra: Partial<Extract<Condition, { kind: "match" }>> = {}
): Condition => ({
  field,
  header: "",
  kind: "match",
  negate: false,
  op,
  value,
  ...extra,
});

const EVERYTHING = set(
  [
    rule({
      actions: [
        { kind: "stop" },
        { kind: "move", mailboxId: "m1", mailboxName: "Shops/Receipts" },
        { kind: "star" },
        { keyword: "Work", kind: "keyword" },
        { kind: "markRead" },
        { kind: "copy", mailboxId: "m2", mailboxName: "Archive" },
      ],
      conditions: [
        match("from", "is", "billing@shop.example"),
        match("recipient", "matches", "*@team.example"),
        match("subject", "regex", "^\\[(invoice|receipt)\\]"),
        match("body", "contains", "unsubscribe", { negate: true }),
        match("listId", "contains", "news.shop.example"),
        match("header", "is", "yes", { header: "X-Spam-Flag" }),
        { kilobytes: 2048, kind: "size", op: "over" },
        { kind: "attachment", negate: false },
      ],
      match: "any",
      name: "Shop mail",
    }),
    rule({
      actions: [
        { address: "me@elsewhere.example", keepCopy: true, kind: "forward" },
      ],
      enabled: false,
      name: "Forward boss",
    }),
    rule({
      actions: [{ kind: "reject", reason: "No thanks." }],
      conditions: [],
      name: "Everything",
    }),
    rule({ actions: [{ kind: "discard" }], name: "Bin" }),
  ],
  true
);

const fromHas = (value: string): Condition => match("from", "contains", value);
const lfScript = (filters: FilterSet): string =>
  generateScript(filters).replaceAll("\r\n", "\n");

describe(generateScript, () => {
  it("writes readable Sieve with the right extensions", () => {
    // LF here only to keep the snapshot readable: see the CRLF test below.
    expect(generateScript(EVERYTHING).replaceAll("\r\n", "\n"))
      .toMatchInlineSnapshot(`
        "# Nebula Mail filters (format 1).
        # Made in Settings > Filters. If you change this script by hand, Nebula
        # Mail shows it as a script instead of rules and never overwrites it
        # without asking.

        require ["body", "copy", "fileinto", "imap4flags", "include", "mailboxid", "mime", "regex", "reject"];

        # Vacation response (Stalwart keeps it in its own script).
        if not true {
          include :personal :optional "vacation";
        }

        # Rule: "Shop mail"
        if anyof (address :is "from" "billing@shop.example", address :matches ["to", "cc"] "*@team.example", header :regex "subject" "^\\\\[(invoice|receipt)\\\\]", not body :text :contains "unsubscribe", header :contains "list-id" "news.shop.example", header :is "X-Spam-Flag" "yes", size :over 2048K, header :mime :anychild :contains "content-disposition" "attachment") {
          addflag "\\\\Seen";
          addflag "\\\\Flagged";
          addflag "work";
          fileinto :copy :mailboxid "m2" "Archive";
          fileinto :mailboxid "m1" "Shops/Receipts";
          stop;
        }

        # Rule: "Forward boss"
        if false {
          if header :contains "from" "news@shop.example" {
            redirect :copy "me@elsewhere.example";
          }
        }

        # Rule: "Everything"
        if true {
          reject "No thanks.";
          stop;
        }

        # Rule: "Bin"
        if header :contains "from" "news@shop.example" {
          discard;
        }
        "
      `);
  });

  it("ends every line with CRLF (RFC 5228 §8.1)", () => {
    const script = generateScript(EVERYTHING);
    expect(script.endsWith("\r\n")).toBeTruthy();
    expect(script.replaceAll("\r\n", "")).not.toMatch(/[\r\n]/u);
  });

  it("stops after a reject (RFC 5429 §2.4)", () => {
    const script = generateScript(
      set([rule({ actions: [{ kind: "reject", reason: "No." }] })])
    ).replaceAll("\r\n", "\n");
    expect(script).toContain('  reject "No.";\n  stop;\n}');
    // An explicit stop is not doubled.
    const explicit = generateScript(
      set([
        rule({
          actions: [{ kind: "stop" }, { kind: "reject", reason: "No." }],
        }),
      ])
    );
    expect(explicit.match(/stop;/gu)).toHaveLength(1);
  });

  it("never runs the vacation response for rejected mail (RFC 5429 §2.4)", () => {
    const reject = (value: string, enabled = true): Rule =>
      rule({
        actions: [{ kind: "reject", reason: "No." }],
        conditions: [fromHas(value)],
        enabled,
      });
    expect(lfScript(set([rule()], true))).toContain(
      '\ninclude :personal :optional "vacation";\n'
    );
    expect(lfScript(set([reject("a"), reject("off", false)], true))).toContain(
      'if not header :contains "from" "a" {\n  include :personal :optional "vacation";\n}'
    );
    expect(lfScript(set([reject("a"), rule(), reject("b")], true))).toContain(
      'if not anyof (header :contains "from" "a", header :contains "from" "b") {'
    );
    // Without the vacation response there is nothing to guard.
    expect(lfScript(set([reject("a")]))).not.toContain("include");
  });

  it("a reject never follows an earlier delivery (RFC 5429 §2.4)", () => {
    const filters = set(
      [
        rule({
          actions: [{ kind: "move", mailboxId: "m1", mailboxName: "Shops" }],
          conditions: [fromHas("shop")],
        }),
        rule({
          actions: [
            {
              address: "me@elsewhere.example",
              keepCopy: false,
              kind: "forward",
            },
          ],
          conditions: [fromHas("boss")],
        }),
        // Stops: never reaches the reject, so it doesn't guard.
        rule({
          actions: [
            { kind: "move", mailboxId: "m3", mailboxName: "Bank" },
            { kind: "stop" },
          ],
          conditions: [fromHas("bank")],
        }),
        // Disabled: it delivers nothing, so it doesn't guard.
        rule({
          actions: [{ kind: "copy", mailboxId: "m2", mailboxName: "Off" }],
          conditions: [fromHas("off")],
          enabled: false,
        }),
        rule({
          actions: [{ kind: "reject", reason: "No." }],
          conditions: [fromHas("spam")],
        }),
      ],
      true
    );
    const script = lfScript(filters);
    expect(script).toContain(
      [
        'if header :contains "from" "spam" {',
        '  if not anyof (header :contains "from" "shop", header :contains "from" "boss") {',
        '    reject "No.";',
        "    stop;",
        "  }",
        "}",
      ].join("\n")
    );
    // The vacation response runs unless the reject really happens.
    expect(script).toContain(
      'if not allof (header :contains "from" "spam", not anyof (header :contains "from" "shop", header :contains "from" "boss")) {\n  include'
    );
    expect(
      withoutIds(parseFilterScript(generateScript(filters)))
    ).toStrictEqual(withoutIds(filters));
    // A reject first needs no guard.
    expect(
      lfScript(set([rule({ actions: [{ kind: "reject", reason: "No." }] })]))
    ).not.toContain("if not");
  });

  it("needs no require line for plain rules", () => {
    const script = generateScript(set([rule({ actions: [{ kind: "stop" }] })]));
    expect(script).not.toContain("require");
    expect(requiredExtensions(set([]))).toStrictEqual([]);
  });
});

describe(parseFilterScript, () => {
  it("reads back everything it writes", () => {
    const script = generateScript(EVERYTHING);
    const parsed = parseFilterScript(script);
    // Actions come back in script order.
    const expected = generateScript(parsed ?? set([]));
    expect(expected).toBe(script);
    expect(parsed?.vacation).toBeTruthy();
    expect(parsed?.rules.map((item) => item.enabled)).toStrictEqual([
      true,
      false,
      true,
      true,
    ]);
    expect(parsed?.rules[0]?.conditions).toStrictEqual(
      EVERYTHING.rules[0]?.conditions
    );
  });

  it("accepts LF line breaks (scripts saved before CRLF)", () => {
    const script = generateScript(set([rule()])).replaceAll("\r\n", "\n");
    expect(withoutIds(parseFilterScript(script))).toStrictEqual(
      withoutIds(set([rule()]))
    );
  });

  it("reads back rejecting rules without the stop it added", () => {
    const rejecting = set(
      [
        rule({ actions: [{ kind: "reject", reason: "No." }] }),
        rule({
          actions: [{ kind: "reject", reason: "Bye." }],
          enabled: false,
          name: "Off",
        }),
      ],
      true
    );
    const parsed = parseFilterScript(generateScript(rejecting));
    expect(withoutIds(parsed)).toStrictEqual(withoutIds(rejecting));
  });

  it("an empty set round-trips", () => {
    expect(parseFilterScript(generateScript(set([])))).toStrictEqual(set([]));
  });

  it.each([
    [
      "another client's script",
      'require "fileinto";\nif true { fileinto "A"; }\n',
    ],
    [
      "ours with a hand edit",
      generateScript(set([rule()])).replace("addflag", "addflag  "),
    ],
    ["ours with an extra command", `${generateScript(set([rule()]))}keep;\n`],
    [
      "ours with a comment added",
      generateScript(set([rule()])).replace("# Rule", "# mine\n# Rule"),
    ],
    ["ours, broken", generateScript(set([rule()])).replace("}", "")],
    [
      "ours with a bad name",
      generateScript(set([rule()])).replace('"News', "News"),
    ],
    ["empty", ""],
  ])("keeps %s raw", (_label, script) => {
    expect(parseFilterScript(script)).toBeNull();
  });
});

// --- Sieve injection --------------------------------------------------------

// The two characters that start a variable, built so no linter takes them
// for a template literal.
const VAR = ["$", "{"].join("");

const HOSTILE = [
  '"; discard; #',
  '\\"; discard; #',
  "\\",
  '\\\\"',
  "a\nb\r\nc",
  "line\n.\ndiscard;",
  `${VAR}x}`,
  `$\\{x}`,
  `${VAR}hex:22}`,
  "*/ discard; /*",
  "# comment",
  "}; discard; if true {",
  "text:\ndiscard;\n.",
  "\u2028\u2029\u0085",
  "\u202Eevil",
  "\uD800 lone surrogate",
  "emoji 📬",
  "\u0000nul",
];

const hostileSet = (text: string): FilterSet =>
  set([
    rule({
      actions: [
        { kind: "move", mailboxId: text, mailboxName: text },
        { keyword: text, kind: "keyword" },
        { address: text, keepCopy: false, kind: "forward" },
        { kind: "reject", reason: text },
      ],
      conditions: [
        match("subject", "contains", text),
        match("header", "regex", text, { header: text }),
      ],
      name: text,
    }),
  ]);

/** Command names anywhere in the parsed script. */
const commandNames = (script: string): string[] => {
  const names: string[] = [];
  const walk = (commands: readonly Command[]) => {
    for (const command of commands) {
      names.push(command.name);
      walk(command.block ?? []);
    }
  };
  walk(parseSieve(script));
  return names;
};

describe("Sieve injection", () => {
  it.each(HOSTILE)("user text %j stays inside its strings", (text) => {
    const script = generateScript(hostileSet(text));
    // Exactly the commands we wrote, nothing the text tried to add.
    expect(commandNames(script)).toStrictEqual([
      "require",
      "if",
      "addflag",
      "fileinto",
      "redirect",
      "reject",
      "stop",
    ]);
    // Comments never span lines: the name is one ASCII line.
    const nameLine = script
      .split("\r\n")
      .find((line) => line.startsWith("# Rule:"));
    expect(nameLine).toMatch(/^# Rule: "[ -~]*"$/u);
    // Stalwart expands variables even without "variables": never let one in.
    expect(script).not.toContain(VAR);
  });

  it.each(HOSTILE)("user text %j reads back as the same text", (text) => {
    const script = generateScript(hostileSet(text));
    const parsed = parseFilterScript(script);
    const clean = cleanLine(text, 1000);
    expect(parsed?.rules[0]?.conditions[0]).toMatchObject({ value: clean });
    expect(parsed?.rules[0]?.actions).toContainEqual({
      kind: "move",
      mailboxId: clean,
      mailboxName: clean,
    });
    expect(parsed?.rules[0]?.name).toBe(cleanLine(text, 100));
  });

  it("survives random text (fuzz)", () => {
    const alphabet = [
      '"',
      "\\",
      "\n",
      "\r",
      "$",
      "{",
      "}",
      ";",
      "#",
      "/",
      "*",
      "a",
      " ",
      "é",
      "\u2028",
    ];
    let seed = 7;
    const random = () => {
      seed = (seed * 1_103_515_245 + 12_345) % 2_147_483_648;
      return seed / 2_147_483_648;
    };
    for (let round = 0; round < 300; round += 1) {
      const length = Math.floor(random() * 20);
      const text = Array.from(
        { length },
        () => alphabet[Math.floor(random() * alphabet.length)] ?? ""
      ).join("");
      const script = generateScript(hostileSet(text));
      expect(commandNames(script)).toHaveLength(7);
      expect(parseFilterScript(script)).not.toBeNull();
    }
  });

  it("quotes only with escapes the parser understands", () => {
    expect(quote(`a"b\\c${VAR}d}`)).toBe('"a\\"b\\\\c$\\{d}"');
    const [token] = tokenize(quote(`a"b\\c${VAR}d}`));
    expect(token).toStrictEqual({ type: "string", value: `a"b\\c${VAR}d}` });
    expect(commentJson("é\n\u2028")).toBe(String.raw`"\u00e9\n\u2028"`);
  });
});

describe(parseSieve, () => {
  it("parses a script from another client (Stalwart's vacation)", () => {
    const script = [
      'require ["vacation", "relational", "date"];',
      "",
      'if currentdate :value "ge" "iso8601" "2026-10-20T00:00:00Z" {',
      'vacation :mime :subject "Away \\"quoted\\"" "Content-Type: text/plain',
      "",
      'I am away.";',
      "}",
      "/* block comment */",
      'if size :over 1M { keep; } elsif exists "x" { stop; } else { discard; }',
      'if anyof(true, false) { set "a" text:',
      "line one",
      "..dot",
      ".",
      "; }",
    ].join("\r\n");
    const commands = parseSieve(script);
    expect(commands.map((command) => command.name)).toStrictEqual([
      "require",
      "if",
      "if",
      "elsif",
      "else",
      "if",
    ]);
    const vacation = commands[1]?.block?.[0];
    expect(vacation?.args).toContainEqual({
      type: "strings",
      value: ['Away "quoted"'],
    });
    expect(commands[2]?.tests[0]?.args).toContainEqual({
      type: "number",
      value: 1024 * 1024,
    });
    expect(commands[5]?.block?.[0]?.args.at(-1)).toStrictEqual({
      type: "strings",
      value: ["line one\n.dot"],
    });
  });

  it.each([
    "if true { stop; ",
    '"unterminated',
    "/* open",
    "if (true { }",
    `${"if true { ".repeat(40)}${"}".repeat(40)}`,
  ])("rejects %j", (script) => {
    expect(() => parseSieve(script)).toThrow(SieveSyntaxError);
  });
});

const withActions = (actions: Action[]) => ruleProblems(rule({ actions }));

describe(ruleProblems, () => {
  it("accepts a sound rule", () => {
    expect(ruleProblems(rule())).toStrictEqual([]);
  });

  it.each<[Action[], string]>([
    [[], "noActions"],
    [
      [{ kind: "discard" }, { kind: "move", mailboxId: "a", mailboxName: "A" }],
      "conflict",
    ],
    [[{ kind: "reject", reason: "" }, { kind: "discard" }], "conflict"],
    [
      [{ address: "not an address", keepCopy: true, kind: "forward" }],
      "badAddress",
    ],
    [[{ keyword: "two words", kind: "keyword" }], "badKeyword"],
    [[{ kind: "move", mailboxId: "", mailboxName: "" }], "noMailbox"],
  ])("finds %j", (actions, problem) => {
    expect(withActions(actions)).toContain(problem);
  });

  it("checks conditions", () => {
    expect(
      ruleProblems(
        rule({
          conditions: [
            match("subject", "contains", " "),
            match("header", "is", "x", { header: "Bad Name:" }),
            match("subject", "regex", "(unclosed"),
            { kilobytes: 0, kind: "size", op: "over" },
          ],
        })
      )
    ).toStrictEqual(["emptyValue", "badHeader", "badRegex", "badSize"]);
  });

  it("counts forwards of enabled rules", () => {
    const forward: Action = {
      address: "a@b.example",
      keepCopy: true,
      kind: "forward",
    };
    expect(
      forwardCount(
        set([
          rule({ actions: [forward] }),
          rule({ actions: [forward], enabled: false }),
        ])
      )
    ).toBe(1);
  });
});

const box = (
  id: string,
  name: string,
  parentId: string | null
): MailboxSummary => ({
  accountId: null,
  id: MailboxId.parse(id),
  mayShare: false,
  name,
  parentId: parentId === null ? null : MailboxId.parse(parentId),
  rights: ALL_RIGHTS,
  role: null,
  sharedWith: 0,
  total: 0,
  unread: 0,
});

describe("folders by IMAP name (the bridge, any separator or prefix)", () => {
  const boxes = [box("m1", "Lists", null), box("m2", "Rust", "m1")];
  const names = new Map([
    ["m1", "INBOX.Lists"],
    ["m2", "INBOX.Lists.Rust"],
  ]);

  it("uses the server's name, and the path where there is none", () => {
    expect(folderOptions(boxes).map((f) => f.path)).toStrictEqual([
      "Lists",
      "Lists/Rust",
    ]);
    expect(folderOptions(boxes, names).map((f) => f.path)).toStrictEqual([
      "INBOX.Lists",
      "INBOX.Lists.Rust",
    ]);
  });

  it("writes it, reads it back and finds the folder again", () => {
    const [, rust] = folderOptions(boxes, names);
    const moving = rule({
      actions: [
        { kind: "move", mailboxId: "m2", mailboxName: rust?.path ?? "" },
      ],
    });
    const script = generateScript(set([moving]), { mailboxIds: false });
    expect(script).toContain('fileinto "INBOX.Lists.Rust";');
    const parsed = parseFilterScript(script);
    const resolved = resolveFolders(
      parsed ?? set([]),
      folderOptions(boxes, names)
    );
    expect(resolved.rules[0]?.actions[0]).toStrictEqual({
      kind: "move",
      mailboxId: "m2",
      mailboxName: "INBOX.Lists.Rust",
    });
  });
});

describe("folders by path (servers without mailboxid, like the bridge)", () => {
  const moving = rule({
    actions: [
      { kind: "move", mailboxId: "m1", mailboxName: "Shops/Receipts" },
      { kind: "copy", mailboxId: "m2", mailboxName: `Odd "name" \${x}` },
    ],
  });

  it("writes fileinto with the path only, without mailbox/mailboxid", () => {
    const script = generateScript(set([moving]), { mailboxIds: false });
    expect(script).toContain('fileinto "Shops/Receipts";');
    expect(script).toContain(String.raw`fileinto :copy "Odd \"name\" $\{x}";`);
    expect(script).not.toContain("mailboxid");
    expect(
      requiredExtensions(set([moving]), { mailboxIds: false })
    ).toStrictEqual(["copy", "fileinto"]);
  });

  it("reads it back and finds the folders by path", () => {
    const script = generateScript(set([moving]), { mailboxIds: false });
    const parsed = parseFilterScript(script);
    // Actions come back in the order they are written (copies first).
    expect(parsed?.rules[0]?.actions).toStrictEqual([
      { kind: "copy", mailboxId: "", mailboxName: `Odd "name" \${x}` },
      { kind: "move", mailboxId: "", mailboxName: "Shops/Receipts" },
    ]);
    const folders = [{ id: "m9", path: "Shops/Receipts" }];
    const resolved = resolveFolders(parsed ?? set([]), folders);
    expect(resolved.rules[0]?.actions).toStrictEqual([
      { kind: "copy", mailboxId: "", mailboxName: `Odd "name" \${x}` },
      { kind: "move", mailboxId: "m9", mailboxName: "Shops/Receipts" },
    ]);
    // Saved again for the same server: the same text.
    expect(generateScript(resolved, { mailboxIds: false })).toBe(script);
  });
});

describe(withCrlf, () => {
  it("makes every line break CRLF (RFC 5228 §8.1)", () => {
    expect(withCrlf("a\nb\r\nc\rd\n")).toBe("a\r\nb\r\nc\r\nd\r\n");
  });
});
