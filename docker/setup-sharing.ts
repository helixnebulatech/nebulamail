// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Test data for shared folders and the storage bar:
//   bun run dev:share
//
// - alice shares her folder "Compartida con Bob" with bob (read and
//   organize), bob shares "Compartida con Alice" with alice (read only);
//   each folder gets one sample email.
// - alice gets a 100 MB disk quota, so the sidebar shows a storage bar
//   (bob keeps none: no bar).
//
// Needs `bun run dev:setup` first (accounts and passwords in docker/.env).
// Safe to run again: it only creates or changes what is missing.

import { randomBytes } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";

import { z } from "zod";

const ROOT = path.join(import.meta.dirname, "..");
const DOCKER_ENV = path.join(ROOT, "docker/.env");
const STALWART_URL = "http://127.0.0.1:8080";
const DOMAIN = "nebulamail.test";
const ALICE_QUOTA_BYTES = 100 * 1024 * 1024;

const USING = [
  "urn:ietf:params:jmap:core",
  "urn:ietf:params:jmap:mail",
  "urn:ietf:params:jmap:mail:share",
  "urn:ietf:params:jmap:principals",
] as const;

const readEnv = (): Map<string, string> => {
  const env = new Map<string, string>();
  if (!existsSync(DOCKER_ENV)) {
    return env;
  }
  for (const line of readFileSync(DOCKER_ENV, "utf-8").split("\n")) {
    const groups = /^(?<key>[A-Z0-9_]+)=(?<value>.*)$/u.exec(line)?.groups;
    if (groups?.key !== undefined && groups.value !== undefined) {
      env.set(groups.key, groups.value);
    }
  }
  return env;
};

interface Credentials {
  readonly user: string;
  readonly password: string;
}

const basic = (auth: Credentials): string =>
  `Basic ${btoa(`${auth.user}:${auth.password}`)}`;

const MethodResponses = z.object({
  methodResponses: z.array(
    z.tuple([z.string(), z.record(z.string(), z.unknown()), z.string()])
  ),
});

type Call = readonly [string, Record<string, unknown>];

/** Runs JMAP calls, failing on any method or set error. */
const jmap = async (
  auth: Credentials,
  calls: readonly Call[],
  using: readonly string[] = USING
): Promise<Record<string, unknown>[]> => {
  const response = await fetch(`${STALWART_URL}/jmap/`, {
    body: JSON.stringify({
      methodCalls: calls.map(([name, args], index) => [
        name,
        args,
        String(index),
      ]),
      using,
    }),
    headers: { authorization: basic(auth), "content-type": "application/json" },
    method: "POST",
  });
  if (!response.ok) {
    throw new Error(`JMAP HTTP ${response.status}: ${await response.text()}`);
  }
  const { methodResponses } = MethodResponses.parse(await response.json());
  return methodResponses.map(([name, args]) => {
    if (name === "error") {
      throw new Error(`JMAP error: ${JSON.stringify(args)}`);
    }
    for (const key of ["notCreated", "notUpdated", "notDestroyed"]) {
      const failed = args[key];
      if (
        failed !== undefined &&
        failed !== null &&
        Object.keys(failed).length > 0
      ) {
        throw new Error(`${name} ${key}: ${JSON.stringify(failed)}`);
      }
    }
    return args;
  });
};

const Session = z.object({
  primaryAccounts: z.object({ "urn:ietf:params:jmap:mail": z.string() }),
});

const accountIdOf = async (auth: Credentials): Promise<string> => {
  const response = await fetch(`${STALWART_URL}/jmap/session`, {
    headers: { authorization: basic(auth) },
  });
  return Session.parse(await response.json()).primaryAccounts[
    "urn:ietf:params:jmap:mail"
  ];
};

const RIGHTS = {
  read: {
    mayAddItems: false,
    mayCreateChild: false,
    mayDelete: false,
    mayReadItems: true,
    mayRemoveItems: false,
    mayRename: false,
    maySetKeywords: false,
    maySetSeen: false,
    mayShare: false,
    maySubmit: false,
  },
  write: {
    mayAddItems: true,
    mayCreateChild: false,
    mayDelete: false,
    mayReadItems: true,
    mayRemoveItems: true,
    mayRename: false,
    maySetKeywords: true,
    maySetSeen: true,
    mayShare: false,
    maySubmit: false,
  },
} as const;

const Mailboxes = z.object({
  list: z.array(
    z.object({
      id: z.string(),
      name: z.string(),
      parentId: z.string().nullable(),
      shareWith: z
        .record(z.string(), z.record(z.string(), z.boolean()))
        .nullable()
        .optional(),
      totalEmails: z.number(),
    })
  ),
});

const Principals = z.object({
  list: z.array(z.object({ email: z.string().nullish(), id: z.string() })),
});

const sampleEmail = (from: string, to: string, subject: string): string =>
  [
    `From: ${from}`,
    `To: ${to}`,
    `Subject: ${subject}`,
    `Date: ${new Date().toUTCString()}`,
    `Message-ID: <${randomBytes(8).toString("hex")}@${DOMAIN}>`,
    "MIME-Version: 1.0",
    "Content-Type: text/plain; charset=utf-8",
    "",
    "Este correo está en una carpeta compartida.",
    "",
  ].join("\r\n");

/** Creates the folder if missing, shares it, and puts one email in it. */
const shareFolder = async (
  owner: Credentials,
  folderName: string,
  withEmail: string,
  rights: (typeof RIGHTS)[keyof typeof RIGHTS],
  email: { readonly from: string; readonly to: string }
): Promise<void> => {
  const accountId = await accountIdOf(owner);
  const [boxes, , people] = await jmap(owner, [
    [
      "Mailbox/get",
      {
        accountId,
        properties: ["name", "parentId", "shareWith", "totalEmails"],
      },
    ],
    ["Principal/query", { accountId, filter: { email: withEmail } }],
    [
      "Principal/get",
      {
        "#ids": { name: "Principal/query", path: "/ids", resultOf: "1" },
        accountId,
        properties: ["email"],
      },
    ],
  ]);
  const principal = Principals.parse(people).list.find(
    (entry) => entry.email === withEmail
  );
  if (principal === undefined) {
    throw new Error(`No principal for ${withEmail}`);
  }
  let folder = Mailboxes.parse(boxes).list.find(
    (box) => box.name === folderName && box.parentId === null
  );
  if (folder === undefined) {
    const [created] = await jmap(owner, [
      [
        "Mailbox/set",
        {
          accountId,
          create: { box: { name: folderName, parentId: null } },
        },
      ],
    ]);
    const { id } = z
      .object({ created: z.object({ box: z.object({ id: z.string() }) }) })
      .parse(created).created.box;
    folder = { id, name: folderName, parentId: null, totalEmails: 0 };
    console.log(`Created folder "${folderName}" for ${owner.user}`);
  }
  const current = folder.shareWith?.[principal.id];
  const same = Object.entries(rights).every(
    ([right, value]) => current?.[right] === value
  );
  if (!same) {
    await jmap(owner, [
      [
        "Mailbox/set",
        {
          accountId,
          update: { [folder.id]: { [`shareWith/${principal.id}`]: rights } },
        },
      ],
    ]);
    console.log(`Shared "${folderName}" with ${withEmail}`);
  }
  if (folder.totalEmails === 0) {
    const upload = await fetch(`${STALWART_URL}/jmap/upload/${accountId}/`, {
      body: sampleEmail(email.from, email.to, `Hola desde «${folderName}»`),
      headers: {
        authorization: basic(owner),
        "content-type": "message/rfc822",
      },
      method: "POST",
    });
    const { blobId } = z
      .object({ blobId: z.string() })
      .parse(await upload.json());
    await jmap(owner, [
      [
        "Email/import",
        {
          accountId,
          emails: { m: { blobId, mailboxIds: { [folder.id]: true } } },
        },
      ],
    ]);
    console.log(`Added a sample email to "${folderName}"`);
  }
};

const Accounts = z.object({
  list: z.array(
    z.object({
      id: z.string(),
      name: z.string(),
      quotas: z.record(z.string(), z.number()).nullish(),
    })
  ),
});

/** alice gets a disk quota, so the storage bar has something to show. */
const setAliceQuota = async (admin: Credentials): Promise<void> => {
  const [, got] = await jmap(
    admin,
    [
      ["x:Account/query", {}],
      [
        "x:Account/get",
        {
          "#ids": { name: "x:Account/query", path: "/ids", resultOf: "0" },
          properties: ["name", "quotas"],
        },
      ],
    ],
    ["urn:ietf:params:jmap:core", "urn:stalwart:jmap"]
  );
  const alice = Accounts.parse(got).list.find((item) => item.name === "alice");
  if (alice === undefined) {
    throw new Error("alice not found: run bun run dev:setup first");
  }
  if (alice.quotas?.maxDiskQuota === ALICE_QUOTA_BYTES) {
    return;
  }
  await jmap(
    admin,
    [
      [
        "x:Account/set",
        {
          update: {
            [alice.id]: { quotas: { maxDiskQuota: ALICE_QUOTA_BYTES } },
          },
        },
      ],
    ],
    ["urn:ietf:params:jmap:core", "urn:stalwart:jmap"]
  );
  console.log("alice: 100 MB disk quota");
};

const credentials = (env: Map<string, string>, name: string): Credentials => {
  const password = env.get(`TEST_${name.toUpperCase()}_PASSWORD`);
  if (password === undefined) {
    throw new Error(
      `Missing TEST_${name.toUpperCase()}_PASSWORD: run bun run dev:setup`
    );
  }
  return { password, user: `${name}@${DOMAIN}` };
};

const main = async (): Promise<void> => {
  const env = readEnv();
  const alice = credentials(env, "alice");
  const bob = credentials(env, "bob");
  await shareFolder(alice, "Compartida con Bob", bob.user, RIGHTS.write, {
    from: `Alice <${alice.user}>`,
    to: `Bob <${bob.user}>`,
  });
  await shareFolder(bob, "Compartida con Alice", alice.user, RIGHTS.read, {
    from: `Bob <${bob.user}>`,
    to: `Alice <${alice.user}>`,
  });
  const adminUser = env.get("STALWART_ADMIN_USER");
  const adminPassword = env.get("STALWART_ADMIN_PASSWORD");
  if (adminUser !== undefined && adminPassword !== undefined) {
    await setAliceQuota({ password: adminPassword, user: adminUser });
  }
  console.log("Sharing test data ready.");
};

await main();
