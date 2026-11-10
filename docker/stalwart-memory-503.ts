// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// A real 503 from Stalwart's memory budget, and what
// `jmap-client` does with it:
//
//   bun run dev:memory-503
//
// Stalwart 1.0.1 answers a download with 503 and `Retry-After: 5` when the
// bytes it would hold for it don't fit in `maxBufferedMemory` (of
// `SystemSettings`; by default most of the free memory, so a dev machine
// never sees it). A lone download always fits: only one next to another
// fails. So this:
//   1. uploads a 12 MiB blob as bob (never referenced: Stalwart drops it);
//   2. lowers `maxBufferedMemory` to its minimum (16 MiB) and reloads the
//      settings: two downloads of it at once no longer fit;
//   3. downloads it 4 times at once with plain `fetch` (the 503s), then 2
//      and 4 times at once through `jmap-client` (its retry);
//   4. always puts `maxBufferedMemory` back as it was and reloads, also on
//      Ctrl-C. While it's low, mail arriving at the dev Stalwart may be
//      refused for a moment (SMTP shares the budget).
//
// Seen 2026-11-01 with 1.0.1: plain fetch, 1 of 4 gets 200 and 3 get 503
// with `Retry-After: 5`; `jmap-client` gets all of 2 after one retry, and
// with 4 at once the retries met again 5 s later and one failed after its
// two retries, before `spread` (retry.ts) put up to 2.5 s more, at random,
// on each wait. With it, all 4 come back fine in 7–8 s (three runs).

import { existsSync, readFileSync } from "node:fs";
import path from "node:path";

import { z } from "zod";

import {
  AccountId,
  BlobId,
  JmapClient,
} from "../packages/jmap-client/src/index.ts";

const ROOT = path.join(import.meta.dirname, "..");
const STALWART_URL = "http://localhost:8080";
const USER = "bob@nebulamail.test";
const BLOB_BYTES = 12 * 1024 * 1024;
const LOW_BUDGET = 16 * 1024 * 1024;
const AT_ONCE = 4;

const env = (() => {
  const file = path.join(ROOT, "docker/.env");
  const lines = existsSync(file) ? readFileSync(file, "utf-8").split("\n") : [];
  return (key: string): string => {
    const line = lines.find((entry) => entry.startsWith(`${key}=`));
    if (line === undefined) {
      throw new Error(`No ${key} in docker/.env: run dev:setup`);
    }
    return line.slice(key.length + 1);
  };
})();

const basic = (user: string, password: string): string =>
  `Basic ${btoa(`${user}:${password}`)}`;
const admin = basic(env("STALWART_ADMIN_USER"), env("STALWART_ADMIN_PASSWORD"));
const bob = basic(USER, env("TEST_BOB_PASSWORD"));

const Responses = z.object({
  methodResponses: z.array(
    z.tuple([z.string(), z.record(z.string(), z.unknown()), z.string()])
  ),
});

type Call = readonly [string, Record<string, unknown>];

/** Runs admin JMAP calls and fails on any method error. */
const jmap = async (
  calls: readonly Call[]
): Promise<Record<string, unknown>[]> => {
  const response = await fetch(`${STALWART_URL}/jmap/`, {
    body: JSON.stringify({
      methodCalls: calls.map(([name, args], index) => [
        name,
        args,
        String(index),
      ]),
      using: ["urn:ietf:params:jmap:core", "urn:stalwart:jmap"],
    }),
    headers: { authorization: admin, "content-type": "application/json" },
    method: "POST",
  });
  if (!response.ok) {
    throw new Error(`JMAP HTTP ${response.status}: ${await response.text()}`);
  }
  const { methodResponses } = Responses.parse(await response.json());
  return methodResponses.map(([name, result]) => {
    if (name === "error" || result.notUpdated !== undefined) {
      throw new Error(`JMAP error: ${JSON.stringify(result)}`);
    }
    return result;
  });
};

const Settings = z.object({
  list: z.tuple([z.object({ maxBufferedMemory: z.number().nullable() })]),
});

const budget = async (): Promise<number | null> => {
  const [got] = await jmap([
    [
      "x:SystemSettings/get",
      { ids: ["singleton"], properties: ["maxBufferedMemory"] },
    ],
  ]);
  return Settings.parse(got).list[0].maxBufferedMemory;
};

const setBudget = async (bytes: number | null): Promise<void> => {
  await jmap([
    [
      "x:SystemSettings/set",
      { update: { singleton: { maxBufferedMemory: bytes } } },
    ],
    ["x:Action/set", { create: { reload: { "@type": "ReloadSettings" } } }],
  ]);
};

const client = await JmapClient.connect({
  fetch: async (input, init) => {
    const headers = new Headers(init?.headers);
    headers.set("Authorization", bob);
    const response = await fetch(input, { ...init, headers });
    if (String(input).includes("/download/")) {
      const after = response.headers.get("retry-after");
      console.log(
        `    ${new Date().toISOString().slice(17, 23)} GET ${response.status}${after === null ? "" : ` (Retry-After ${after})`}`
      );
    }
    return response;
  },
  getAccessToken: async () => await Promise.resolve("unused"),
  sessionUrl: `${STALWART_URL}/.well-known/jmap`,
});
const accountId = AccountId.parse(
  client.primaryAccountId("urn:ietf:params:jmap:mail")
);

const bytes = new Uint8Array(BLOB_BYTES);
for (let at = 0; at < BLOB_BYTES; at += 65_536) {
  crypto.getRandomValues(bytes.subarray(at, at + 65_536));
}
const { blobId } = await client.upload(accountId, new Blob([bytes]));
const url = `${STALWART_URL}/jmap/download/${accountId}/${blobId}/blob.bin`;

/** `AT_ONCE` plain downloads at once: status (and Retry-After) of each. */
const plainBurst = async (): Promise<string> => {
  const answers = await Promise.all(
    Array.from({ length: AT_ONCE }, async () => {
      const response = await fetch(url, { headers: { authorization: bob } });
      await response.arrayBuffer();
      const after = response.headers.get("retry-after");
      return `${response.status}${after === null ? "" : ` (Retry-After ${after})`}`;
    })
  );
  return answers.join(", ");
};

/** `count` downloads at once through jmap-client. */
const clientBurst = async (count: number): Promise<string> => {
  const started = Date.now();
  const results = await Promise.allSettled(
    Array.from(
      { length: count },
      async () =>
        await client.downloadBlob({
          accountId,
          blobId: BlobId.parse(blobId),
          name: "blob.bin",
        })
    )
  );
  const answers = results.map((result) =>
    result.status === "fulfilled"
      ? "ok"
      : `failed (${result.reason instanceof Error ? result.reason.message : String(result.reason)})`
  );
  return `${answers.join(", ")} in ${String(Date.now() - started)} ms`;
};

const original = await budget();
let restored = false;
const restore = async (): Promise<void> => {
  if (!restored) {
    restored = true;
    await setBudget(original);
    console.log(`maxBufferedMemory back to ${String(original)}`);
  }
};
process.on("SIGINT", () => {
  void restore().finally(() => process.exit(130));
});

try {
  console.log(`maxBufferedMemory: ${String(original)}`);
  console.log(`plain, normal budget: ${await plainBurst()}`);
  await setBudget(LOW_BUDGET);
  console.log(`maxBufferedMemory set to ${String(LOW_BUDGET)}`);
  console.log(`plain, low budget: ${await plainBurst()}`);
  for (const count of [2, AT_ONCE]) {
    console.log(`jmap-client, ${String(count)} at once:`);
    // oxlint-disable-next-line no-await-in-loop -- one burst after the other.
    console.log(`  ${await clientBurst(count)}`);
  }
} finally {
  await restore();
}
console.log(`plain, after: ${await plainBurst()}`);
