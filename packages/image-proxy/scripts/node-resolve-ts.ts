// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Lets Node run this monorepo's TypeScript sources directly. Node 26 strips
// types natively but, unlike bundlers, does not add extensions to relative
// imports (`./fetch` → `./fetch.ts`). This hook retries a failed relative
// import with `.ts` and `/index.ts`.
// Usage: node --import ./scripts/node-resolve-ts.ts src/node/server.ts

import { registerHooks } from "node:module";
import type { ResolveFnOutput, ResolveHookContext } from "node:module";

type NextResolve = (
  specifier: string,
  context?: Partial<ResolveHookContext>
) => ResolveFnOutput;

const SUFFIXES = [".ts", "/index.ts"];

const isRelative = (specifier: string): boolean =>
  specifier.startsWith("./") || specifier.startsWith("../");

const isNotFound = (error: unknown): boolean =>
  typeof error === "object" &&
  error !== null &&
  "code" in error &&
  error.code === "ERR_MODULE_NOT_FOUND";

const withSuffix = (
  specifier: string,
  context: ResolveHookContext,
  nextResolve: NextResolve
): ResolveFnOutput | undefined => {
  for (const suffix of SUFFIXES) {
    try {
      return nextResolve(`${specifier}${suffix}`, context);
    } catch {
      // Try the next suffix.
    }
  }
  return undefined;
};

registerHooks({
  resolve: (specifier, context, nextResolve) => {
    try {
      return nextResolve(specifier, context);
    } catch (error) {
      const resolved =
        isNotFound(error) && isRelative(specifier)
          ? withSuffix(specifier, context, nextResolve)
          : undefined;
      if (resolved === undefined) {
        throw error;
      }
      return resolved;
    }
  },
});
