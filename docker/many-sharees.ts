// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Many accounts to share a calendar or address book with, for the bridge's
// test of big sharing changes (sharing limits:
// `share_with_many_accounts` in bridge/crates/jmap/tests/server/sharing.rs):
//
//   bun docker/many-sharees.ts <stalwart|baikal|nextcloud> <count>
//   bun docker/many-sharees.ts <stalwart|baikal|nextcloud> --remove
//
// The accounts are `share-1` … `share-<count>`, with the addresses
// `share-<n>@nebulamail.test`. Nobody logs in with them (Stalwart gets a
// random password nobody keeps; Baïkal and Nextcloud an unusable one).
//
// - Stalwart (the dev one, `docker/.env`'s admin): the management API
//   (`x:Account/set`), 100 at a time. Remove them afterwards.
// - Baïkal and Nextcloud (test servers): straight into their databases
//   (SQLite, PostgreSQL) with `docker exec`, so that 10 000 take seconds,
//   not the hours of `occ user:add` with its password hashing. Nextcloud
//   finds them like any user: its user backend and principal search read
//   `oc_users` and the `settings`/`email` preference (stored as Nextcloud
//   stores it: string type, indexed flag, the indexed copy).
//
// Sharing with them in Nextcloud also fills the owner's "Recently
// contacted" address book (the contactsinteraction app), one card each:
// `--remove` deletes those too, or the contacts tests see thousands of
// cards there.
//
// Safe to run again: only the missing ones are created.

import { spawnSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";

import { z } from "zod";

const ROOT = path.join(import.meta.dirname, "..");
const DOCKER_ENV = path.join(ROOT, "docker/.env");
const STALWART_URL = "http://127.0.0.1:8080";
const DOMAIN = "nebulamail.test";
const PREFIX = "share-";
const BATCH = 100;

const names = (count: number): string[] =>
  Array.from({ length: count }, (_, index) => `${PREFIX}${index + 1}`);

const fail = (message: string): never => {
  console.error(message);
  process.exit(1);
};

// --- Stalwart -----------------------------------------------------------------

const readEnv = (file: string): Map<string, string> => {
  const env = new Map<string, string>();
  if (!existsSync(file)) {
    return env;
  }
  for (const line of readFileSync(file, "utf-8").split("\n")) {
    const groups = /^(?<key>[A-Z0-9_]+)=(?<value>.*)$/u.exec(line)?.groups;
    if (groups?.key !== undefined && groups.value !== undefined) {
      env.set(groups.key, groups.value);
    }
  }
  return env;
};

const MethodResponses = z.object({
  methodResponses: z.array(
    z.tuple([z.string(), z.record(z.string(), z.unknown()), z.string()])
  ),
});

type Call = readonly [string, Record<string, unknown>];

const jmap = async (
  calls: readonly Call[]
): Promise<Record<string, unknown>[]> => {
  const env = readEnv(DOCKER_ENV);
  const user = env.get("STALWART_ADMIN_USER");
  const password = env.get("STALWART_ADMIN_PASSWORD");
  if (user === undefined || password === undefined) {
    return fail(
      "Missing STALWART_ADMIN_USER/STALWART_ADMIN_PASSWORD in docker/.env"
    );
  }
  const response = await fetch(`${STALWART_URL}/jmap/`, {
    body: JSON.stringify({
      methodCalls: calls.map(([name, args], index) => [
        name,
        args,
        String(index),
      ]),
      using: ["urn:ietf:params:jmap:core", "urn:stalwart:jmap"],
    }),
    headers: {
      authorization: `Basic ${btoa(`${user}:${password}`)}`,
      "content-type": "application/json",
    },
    method: "POST",
  });
  if (!response.ok) {
    return fail(`JMAP HTTP ${response.status}: ${await response.text()}`);
  }
  const { methodResponses } = MethodResponses.parse(await response.json());
  return methodResponses.map(([name, args]) => {
    if (name === "error") {
      fail(`JMAP error: ${JSON.stringify(args)}`);
    }
    for (const key of ["notCreated", "notDestroyed"]) {
      const failed = args[key];
      if (
        failed !== undefined &&
        failed !== null &&
        Object.keys(failed).length > 0
      ) {
        fail(`JMAP ${key}: ${JSON.stringify(failed)}`);
      }
    }
    return args;
  });
};

const Listed = z.object({
  list: z.array(z.object({ id: z.string(), name: z.string() })),
});

const stalwartAccounts = async (): Promise<Map<string, string>> => {
  const [, got] = await jmap([
    ["x:Account/query", {}],
    [
      "x:Account/get",
      {
        "#ids": { name: "x:Account/query", path: "/ids", resultOf: "0" },
        properties: ["name"],
      },
    ],
  ]);
  return new Map(Listed.parse(got).list.map((item) => [item.name, item.id]));
};

const stalwart = async (count: number | "remove"): Promise<void> => {
  const existing = await stalwartAccounts();
  const ours = [...existing].filter(([name]) => /^share-\d+$/u.test(name));
  if (count === "remove") {
    const batches: string[][] = [];
    for (let at = 0; at < ours.length; at += BATCH) {
      batches.push(ours.slice(at, at + BATCH).map(([, id]) => id));
    }
    await Promise.all(
      batches.map(
        async (ids) => await jmap([["x:Account/set", { destroy: ids }]])
      )
    );
    console.log(`Stalwart: removed ${ours.length} accounts`);
    return;
  }
  const [, domains] = await jmap([
    ["x:Domain/query", {}],
    [
      "x:Domain/get",
      {
        "#ids": { name: "x:Domain/query", path: "/ids", resultOf: "0" },
        properties: ["name"],
      },
    ],
  ]);
  const domain = Listed.parse(domains).list.find(
    (item) => item.name === DOMAIN
  );
  if (domain === undefined) {
    return fail(`Domain ${DOMAIN} not found: run bun run dev:setup`);
  }
  const missing = names(count).filter((name) => !existing.has(name));
  const creates: Record<string, unknown>[] = [];
  for (let at = 0; at < missing.length; at += BATCH) {
    const create = Object.fromEntries(
      missing.slice(at, at + BATCH).map((name) => [
        name,
        {
          "@type": "User",
          credentials: {
            "0": {
              "@type": "Password",
              secret: randomBytes(24).toString("base64url"),
            },
          },
          description: "Sharing test account (docker/many-sharees.ts)",
          domainId: domain.id,
          name,
          roles: { "@type": "User" },
        },
      ])
    );
    creates.push(create);
  }
  await Promise.all(
    creates.map(async (create) => await jmap([["x:Account/set", { create }]]))
  );
  console.log(`Stalwart: ${count} accounts (${missing.length} new)`);
};

// --- Baïkal and Nextcloud -----------------------------------------------------

const exec = (container: string, command: string[], input: string): string => {
  const result = spawnSync("docker", ["exec", "-i", container, ...command], {
    encoding: "utf-8",
    input,
    maxBuffer: 64 * 1024 * 1024,
  });
  if (result.status !== 0) {
    return fail(`docker exec ${container} failed: ${result.stderr}`);
  }
  return result.stdout;
};

const values = (count: number, row: (name: string) => string): string =>
  names(count).map(row).join(",\n");

const baikal = (count: number | "remove"): void => {
  const db = "/var/www/baikal/Specific/db/db.sqlite";
  const like = `'${PREFIX}%'`;
  const sql =
    count === "remove"
      ? `DELETE FROM users WHERE username LIKE ${like};
         DELETE FROM principals WHERE uri LIKE 'principals/${PREFIX}%';`
      : `BEGIN;
         INSERT OR IGNORE INTO users (username, digesta1) VALUES
         ${values(count, (name) => `('${name}', 'x')`)};
         INSERT OR IGNORE INTO principals (uri, email, displayname) VALUES
         ${values(count, (name) => `('principals/${name}', '${name}@${DOMAIN}', '${name}')`)};
         COMMIT;`;
  exec("nebula-dev-baikal-1", ["sqlite3", db], sql);
  const total = exec(
    "nebula-dev-baikal-1",
    ["sqlite3", db],
    `SELECT count(*) FROM principals WHERE uri LIKE 'principals/${PREFIX}%';`
  );
  console.log(`Baïkal: ${total.trim()} sharing accounts`);
};

const nextcloud = (count: number | "remove"): void => {
  const psql = [
    "psql",
    "-v",
    "ON_ERROR_STOP=1",
    "-q",
    "-U",
    "nextcloud",
    "-d",
    "nextcloud",
  ];
  const like = `'${PREFIX}%'`;
  // `!` is never a valid password hash: nobody can log in with these.
  const sql =
    count === "remove"
      ? `DELETE FROM oc_preferences WHERE userid LIKE ${like};
         DELETE FROM oc_recent_contact WHERE uid LIKE ${like};
         DELETE FROM oc_users WHERE uid LIKE ${like};`
      : `BEGIN;
         INSERT INTO oc_users (uid, displayname, password, uid_lower) VALUES
         ${values(count, (name) => `('${name}', '${name}', '!', '${name}')`)}
         ON CONFLICT DO NOTHING;
         INSERT INTO oc_preferences (userid, appid, configkey, configvalue, type, flags, indexed) VALUES
         ${values(count, (name) => `('${name}', 'settings', 'email', '${name}@${DOMAIN}', 1, 2, '${name}@${DOMAIN}')`)}
         ON CONFLICT DO NOTHING;
         COMMIT;`;
  exec("nebula-dev-nextcloud-db-1", psql, sql);
  const total = exec(
    "nebula-dev-nextcloud-db-1",
    [
      ...psql,
      "-t",
      "-c",
      `SELECT count(*) FROM oc_users WHERE uid LIKE ${like};`,
    ],
    ""
  );
  console.log(`Nextcloud: ${total.trim()} sharing accounts`);
};

// --- main ---------------------------------------------------------------------

const [server, what] = process.argv.slice(2);
const count = what === "--remove" ? "remove" : Number(what);
if (
  count !== "remove" &&
  !(Number.isInteger(count) && count > 0 && count <= 10_000)
) {
  fail(
    "usage: bun docker/many-sharees.ts <stalwart|baikal|nextcloud> <1-10000|--remove>"
  );
}
if (server === "stalwart") {
  await stalwart(count);
} else if (server === "baikal") {
  baikal(count);
} else if (server === "nextcloud") {
  nextcloud(count);
} else {
  fail(`unknown server: ${server ?? "(none)"}`);
}
