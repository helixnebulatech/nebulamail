// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Test data for long lists (virtual list, infinite loading, threads):
//   bun run dev:seed-load            # 3000 emails in alice's "Load test" folder
//   bun run dev:seed-load --count=5000
//   bun run dev:seed-load --remove   # delete the folder and its emails
//
// Everything goes into its own folder ("Load test"), so the inbox and the
// other test emails stay as they are. Senders use the reserved
// `.example` domain. About a fifth of the emails are replies (threads of
// 2–5 with References), every third conversation ends unread and a few
// are starred. Safe to run again: with the right number of emails it does
// nothing; otherwise it empties the folder and fills it again.

import { existsSync, readFileSync } from "node:fs";
import path from "node:path";

import { z } from "zod";

const ROOT = path.join(import.meta.dirname, "..");
const STALWART_URL = "http://127.0.0.1:8080";
const USER = "alice@nebulamail.test";
const FOLDER = "Load test";
const DOMAIN = "load-test.example";
const USING = ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"];

const args = process.argv.slice(2);
const remove = args.includes("--remove");
const countArg = args
  .find((arg) => arg.startsWith("--count="))
  ?.slice("--count=".length);
const COUNT = z.coerce
  .number()
  .int()
  .min(1)
  .max(50_000)
  .parse(countArg ?? 3000);

const password = (() => {
  const file = path.join(ROOT, "docker/.env");
  const line = existsSync(file)
    ? readFileSync(file, "utf-8")
        .split("\n")
        .find((entry) => entry.startsWith("TEST_ALICE_PASSWORD="))
    : undefined;
  if (line === undefined) {
    throw new Error("No TEST_ALICE_PASSWORD in docker/.env: run dev:setup");
  }
  return line.slice("TEST_ALICE_PASSWORD=".length);
})();
const authorization = `Basic ${btoa(`${USER}:${password}`)}`;

const Responses = z.object({
  methodResponses: z.array(
    z.tuple([z.string(), z.record(z.string(), z.unknown()), z.string()])
  ),
});

type Call = readonly [string, Record<string, unknown>];

/** Runs JMAP calls and fails on any method error or rejected object. */
const jmap = async (
  calls: readonly Call[]
): Promise<Record<string, unknown>[]> => {
  const response = await fetch(`${STALWART_URL}/jmap/`, {
    body: JSON.stringify({
      methodCalls: calls.map(([name, callArgs], index) => [
        name,
        callArgs,
        String(index),
      ]),
      using: USING,
    }),
    headers: { authorization, "content-type": "application/json" },
    method: "POST",
  });
  if (!response.ok) {
    throw new Error(`JMAP HTTP ${response.status}: ${await response.text()}`);
  }
  const { methodResponses } = Responses.parse(await response.json());
  return methodResponses.map(([name, result]) => {
    if (name === "error") {
      throw new Error(`JMAP error: ${JSON.stringify(result)}`);
    }
    for (const key of ["notCreated", "notDestroyed"]) {
      const failed = result[key];
      if (
        failed !== undefined &&
        failed !== null &&
        Object.keys(failed).length > 0
      ) {
        throw new Error(`${name} ${key}: ${JSON.stringify(failed)}`);
      }
    }
    return result;
  });
};

const accountId = await (async () => {
  const session = await fetch(`${STALWART_URL}/jmap/session`, {
    headers: { authorization },
  });
  return z
    .object({
      primaryAccounts: z.object({ "urn:ietf:params:jmap:mail": z.string() }),
    })
    .parse(await session.json()).primaryAccounts["urn:ietf:params:jmap:mail"];
})();

const Ids = z.object({ ids: z.array(z.string()) });
const Mailboxes = z.object({
  list: z.array(
    z.object({ id: z.string(), name: z.string(), totalEmails: z.number() })
  ),
});

const findFolder = async () => {
  const [result] = await jmap([
    [
      "Mailbox/get",
      { accountId, properties: ["name", "totalEmails", "parentId"] },
    ],
  ]);
  return Mailboxes.parse(result).list.find((box) => box.name === FOLDER);
};

/** Destroys every email in the folder, 500 at a time. */
const emptyFolder = async (mailboxId: string): Promise<void> => {
  for (;;) {
    // oxlint-disable-next-line no-await-in-loop -- one page after another.
    const [query] = await jmap([
      [
        "Email/query",
        { accountId, filter: { inMailbox: mailboxId }, limit: 500 },
      ],
    ]);
    const { ids } = Ids.parse(query);
    if (ids.length === 0) {
      return;
    }
    // oxlint-disable-next-line no-await-in-loop -- one page after another.
    await jmap([["Email/set", { accountId, destroy: ids }]]);
  }
};

// --- The emails ---------------------------------------------------------------

const SENDERS = [
  ["Ana García", "ana"],
  ["Bob Stone", "bob"],
  ["Carla Ruiz", "carla"],
  ["Dev Updates", "updates"],
  ["Elena Novak", "elena"],
  ["Facturas", "billing"],
] as const;
const TOPICS = [
  "Weekly report",
  "Presupuesto del proyecto",
  "Lunch on Friday?",
  "Release notes",
  "Factura pendiente",
  "Design review",
  "Viaje a Lisboa",
  "Quarterly numbers",
] as const;
/** Thread sizes, repeated: about a fifth of the emails are replies. */
const SIZES = [1, 1, 2, 1, 3, 1, 1, 1, 2, 5] as const;
const HOUR = 3_600_000;

interface Seed {
  readonly key: string;
  readonly email: Record<string, unknown>;
}

const pick = <T>(items: readonly T[], index: number): T => {
  const item = items[index % items.length];
  if (item === undefined) {
    throw new Error("empty list");
  }
  return item;
};

/** Newest conversation first; each one a few hours older than the last. */
const buildEmails = (mailboxId: string): Seed[] => {
  const seeds: Seed[] = [];
  const now = Date.now();
  for (let thread = 0; seeds.length < COUNT; thread += 1) {
    const size = Math.min(pick(SIZES, thread), COUNT - seeds.length);
    const topic = `${pick(TOPICS, thread)} #${thread + 1}`;
    const start = now - (thread + 1) * 3 * HOUR;
    const ids: string[] = [];
    for (let position = 0; position < size; position += 1) {
      const [name, local] = pick(SENDERS, thread + position);
      const messageId = `load-${thread}-${position}@${DOMAIN}`;
      const last = position === size - 1;
      seeds.push({
        email: {
          bodyValues: {
            body: {
              value: `Load test email ${position + 1} of ${size} in conversation ${thread + 1}.\n\nGenerated by docker/seed-load-test.ts.\n`,
            },
          },
          from: [{ email: `${local}@${DOMAIN}`, name }],
          keywords: {
            ...(last && thread % 3 === 0 ? {} : { $seen: true }),
            ...(last && thread % 17 === 0 ? { $flagged: true } : {}),
          },
          mailboxIds: { [mailboxId]: true },
          messageId: [messageId],
          receivedAt: new Date(start + position * 20 * 60_000).toISOString(),
          subject: position === 0 ? topic : `Re: ${topic}`,
          textBody: [{ partId: "body", type: "text/plain" }],
          to: [{ email: USER, name: "Alice" }],
          ...(position === 0
            ? {}
            : { inReplyTo: [ids.at(-1)], references: [...ids] }),
        },
        key: `m${thread}x${position}`,
      });
      ids.push(messageId);
    }
  }
  return seeds;
};

// --- Main ----------------------------------------------------------------------

const existing = await findFolder();

if (remove) {
  if (existing === undefined) {
    console.log(`No "${FOLDER}" folder: nothing to remove.`);
  } else {
    await emptyFolder(existing.id);
    await jmap([["Mailbox/set", { accountId, destroy: [existing.id] }]]);
    console.log(`Removed "${FOLDER}" and its emails.`);
  }
} else if (existing?.totalEmails === COUNT) {
  console.log(`"${FOLDER}" already has ${COUNT} emails: nothing to do.`);
} else {
  let mailboxId = existing?.id;
  if (mailboxId === undefined) {
    const [created] = await jmap([
      ["Mailbox/set", { accountId, create: { box: { name: FOLDER } } }],
    ]);
    mailboxId = z
      .object({ created: z.object({ box: z.object({ id: z.string() }) }) })
      .parse(created).created.box.id;
  } else {
    await emptyFolder(mailboxId);
  }
  const seeds = buildEmails(mailboxId);
  // Oldest first, so replies come after what they answer; 100 per call.
  const ordered = seeds.toReversed();
  for (let start = 0; start < ordered.length; start += 100) {
    const batch = ordered.slice(start, start + 100);
    // oxlint-disable-next-line no-await-in-loop -- threads need their parents first.
    await jmap([
      [
        "Email/set",
        {
          accountId,
          create: Object.fromEntries(
            batch.map((seed) => [seed.key, seed.email])
          ),
        },
      ],
    ]);
  }
  console.log(`Created ${seeds.length} emails in alice's "${FOLDER}" folder.`);
}
