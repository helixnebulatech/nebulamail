// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Playwright's global setup for `bun run test:static`: prepares the
// throwaway Stalwart (`stalwart-static`, docker/compose.yaml) as
// docs/static.md tells an operator, with its management API:
//
// 1. Initial setup (bootstrap), with the recovery admin of
//    STALWART_STATIC_RECOVERY_ADMIN (random, set by whoever started it).
// 2. The guide's response headers, and its own web interface off (the test
//    needs nothing from GitHub).
// 3. alice@nebulamail.test with a fresh random password, the public OAuth
//    client `nebulamail` with the exact redirect of the zip's login
//    page and the four scopes, and the zip as an `Application` from the
//    mounted file.
// 4. `ReloadSettings` and `UpdateApps` (Stalwart 1.0), so nothing waits for
//    a restart.
// 5. One email in alice's inbox (Email/import: nothing is sent).
// 6. The proxy that is the browser's only way out (proxy.ts), until the
//    tests end.
//
// Only ever this container: it is restarted by name in its own project
// (once, to leave bootstrap mode).

import { spawnSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { once } from "node:events";
import { setTimeout as sleep } from "node:timers/promises";

import { z } from "zod";

import {
  BASE_PATH,
  BODY_TEXT,
  CLIENT_ID,
  COMPOSE,
  DOMAIN,
  passwordEnv,
  REMOTE_IMAGE,
  RESPONSE_HEADERS,
  STALWART_URL,
  SUBJECT,
  USER,
} from "./env";
import { startProxy } from "./proxy";

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

const ADMIN_CAPS = ["urn:ietf:params:jmap:core", "urn:stalwart:jmap"];
const MAIL_CAPS = ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"];

const basic = (auth: Credentials): string =>
  `Basic ${btoa(`${auth.user}:${auth.password}`)}`;

/** Runs JMAP calls and fails on any method error or anything not done. */
const jmap = async (
  auth: Credentials,
  calls: readonly Call[],
  using: readonly string[] = ADMIN_CAPS
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
        typeof failed === "object" &&
        failed !== null &&
        Object.keys(failed).length > 0
      ) {
        throw new Error(`${name} ${key}: ${JSON.stringify(failed)}`);
      }
    }
    return args;
  });
};

const List = <T extends z.ZodType>(item: T) =>
  z.object({ list: z.array(item) });
const Ids = z.object({ ids: z.array(z.string()) });

const waitForStalwart = async (): Promise<void> => {
  for (let attempt = 0; attempt < 90; attempt += 1) {
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
    "The throwaway Stalwart is not answering on port 8095. Start it (docs/static.md, «Test it»)."
  );
};

const restart = async (): Promise<void> => {
  const result = spawnSync(
    "docker",
    [...COMPOSE, "restart", "stalwart-static"],
    {
      stdio: "inherit",
    }
  );
  if (result.status !== 0) {
    throw new Error("docker compose restart stalwart-static failed");
  }
  await waitForStalwart();
};

const recoveryAdmin = (): Credentials => {
  const value = process.env.STALWART_STATIC_RECOVERY_ADMIN ?? "";
  const separator = value.indexOf(":");
  if (separator < 1) {
    throw new Error(
      "Set STALWART_STATIC_RECOVERY_ADMIN=admin:<password>, the same one the container was started with."
    );
  }
  return {
    password: value.slice(separator + 1),
    user: value.slice(0, separator),
  };
};

/** Returns true if Stalwart was in bootstrap mode (it needs a restart). */
const bootstrap = async (admin: Credentials): Promise<boolean> => {
  const [current] = await jmap(admin, [["x:Bootstrap/get", { ids: null }]]);
  if (List(z.unknown()).parse(current).list.length === 0) {
    return false;
  }
  await jmap(admin, [
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
  return true;
};

/**
 * docs/static.md, «Response headers». Unknown OAuth clients need no setting:
 * Stalwart 1.0 always refuses them.
 */
const configure = async (admin: Credentials): Promise<void> => {
  const [apps] = await jmap(admin, [["x:Application/get", { ids: null }]]);
  const webUi = List(z.object({ id: z.string(), resourceUrl: z.string() }))
    .parse(apps)
    .list.filter((app) => app.resourceUrl.includes("stalwartlabs/webui"));
  await jmap(admin, [
    [
      "x:Http/set",
      {
        update: {
          singleton: {
            responseHeaders: RESPONSE_HEADERS,
            usePermissiveCors: false,
          },
        },
      },
    ],
    [
      "x:Application/set",
      {
        update: Object.fromEntries(
          webUi.map((app) => [app.id, { enable: false }])
        ),
      },
    ],
  ]);
};

const domainId = async (admin: Credentials): Promise<string> => {
  const [, got] = await jmap(admin, [
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

/** alice, with a new random password (also if she already exists). */
const createAlice = async (admin: Credentials): Promise<string> => {
  const password = randomBytes(18).toString("base64url");
  const credentials = { "0": { "@type": "Password", secret: password } };
  const [query] = await jmap(admin, [
    ["x:Account/query", { filter: { name: "alice" } }],
  ]);
  const [existing] = Ids.parse(query).ids;
  await jmap(admin, [
    [
      "x:Account/set",
      existing === undefined
        ? {
            create: {
              alice: {
                "@type": "User",
                credentials,
                description: "Static build test",
                domainId: await domainId(admin),
                name: "alice",
                roles: { "@type": "User" },
              },
            },
          }
        : { update: { [existing]: { credentials } } },
    ],
  ]);
  return password;
};

/**
 * docs/static.md: a public client (no secret) with the exact redirect and
 * the four scopes the app asks for (lib/oauth-scope.ts).
 */
const createClient = async (admin: Credentials): Promise<void> => {
  const [query] = await jmap(admin, [
    ["x:OAuthClient/query", { filter: { clientId: CLIENT_ID } }],
  ]);
  const [existing] = Ids.parse(query).ids;
  const client = {
    clientId: CLIENT_ID,
    description: "Nebula Mail (static build test)",
    redirectUris: { [`${STALWART_URL}${BASE_PATH}/login`]: true },
    scopes: { calendars: true, contacts: true, files: true, mail: true },
    // As docs/static.md: only code and refresh grants (never first party:
    // no Application names this client).
    grantTypes: { authorizationCode: true, refreshToken: true },
  };
  await jmap(admin, [
    [
      "x:OAuthClient/set",
      existing === undefined
        ? { create: { client } }
        : { update: { [existing]: client } },
    ],
  ]);
};

/** docs/static.md, «Install the zip», from the read-only mount. */
const installZip = async (admin: Credentials): Promise<void> => {
  const resourceUrl = "file:///opt/nebula-static/nebula-mail-static.zip";
  const [apps] = await jmap(admin, [["x:Application/get", { ids: null }]]);
  const installed = List(z.object({ resourceUrl: z.string() }))
    .parse(apps)
    .list.some((app) => app.resourceUrl === resourceUrl);
  if (!installed) {
    await jmap(admin, [
      [
        "x:Application/set",
        {
          create: {
            app: {
              description: "Nebula Mail",
              resourceUrl,
              unpackDirectory: "/var/lib/stalwart/apps",
              urlPrefix: { [BASE_PATH]: true },
            },
          },
        },
      ],
    ]);
  }
};

/** One HTML email with a remote image, imported (nothing is sent). */
const seedMail = async (alice: Credentials): Promise<void> => {
  const session = await fetch(`${STALWART_URL}/jmap/session`, {
    headers: { authorization: basic(alice) },
  });
  const accountId = z
    .object({
      primaryAccounts: z.object({ "urn:ietf:params:jmap:mail": z.string() }),
    })
    .parse(await session.json()).primaryAccounts["urn:ietf:params:jmap:mail"];
  const [inboxes, existing] = await jmap(
    alice,
    [
      ["Mailbox/query", { accountId, filter: { role: "inbox" } }],
      ["Email/query", { accountId, filter: { subject: SUBJECT } }],
    ],
    MAIL_CAPS
  );
  if (Ids.parse(existing).ids.length > 0) {
    return;
  }
  const [inbox] = Ids.parse(inboxes).ids;
  if (inbox === undefined) {
    throw new Error("alice has no inbox");
  }
  const raw = [
    "From: Bob <bob@nebulamail.test>",
    `To: Alice <${USER}>`,
    `Subject: ${SUBJECT}`,
    `Date: ${new Date().toUTCString()}`,
    `Message-ID: <${randomBytes(8).toString("hex")}@${DOMAIN}>`,
    "MIME-Version: 1.0",
    "Content-Type: text/html; charset=utf-8",
    "",
    `<html><body><p>${BODY_TEXT}.</p><img src="${REMOTE_IMAGE}" alt="Banner" width="600" height="200"></body></html>`,
    "",
  ].join("\r\n");
  const upload = await fetch(`${STALWART_URL}/jmap/upload/${accountId}/`, {
    body: raw,
    headers: { authorization: basic(alice), "content-type": "message/rfc822" },
    method: "POST",
  });
  const { blobId } = z
    .object({ blobId: z.string() })
    .parse(await upload.json());
  await jmap(
    alice,
    [
      [
        "Email/import",
        {
          accountId,
          emails: {
            m: { blobId, keywords: {}, mailboxIds: { [inbox]: true } },
          },
        },
      ],
    ],
    MAIL_CAPS
  );
};

/**
 * docs/static.md: settings and a new Application apply without a restart
 * (Stalwart 1.0). `UpdateApps` downloads and unpacks every enabled app.
 */
const reload = async (admin: Credentials): Promise<void> => {
  await jmap(admin, [
    [
      "x:Action/set",
      {
        create: {
          apps: { "@type": "UpdateApps" },
          settings: { "@type": "ReloadSettings" },
        },
      },
    ],
  ]);
};

/** Stalwart unpacks the zip in the background: wait for its index.html. */
const waitForApp = async (): Promise<void> => {
  for (let attempt = 0; attempt < 60; attempt += 1) {
    // oxlint-disable-next-line no-await-in-loop -- polling: each try waits for the previous one.
    const response = await fetch(`${STALWART_URL}${BASE_PATH}/`);
    // oxlint-disable-next-line no-await-in-loop -- the body of that same try.
    const body = await response.text();
    if (response.ok && body.includes("Content-Security-Policy")) {
      return;
    }
    // oxlint-disable-next-line no-await-in-loop -- polling interval.
    await sleep(1000);
  }
  throw new Error(`Stalwart doesn't serve the zip at ${BASE_PATH}/`);
};

const globalSetup = async (): Promise<() => Promise<void>> => {
  const admin = recoveryAdmin();
  await waitForStalwart();
  if (await bootstrap(admin)) {
    await restart();
  }
  await configure(admin);
  const password = await createAlice(admin);
  await createClient(admin);
  await installZip(admin);
  await reload(admin);
  await seedMail({ password, user: USER });
  await waitForApp();
  process.env[passwordEnv] = password;
  const proxy = await startProxy();
  return async () => {
    proxy.closeAllConnections();
    proxy.close();
    await once(proxy, "close");
  };
};

export default globalSetup;
