// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Runnable Node server for development and Docker:
//
//   IMAGE_PROXY_KEYS=1:<base64url 32 bytes> APP_URL=http://localhost:3000 \
//     node --import ./scripts/node-resolve-ts.ts src/node/server.ts
//
// `IMAGE_CACHE_DIR` (disk) or `IMAGE_CACHE_S3_*` (S3) enables cache mode;
// without them only batch mode works. `IMAGE_PRELOAD_SECRET` adds preload
// (needs a cache): the relay's URLs; with `IMAGE_PRELOAD_FULL_EMAIL=true`
// also the whole email from Stalwart's MTA Hook (privacy risk, README).
// Also runs on Bun (`bun src/node/server.ts`: net-guard picks its Bun
// adapter) and on Deno (`scripts/start-deno.sh`, the Node adapter with the
// real undici).

import { once } from "node:events";
import { createServer } from "node:http";
import path from "node:path";

import type { ImageUrlExtractor, PreloadConfig } from "../config";
import { FULL_EMAIL_WARNING, preloadOptions } from "../preload-env";
import type { PreloadStateStore } from "../preload/state";
import { createS3StateStore, createS3Storage } from "../s3/s3-storage";
import { parseTicketKeys } from "../tickets/ticket";
import { clientAddressFrom } from "./client-ip";
import { createDiskStateStore } from "./disk-state";
import { parseServerEnv, pickServerEnv } from "./env";
import type { ServerEnv } from "./env";
import { createNodeProxy } from "./proxy";

const SWEEP_INTERVAL_MS = 60 * 60 * 1000;

/** Our own configuration errors: their messages name variables, never values. */
const CONFIG_ERRORS = new Set(["EnvError", "ProxyConfigError", "TicketError"]);

/**
 * The full-email hook's HTML is parsed in worker threads with a deadline
 *the same parser and code as the BFF's tickets.
 */
const htmlExtractor = async (): Promise<ImageUrlExtractor> => {
  const { createWorkerExtractor } =
    await import("@nebulamail/mail-render/server-extract");
  return createWorkerExtractor();
};

/**
 * Preload, with its state next to the cache: S3 under `pre/`, disk in its
 * own folder (never swept nor evicted by the image size cap).
 */
const preloadConfig = async (
  env: ServerEnv
): Promise<PreloadConfig | undefined> => {
  let state: PreloadStateStore | undefined;
  if (env.s3 !== undefined) {
    state = createS3StateStore(env.s3);
  } else if (env.cacheDir !== undefined) {
    state = createDiskStateStore(path.join(env.cacheDir, "preload-state"));
  }
  if (env.preload === undefined || state === undefined) {
    return undefined;
  }
  return {
    ...(env.preload.fullEmail ? { extractor: await htmlExtractor() } : {}),
    secret: env.preload.secret,
    state,
    ...preloadOptions(env.preload),
  };
};

const start = async (): Promise<void> => {
  // Read by name: Deno allows only these variables (scripts/start-deno.sh).
  const env = parseServerEnv(pickServerEnv(process.env));
  const keys = await parseTicketKeys(env.keys);
  const storage =
    env.s3 === undefined
      ? undefined
      : createS3Storage({
          ...env.s3,
          ...(env.cacheTtlMs === undefined ? {} : { maxAgeMs: env.cacheTtlMs }),
        });
  const preload = await preloadConfig(env);
  const proxy = createNodeProxy({
    appOrigin: env.appOrigin,
    clientAddress: clientAddressFrom(env.clientIp),
    keys,
    ...(env.cacheDir === undefined ? {} : { cacheDir: env.cacheDir }),
    ...(env.cacheMaxBytes === undefined
      ? {}
      : { cacheMaxBytes: env.cacheMaxBytes }),
    ...(storage === undefined ? {} : { storage }),
    ...(preload === undefined ? {} : { preload }),
    ...(env.cacheTtlMs === undefined
      ? {}
      : { limits: { cacheTtlMs: env.cacheTtlMs } }),
    log: (event) => {
      if (event.event === "preload-full-email") {
        console.warn(FULL_EMAIL_WARNING);
      } else if (event.event !== "batch" && event.event !== "preload") {
        console.warn(JSON.stringify(event));
      }
    },
  });
  const server = createServer(proxy.listener);
  server.headersTimeout = 10_000;
  server.requestTimeout = 60_000;
  server.listen(env.port, env.host);
  await once(server, "listening");
  const address = server.address();
  const port =
    typeof address === "object" && address !== null ? address.port : env.port;
  const cached = env.cacheDir !== undefined || storage !== undefined;
  const modes = [
    "batch",
    ...(cached ? ["cache"] : []),
    ...(preload === undefined ? [] : ["preload"]),
    ...(preload?.fullEmail === true ? ["preload full email"] : []),
  ].join(", ");
  console.info(
    `image proxy listening on http://${env.host}:${port} (modes: ${modes})`
  );
  if (env.cacheDir !== undefined && env.cacheMaxBytes !== undefined) {
    console.info(
      `image proxy: disk cache capped at ${String(env.cacheMaxBytes)} bytes (IMAGE_CACHE_MAX_BYTES)`
    );
  }
  if (env.clientIp !== "socket") {
    console.warn(
      `image proxy: trusting ${env.clientIp} for the client address: only behind a proxy that sets it`
    );
  }

  const { sweep } = proxy;
  const runSweep = async (): Promise<void> => {
    try {
      await sweep?.();
    } catch {
      console.warn(JSON.stringify({ event: "sweep-failed" }));
    }
  };
  const sweeper =
    sweep === undefined
      ? undefined
      : setInterval(() => {
          void runSweep();
        }, SWEEP_INTERVAL_MS);
  sweeper?.unref();

  const stop = () => {
    clearInterval(sweeper);
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
      : "image proxy: failed to start"
  );
  process.exitCode = 1;
}
