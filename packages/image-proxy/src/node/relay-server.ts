// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Runnable preload relay (README "Preload › Relay"). Run it on the mail
// server, next to Stalwart, as its own user:
//
//   IMAGE_PRELOAD_RELAY_SECRET=<Stalwart → relay> \
//   IMAGE_PRELOAD_SECRET=<the proxy's secret> \
//   IMAGE_PRELOAD_UPSTREAM=https://img.example.com \
//     node --import ./scripts/node-resolve-ts.ts src/node/relay-server.ts
//
// Listens on 127.0.0.1:3004 by default. Only outgoing connection: the
// proxy (`IMAGE_PRELOAD_UPSTREAM`); it never downloads an image itself.

import { once } from "node:events";
import { createServer } from "node:http";

import { createWorkerExtractor } from "@nebulamail/mail-render/server-extract";

import { createRelay } from "../relay/relay";
import { toNodeListener } from "./http";
import { parseRelayEnv, pickRelayEnv } from "./relay-env";

const CONFIG_ERRORS = new Set(["EnvError", "RelayConfigError"]);

const start = async (): Promise<void> => {
  const env = parseRelayEnv(pickRelayEnv(process.env));
  const relay = createRelay({
    // Parsed in worker threads with a deadline: the same
    // parser and code as the proxy and the BFF.
    extractor: createWorkerExtractor(),
    hookSecret: env.hookSecret,
    upstream: env.upstream,
    upstreamSecret: env.upstreamSecret,
    ...(env.indexKey === undefined ? {} : { indexKey: env.indexKey }),
    ...(env.emailTokens ? { emailTokens: true } : {}),
    ...(env.maxMessageBytes === undefined
      ? {}
      : { maxMessageBytes: env.maxMessageBytes }),
    log: (event) => {
      if (event.event !== "relay-sent") {
        console.warn(JSON.stringify(event));
      }
    },
  });
  // Background sends keep running in this process: no waitUntil needed.
  const server = createServer(
    toNodeListener(relay, (req) => req.socket.remoteAddress)
  );
  server.headersTimeout = 10_000;
  server.requestTimeout = 60_000;
  server.listen(env.port, env.host);
  await once(server, "listening");
  const address = server.address();
  const port =
    typeof address === "object" && address !== null ? address.port : env.port;
  console.info(
    `preload relay listening on http://${env.host}:${port}, sending image URLs to ${new URL(env.upstream).origin}`
  );
  const stop = () => {
    server.close();
  };
  process.once("SIGINT", stop);
  process.once("SIGTERM", stop);
};

try {
  await start();
} catch (error) {
  console.error(
    error instanceof Error && CONFIG_ERRORS.has(error.name)
      ? error.message
      : "preload relay: failed to start"
  );
  process.exitCode = 1;
}
