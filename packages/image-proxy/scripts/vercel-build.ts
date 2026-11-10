// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Vercel build (README, "Vercel"): bundles `vercel/entry.ts` into one file with
// bun and writes it with Vercel's Build Output API (`.vercel/output`).
// Vercel's own Node builder compiles each file separately and keeps the
// workspace's extensionless imports, which Node then cannot resolve
// (ERR_MODULE_NOT_FOUND, seen on the first real deploy, 2026-10-26).
// Build Output also means only the function is published: no static copy of
// the sources.
//
//   bun scripts/vercel-build.ts      (vercel.json `buildCommand`)

import { execFileSync } from "node:child_process";
import { mkdir, rm, writeFile } from "node:fs/promises";
import path from "node:path";

const root = path.resolve(import.meta.dirname, "..");
const output = path.join(root, ".vercel", "output");
const fn = path.join(output, "functions", "api", "proxy.func");

await rm(output, { force: true, recursive: true });
await mkdir(fn, { recursive: true });

// The bun CLI, not `Bun.build`: this package has no Bun types.
execFileSync(
  "bun",
  [
    "build",
    path.join(root, "vercel", "entry.ts"),
    "--target=node",
    "--format=esm",
    `--outfile=${path.join(fn, "index.mjs")}`,
  ],
  { stdio: "inherit" }
);

// The HTML parser's worker thread (`@nebulamail/mail-render/server-extract`),
// as its own file next to the function: the full-email hook passes it as
// `workerUrl` (README, "Full email to the proxy").
execFileSync(
  "bun",
  [
    "build",
    path.join(root, "..", "mail-render", "src", "parse-worker.ts"),
    "--target=node",
    "--format=esm",
    `--outfile=${path.join(fn, "parse-worker.mjs")}`,
  ],
  { stdio: "inherit" }
);

const json = (value: unknown): string => `${JSON.stringify(value, null, 2)}\n`;

await writeFile(
  path.join(fn, ".vc-config.json"),
  json({
    handler: "index.mjs",
    launcherType: "Nodejs",
    maxDuration: 60,
    runtime: "nodejs24.x",
    shouldAddHelpers: false,
    supportsResponseStreaming: true,
  })
);

await mkdir(path.join(output, "static"), { recursive: true });
await writeFile(
  path.join(output, "static", "robots.txt"),
  "User-agent: *\nDisallow: /\n"
);

await writeFile(
  path.join(output, "config.json"),
  json({
    crons: [{ path: "/cron/sweep", schedule: "17 3 * * *" }],
    routes: [
      { handle: "filesystem" },
      // Every other path goes to the function, which reads `?__path=`.
      { dest: "/api/proxy?__path=$1", src: "^/(.*)$" },
    ],
    version: 3,
  })
);

console.log(`Vercel build output written to ${path.relative(root, output)}`);
