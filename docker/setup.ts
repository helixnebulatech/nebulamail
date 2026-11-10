// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Prepares the local Stalwart for development, without manual steps:
//   bun run dev:setup
//
// 1. Finishes Stalwart's initial setup (bootstrap mode) if needed.
// 2. Dev settings: 15 min access tokens, CORS for JMAP with Bearer tokens
// directory queries for the share dialogs, logs on stdout,
//    and Stalwart's own Authentication-Results on every path (RFC 8601),
//    applied with a settings reload.
// 3. Test accounts alice@, bob@ and carol@nebulamail.test, and the group
//    ventas@ ("Ventas") with bob in it, to share with a group.
// 4. OAuth client "nebula-dev" (confidential, with the four scopes the app
//    asks for) for apps/web, and the app's local config (session and image
//    ticket keys, proxy origin).
// 5. A few sample emails in alice's inbox.
//
// Secrets go to git-ignored files: docker/.env and apps/web/.env.local.
// Safe to run again: it only creates what is missing (and updates the
// OAuth client, e.g. to add scopes to one made before Stalwart 1.0).

import { spawnSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { setTimeout as sleep } from "node:timers/promises";

import { z } from "zod";

const ROOT = path.join(import.meta.dirname, "..");
const DOCKER_ENV = path.join(ROOT, "docker/.env");
const WEB_ENV = path.join(ROOT, "apps/web/.env.local");
const STALWART_URL = "http://127.0.0.1:8080";
const PUBLIC_URL = "http://localhost:8080";
const APP_URL = "http://localhost:3001";
const IMAGE_PROXY_ORIGIN = "http://img.localhost:3003";
const DOMAIN = "nebulamail.test";
const OAUTH_CLIENT_ID = "nebula-dev";
const TEST_USERS = ["alice", "bob", "carol"] as const;
const OAUTH_SCOPES = {
  calendars: true,
  contacts: true,
  files: true,
  mail: true,
} as const;

// --- .env files -------------------------------------------------------------

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

/** Sets keys in an env file, keeping comments and other keys. */
const writeEnv = (file: string, values: Record<string, string>): void => {
  const lines = existsSync(file) ? readFileSync(file, "utf-8").split("\n") : [];
  const pending = new Map(Object.entries(values));
  const updated = lines.map((line) => {
    const key = /^(?<key>[A-Z0-9_]+)=/u.exec(line)?.groups?.key;
    const value = key === undefined ? undefined : pending.get(key);
    if (key === undefined || value === undefined) {
      return line;
    }
    pending.delete(key);
    return `${key}=${value}`;
  });
  while (updated.at(-1) === "") {
    updated.pop();
  }
  for (const [key, value] of pending) {
    updated.push(`${key}=${value}`);
  }
  writeFileSync(file, `${updated.join("\n")}\n`, { mode: 0o600 });
};

const secret = (): string => randomBytes(24).toString("base64url");

// --- JMAP -------------------------------------------------------------------

interface Credentials {
  readonly user: string;
  readonly password: string;
}

const MethodResponses = z.object({
  methodResponses: z.array(
    z.tuple([z.string(), z.record(z.string(), z.unknown()), z.string()])
  ),
});

type Call = readonly [string, Record<string, unknown>];

const basic = (auth: Credentials): string =>
  `Basic ${btoa(`${auth.user}:${auth.password}`)}`;

/** Runs JMAP calls and fails on any method error. Returns each response's arguments. */
const jmap = async (
  auth: Credentials,
  calls: readonly Call[],
  using: readonly string[] = ["urn:ietf:params:jmap:core", "urn:stalwart:jmap"]
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

const first = <T>(items: readonly T[]): T => {
  const [item] = items;
  if (item === undefined) {
    throw new Error("Empty JMAP response");
  }
  return item;
};

const Ids = z.object({ ids: z.array(z.string()) });
const List = <T extends z.ZodType>(item: T) =>
  z.object({ list: z.array(item) });

// --- Steps ------------------------------------------------------------------

const waitForStalwart = async (): Promise<void> => {
  for (let attempt = 0; attempt < 60; attempt += 1) {
    try {
      // oxlint-disable-next-line no-await-in-loop -- polling: each try waits for the previous one.
      const response = await fetch(`${STALWART_URL}/healthz/live`);
      if (response.ok) {
        return;
      }
    } catch {
      // Not up yet.
    }
    // oxlint-disable-next-line no-await-in-loop -- polling interval.
    await sleep(1000);
  }
  throw new Error(
    "Stalwart is not answering. Start it: docker compose -f docker/compose.yaml up -d stalwart"
  );
};

const restartStalwart = async (): Promise<void> => {
  const restart = spawnSync(
    "docker",
    [
      "compose",
      "-f",
      path.join(ROOT, "docker/compose.yaml"),
      "restart",
      "stalwart",
    ],
    {
      stdio: "inherit",
    }
  );
  if (restart.status !== 0) {
    throw new Error("docker compose restart failed");
  }
  await waitForStalwart();
};

/**
 * Applies changed settings without a restart (Stalwart 1.0's
 * `ReloadSettings` action; it changes nothing if they have an error).
 */
const reloadSettings = async (auth: Credentials): Promise<void> => {
  await jmap(auth, [
    ["x:Action/set", { create: { reload: { "@type": "ReloadSettings" } } }],
  ]);
};

const recoveryAdmin = (env: Map<string, string>): Credentials => {
  const value = env.get("STALWART_RECOVERY_ADMIN");
  const separator = value?.indexOf(":") ?? -1;
  if (value === undefined || separator < 1) {
    throw new Error(
      "Set STALWART_RECOVERY_ADMIN=admin:<password> in docker/.env (see docker/README.md)"
    );
  }
  return {
    password: value.slice(separator + 1),
    user: value.slice(0, separator),
  };
};

/** Stalwart starts in bootstrap mode until its initial setup is saved. */
const bootstrap = async (env: Map<string, string>): Promise<void> => {
  const recovery = recoveryAdmin(env);
  const [current] = await jmap(recovery, [["x:Bootstrap/get", { ids: null }]]);
  if (List(z.unknown()).parse(current).list.length === 0) {
    return;
  }
  console.log("Stalwart: initial setup");
  const [result] = await jmap(recovery, [
    [
      "x:Bootstrap/set",
      {
        update: {
          singleton: {
            defaultDomain: DOMAIN,
            generateDkimKeys: false,
            defaultHostname: "localhost",
            requestTlsCertificate: false,
          },
        },
      },
    ],
  ]);
  const admin = z
    .object({
      updated: z.object({
        singleton: z.object({ secret: z.string(), username: z.string() }),
      }),
    })
    .parse(result).updated.singleton;
  writeEnv(DOCKER_ENV, {
    STALWART_ADMIN_PASSWORD: admin.secret,
    STALWART_ADMIN_USER: admin.username,
  });
  env.set("STALWART_ADMIN_USER", admin.username);
  env.set("STALWART_ADMIN_PASSWORD", admin.secret);

  console.log("Stalwart: restarting to leave bootstrap mode");
  await restartStalwart();
};

const admin = (env: Map<string, string>): Credentials => {
  const user = env.get("STALWART_ADMIN_USER");
  const password = env.get("STALWART_ADMIN_PASSWORD");
  if (user === undefined || password === undefined) {
    throw new Error(
      "Missing STALWART_ADMIN_USER/STALWART_ADMIN_PASSWORD in docker/.env"
    );
  }
  return { password, user };
};

const DEV_SETTINGS = {
  // short access tokens; the BFF keeps the refresh token.
  // Stalwart refuses unknown client ids by itself and has no switch for
  // dynamic registration:
  // public clients may always register, but only with loopback or
  // private-scheme redirect URIs (docs/static.md).
  oidc: { accessTokenExpiry: 15 * 60 * 1000 },
  // Bearer tokens only (no cookies), so `*` is safe.
  // The default of 1000 requests a minute cuts the tests short: they all
  // come from one account and one IP.
  http: {
    rateLimitAuthenticated: { count: 100_000, period: 60_000 },
    usePermissiveCors: true,
  },
  // Stalwart 1.0 refuses `Principal/query` to users by default ("directory
  // queries disabled"): without it the share dialogs can't find anyone
  // (operators set the same, docs/static.md).
  sharing: { allowDirectoryQueries: true },
} as const;

const differs = (
  current: unknown,
  wanted: Record<string, unknown>
): boolean => {
  const item = first(
    List(z.record(z.string(), z.unknown())).parse(current).list
  );
  return Object.entries(wanted).some(
    ([key, value]) => JSON.stringify(item[key]) !== JSON.stringify(value)
  );
};

/** Applies DEV_SETTINGS. Returns true if the settings must be reloaded. */
const configure = async (auth: Credentials): Promise<boolean> => {
  const [oidc, http, sharing] = await jmap(auth, [
    [
      "x:OidcProvider/get",
      { ids: ["singleton"], properties: Object.keys(DEV_SETTINGS.oidc) },
    ],
    [
      "x:Http/get",
      { ids: ["singleton"], properties: Object.keys(DEV_SETTINGS.http) },
    ],
    [
      "x:Sharing/get",
      { ids: ["singleton"], properties: Object.keys(DEV_SETTINGS.sharing) },
    ],
  ]);
  if (
    !differs(oidc, DEV_SETTINGS.oidc) &&
    !differs(http, DEV_SETTINGS.http) &&
    !differs(sharing, DEV_SETTINGS.sharing)
  ) {
    return false;
  }
  await jmap(auth, [
    ["x:OidcProvider/set", { update: { singleton: DEV_SETTINGS.oidc } }],
    ["x:Http/set", { update: { singleton: DEV_SETTINGS.http } }],
    ["x:Sharing/set", { update: { singleton: DEV_SETTINGS.sharing } }],
  ]);
  return true;
};

// A migrated Stalwart logs to files in /var/log/stalwart/, which the image
// doesn't have: nothing is saved. On stdout they are in `docker compose logs
// stalwart` (and in the CI's "Server logs"), and don't depend on the container.
const STDOUT_TRACER = {
  "@type": "Stdout",
  ansi: false,
  buffered: false,
  enable: true,
  level: "info",
} as const;

/** Returns true if the settings must be reloaded. */
const logToStdout = async (auth: Credentials): Promise<boolean> => {
  const [tracers] = await jmap(auth, [["x:Tracer/get", { ids: null }]]);
  const { list } = List(
    z.object({ "@type": z.string(), id: z.string() })
  ).parse(tracers);
  const files = list.filter((tracer) => tracer["@type"] === "Log");
  const hasStdout = list.some((tracer) => tracer["@type"] === "Stdout");
  if (files.length === 0 && hasStdout) {
    return false;
  }
  await jmap(auth, [
    [
      "x:Tracer/set",
      {
        destroy: files.map((tracer) => tracer.id),
        ...(hasStdout ? {} : { create: { stdout: STDOUT_TRACER } }),
      },
    ],
  ]);
  return true;
};

// Authentication-Results (RFC 8601): by default Stalwart adds its own only
// to mail arriving on port 25, and keeps the ones senders write. Mail sent
// by a local user (JMAP, SMTP 465) then has the sender's header on top, and
// it can claim anything ("dmarc=pass header.d=paypal.com"; tested
// 2026-10-23). Adding Stalwart's header on every path puts the server's own
// verdict on top (e.g. "localhost; none" for local mail), which is the only
// one the app reads (apps/web/src/mail/auth-results.ts).
const AUTH_RESULTS_ALWAYS = { else: "true", match: {} };

/** Returns true if the settings must be reloaded. */
const addAuthResultsEverywhere = async (
  auth: Credentials
): Promise<boolean> => {
  const [stage] = await jmap(auth, [
    [
      "x:MtaStageData/get",
      { ids: ["singleton"], properties: ["addAuthResultsHeader"] },
    ],
  ]);
  const current = z
    .object({
      addAuthResultsHeader: z.object({
        else: z.unknown(),
        match: z.record(z.string(), z.unknown()),
      }),
    })
    .safeParse(first(List(z.unknown()).parse(stage).list));
  if (
    current.success &&
    current.data.addAuthResultsHeader.else === "true" &&
    Object.keys(current.data.addAuthResultsHeader.match).length === 0
  ) {
    return false;
  }
  await jmap(auth, [
    [
      "x:MtaStageData/set",
      { update: { singleton: { addAuthResultsHeader: AUTH_RESULTS_ALWAYS } } },
    ],
  ]);
  console.log("Stalwart: Authentication-Results on every path");
  return true;
};

const domainId = async (auth: Credentials): Promise<string> => {
  const [, got] = await jmap(auth, [
    ["x:Domain/query", {}],
    [
      "x:Domain/get",
      {
        "#ids": { name: "x:Domain/query", path: "/ids", resultOf: "0" },
        properties: ["name"],
      },
    ],
  ]);
  const domain = List(z.object({ id: z.string(), name: z.string() }))
    .parse(got)
    .list.find((item) => item.name === DOMAIN);
  if (domain === undefined) {
    throw new Error(`Domain ${DOMAIN} not found`);
  }
  return domain.id;
};

const passwordKey = (name: string): string =>
  `TEST_${name.toUpperCase()}_PASSWORD`;

const createUsers = async (
  auth: Credentials,
  env: Map<string, string>
): Promise<void> => {
  const [, got] = await jmap(auth, [
    ["x:Account/query", {}],
    [
      "x:Account/get",
      {
        "#ids": { name: "x:Account/query", path: "/ids", resultOf: "0" },
        properties: ["name"],
      },
    ],
  ]);
  const existing = new Set(
    List(z.object({ name: z.string() }))
      .parse(got)
      .list.map((item) => item.name)
  );
  const domain = await domainId(auth);
  const missing = TEST_USERS.filter((name) => {
    const key = passwordKey(name);
    if (existing.has(name) && !env.has(key)) {
      throw new Error(
        `${name} exists but ${key} is missing in docker/.env; delete the account or add the password`
      );
    }
    return !existing.has(name);
  });
  if (missing.length === 0) {
    return;
  }
  const passwords = new Map(missing.map((name) => [name, secret()]));
  await jmap(auth, [
    [
      "x:Account/set",
      {
        create: Object.fromEntries(
          [...passwords].map(([name, password]) => [
            name,
            {
              "@type": "User",
              credentials: { "0": { "@type": "Password", secret: password } },
              description: "Test account",
              domainId: domain,
              name,
              roles: { "@type": "User" },
            },
          ])
        ),
      },
    ],
  ]);
  for (const [name, password] of passwords) {
    writeEnv(DOCKER_ENV, { [passwordKey(name)]: password });
    env.set(passwordKey(name), password);
    console.log(`Created ${name}@${DOMAIN}`);
  }
};

// The group the bridge's group-sharing test shares with
// (bridge/crates/jmap/tests/server/sharing.rs, `alice_shares_with_a_group`):
// Stalwart shows it in DAV as `/dav/pal/ventas@nebulamail.test/`, with its
// description as `displayname` and `calendar-user-type` GROUP.
const TEST_GROUP = {
  description: "Ventas",
  members: ["bob"],
  name: "ventas",
} as const;

const createGroup = async (auth: Credentials): Promise<void> => {
  const [, got] = await jmap(auth, [
    ["x:Account/query", {}],
    [
      "x:Account/get",
      {
        "#ids": { name: "x:Account/query", path: "/ids", resultOf: "0" },
        properties: ["name", "@type", "memberGroupIds"],
      },
    ],
  ]);
  const accounts = List(
    z.object({
      "@type": z.string(),
      id: z.string(),
      memberGroupIds: z.record(z.string(), z.boolean()).optional(),
      name: z.string(),
    })
  ).parse(got).list;
  let groupId = accounts.find(
    (item) => item["@type"] === "Group" && item.name === TEST_GROUP.name
  )?.id;
  if (groupId === undefined) {
    const [created] = await jmap(auth, [
      [
        "x:Account/set",
        {
          create: {
            group: {
              "@type": "Group",
              description: TEST_GROUP.description,
              domainId: await domainId(auth),
              name: TEST_GROUP.name,
            },
          },
        },
      ],
    ]);
    groupId = z
      .object({ created: z.object({ group: z.object({ id: z.string() }) }) })
      .parse(created).created.group.id;
    console.log(`Created group ${TEST_GROUP.name}@${DOMAIN}`);
  }
  const id = groupId;
  const members: readonly string[] = TEST_GROUP.members;
  const joining = accounts.filter(
    (item) =>
      item["@type"] === "User" &&
      members.includes(item.name) &&
      item.memberGroupIds?.[id] !== true
  );
  if (joining.length === 0) {
    return;
  }
  await jmap(auth, [
    [
      "x:Account/set",
      {
        update: Object.fromEntries(
          joining.map((item) => [item.id, { [`memberGroupIds/${id}`]: true }])
        ),
      },
    ],
  ]);
  for (const item of joining) {
    console.log(`Added ${item.name} to ${TEST_GROUP.name}@${DOMAIN}`);
  }
};

const createOAuthClient = async (auth: Credentials): Promise<void> => {
  const webEnv = readEnv(WEB_ENV);
  const [query] = await jmap(auth, [
    ["x:OAuthClient/query", { filter: { clientId: OAUTH_CLIENT_ID } }],
  ]);
  const [existingId] = Ids.parse(query).ids;
  const clientSecret = webEnv.get("OAUTH_CLIENT_SECRET") ?? secret();
  const client = {
    clientId: OAUTH_CLIENT_ID,
    description: "Nebula Mail (local development)",
    redirectUris: { [`${APP_URL}/auth/callback`]: true },
    // A registered client may only ask for the scopes it has (with none,
    // only sign-in), and the app asks for these four
    // (apps/web/src/lib/oauth-scope.ts). Set on update too, in case they
    // changed.
    scopes: OAUTH_SCOPES,
    // Explicit, also on update: only the code and refresh grants (no
    // device code). Never first party: no Application names this client
    // (Stalwart computes `firstParty` from that; setting it does nothing).
    grantTypes: { authorizationCode: true, refreshToken: true },
    secret: clientSecret,
  };
  if (existingId === undefined) {
    await jmap(auth, [["x:OAuthClient/set", { create: { client } }]]);
    console.log(`Created OAuth client ${OAUTH_CLIENT_ID}`);
  } else {
    // Stalwart stores only a hash, so the secret is set again from the local file.
    await jmap(auth, [
      ["x:OAuthClient/set", { update: { [existingId]: client } }],
    ]);
  }
  writeEnv(WEB_ENV, {
    APP_URL,
    JMAP_SERVER: PUBLIC_URL,
    OAUTH_CLIENT_ID,
    OAUTH_CLIENT_SECRET: clientSecret,
    // Another host name, not just another port: cookies ignore ports, so on
    // plain localhost:3003 the proxy would receive the app's cookies.
    IMAGE_PROXY_ORIGIN,
    ...(webEnv.has("SESSION_KEYS")
      ? {}
      : { SESSION_KEYS: `1:${randomBytes(32).toString("base64url")}` }),
    ...(webEnv.has("IMAGE_PROXY_KEYS")
      ? {}
      : { IMAGE_PROXY_KEYS: `1:${randomBytes(32).toString("base64url")}` }),
  });
};

// --- Sample mail -------------------------------------------------------------

const MAIL_CAPS = [
  "urn:ietf:params:jmap:core",
  "urn:ietf:params:jmap:mail",
] as const;

const date = (minutesAgo: number): string =>
  new Date(Date.now() - minutesAgo * 60_000).toUTCString();

const sampleEmails = (): string[] => {
  const header = (
    subject: string,
    from: string,
    minutesAgo: number,
    extra = ""
  ): string =>
    [
      `From: ${from}`,
      `To: Alice <alice@${DOMAIN}>`,
      `Subject: ${subject}`,
      `Date: ${date(minutesAgo)}`,
      `Message-ID: <${randomBytes(8).toString("hex")}@${DOMAIN}>`,
      "MIME-Version: 1.0",
      extra,
    ]
      .filter((line) => line !== "")
      .join("\r\n");
  return [
    `${header("Bienvenida", `Bob <bob@${DOMAIN}>`, 600, "Content-Type: text/plain; charset=utf-8")}\r\n\r\nHola Alice,\r\n\r\nEste es un correo de prueba en texto plano.\r\nVisita https://example.com para más información.\r\n\r\nBob\r\n`,
    `${header("Boletín de octubre", "Newsletter <news@example.com>", 120, "Content-Type: text/html; charset=utf-8")}\r\n\r\n<html><body><h1>Novedades</h1><p>Un correo HTML con una <a href="https://example.com/?utm_source=mail&utm_medium=email">enlace con rastreo</a>.</p><img src="https://example.com/banner.png" alt="Banner" width="600" height="200"><img src="https://tracking.example.com/open.gif" width="1" height="1"></body></html>\r\n`,
    `${header("Factura adjunta", `Bob <bob@${DOMAIN}>`, 30, 'Content-Type: multipart/mixed; boundary="b1"')}\r\n\r\n--b1\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nTe adjunto la factura.\r\n--b1\r\nContent-Type: text/plain; name="factura.txt"\r\nContent-Disposition: attachment; filename="factura.txt"\r\n\r\nTotal: 42 EUR\r\n--b1--\r\n`,
  ];
};

const seedMail = async (env: Map<string, string>): Promise<void> => {
  const password = env.get("TEST_ALICE_PASSWORD");
  if (password === undefined) {
    return;
  }
  const alice: Credentials = { password, user: `alice@${DOMAIN}` };
  const session = await fetch(`${STALWART_URL}/jmap/session`, {
    headers: { authorization: basic(alice) },
  });
  const accountId = z
    .object({
      primaryAccounts: z.object({ "urn:ietf:params:jmap:mail": z.string() }),
    })
    .parse(await session.json()).primaryAccounts["urn:ietf:params:jmap:mail"];

  const [mailboxes, emails] = await jmap(
    alice,
    [
      ["Mailbox/query", { accountId, filter: { role: "inbox" } }],
      ["Email/query", { accountId, limit: 1 }],
    ],
    MAIL_CAPS
  );
  if (Ids.parse(emails).ids.length > 0) {
    return;
  }
  const inbox = first(Ids.parse(mailboxes).ids);
  const blobIds = await Promise.all(
    sampleEmails().map(async (raw) => {
      const upload = await fetch(`${STALWART_URL}/jmap/upload/${accountId}/`, {
        body: raw,
        headers: {
          authorization: basic(alice),
          "content-type": "message/rfc822",
        },
        method: "POST",
      });
      return z.object({ blobId: z.string() }).parse(await upload.json()).blobId;
    })
  );
  const imports = Object.fromEntries(
    blobIds.map((blobId, index) => [
      `m${index}`,
      {
        blobId,
        keywords: index === 0 ? { $seen: true } : {},
        mailboxIds: { [inbox]: true },
      },
    ])
  );
  await jmap(
    alice,
    [["Email/import", { accountId, emails: imports }]],
    MAIL_CAPS
  );
  console.log(
    `Imported ${Object.keys(imports).length} sample emails for alice`
  );
};

// --- Main ---------------------------------------------------------------------

const main = async (): Promise<void> => {
  const env = readEnv(DOCKER_ENV);
  await waitForStalwart();
  await bootstrap(env);
  const auth = admin(env);
  const settingsChanged = await configure(auth);
  const logsChanged = await logToStdout(auth);
  const authResultsChanged = await addAuthResultsEverywhere(auth);
  if (settingsChanged || logsChanged || authResultsChanged) {
    console.log("Stalwart: reloading the settings");
    await reloadSettings(auth);
  }
  await createUsers(auth, env);
  await createGroup(auth);
  await createOAuthClient(auth);
  await seedMail(env);
  console.log(
    `\nReady. Test login: alice@${DOMAIN} (password in docker/.env, TEST_ALICE_PASSWORD).`
  );
};

await main();
