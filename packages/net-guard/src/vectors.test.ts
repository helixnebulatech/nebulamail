// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";
import { z } from "zod";

import { fakeDns } from "./http/fake-dns";
import { createCancelableResolver } from "./http/resolver";
import { guardUrl } from "./index";
import type { Resolver } from "./index";
import { fail, later } from "./test-fixtures";

const UrlVector = z.object({
  dns: z
    .object({
      A: z.array(z.string()).optional(),
      AAAA: z.array(z.string()).optional(),
    })
    .optional(),
  expect: z.enum(["allow", "block"]),
  id: z.string(),
  input: z.string(),
  reason: z.string().min(1).optional(),
});

const Vectors = z.object({
  fetch: z.array(z.object({ expect: z.string(), id: z.string() })),
  url: z.array(UrlVector),
  version: z.literal(1),
});

const raw: unknown = JSON.parse(
  readFileSync(
    new URL("../../../security/ssrf-vectors.json", import.meta.url),
    "utf-8"
  )
);

describe("ssrf-vectors.json", () => {
  const vectors = Vectors.parse(raw);

  it("has unique ids", () => {
    const ids = [...vectors.url, ...vectors.fetch].map((v) => v.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("gives a reason for every blocked url", () => {
    const missing = vectors.url
      .filter((v) => v.expect === "block" && v.reason === undefined)
      .map((v) => v.id);
    expect(missing).toStrictEqual([]);
  });
});

describe("url vectors against the core guard", () => {
  const vectors = Vectors.parse(raw);

  it.each(vectors.url)("$id", async (vector) => {
    const unexpected: string[] = [];
    const resolver: Resolver = async (hostname) => {
      // No `dns` in the vector: the guard must decide without resolving.
      if (vector.dns === undefined) {
        unexpected.push(hostname);
        return await fail("resolver must not be called");
      }
      return await later({
        A: vector.dns.A ?? [],
        AAAA: vector.dns.AAAA ?? [],
      });
    };

    const expected =
      vector.expect === "allow"
        ? { ok: true }
        : { ok: false, reason: vector.reason };
    const result = await guardUrl(vector.input, resolver);

    expect(result).toMatchObject(expected);
    expect(Object.keys(result)).toHaveLength(2);
    expect(unexpected).toStrictEqual([]);
  });
});

/** The name the resolver gets: lower case, no final dot (`undefined` if none). */
const resolvedName = (input: string): string | undefined => {
  const hostname = URL.parse(input)?.hostname.toLowerCase();
  return hostname?.endsWith(".") === true ? hostname.slice(0, -1) : hostname;
};

// The same vectors, with the answers coming from c-ares (the default resolver
// of every pinned adapter) and a fake DNS server: what c-ares returns in Node,
// Deno and Bun (`::ffff:` texts, 6to4, NAT64, mixed A/AAAA, empty) goes through
// the same checks. One fake per vector: several vectors share a name with
// different answers.
describe("url vectors through the c-ares resolver", () => {
  const vectors = Vectors.parse(raw);

  it.each(vectors.url)("$id", async (vector) => {
    const name = resolvedName(vector.input);
    const zones = new Map(
      vector.dns === undefined || name === undefined
        ? []
        : [[name, { A: vector.dns.A ?? [], AAAA: vector.dns.AAAA ?? [] }]]
    );
    const dns = await fakeDns({ zones });
    try {
      const cares = createCancelableResolver({
        servers: [dns.server],
        timeoutMs: 1000,
        tries: 1,
      });
      const lookups: string[] = [];
      const resolver: Resolver = async (hostname, signal) => {
        lookups.push(hostname);
        return await cares(hostname, signal);
      };
      const expected =
        vector.expect === "allow"
          ? { ok: true }
          : { ok: false, reason: vector.reason };
      const result = await guardUrl(vector.input, resolver);

      expect(result).toMatchObject(expected);
      expect(Object.keys(result)).toHaveLength(2);
      // Without `dns` (and for names that are never public) the guard decides
      // without resolving; otherwise only the vector's own name is asked for.
      const asked = [...new Set(dns.queries.map((query) => query.name))];
      expect([lookups.length, asked]).toStrictEqual(
        vector.dns === undefined || lookups.length === 0 ? [0, []] : [1, [name]]
      );
    } finally {
      dns.close();
    }
  });
});
