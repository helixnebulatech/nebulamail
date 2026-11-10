// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// TEST-ONLY helpers (not in package.json `exports`): a fake `fetch` that
// plays a DoH server and the target servers, and records every request.

import type { WebFetch } from "./stream";

export const DOH = "https://doh.test/dns-query";

export interface FakeDns {
  readonly A?: readonly string[];
  readonly AAAA?: readonly string[];
  /** DNS status code; 3 = NXDOMAIN. Default 0 (or 3 when no records). */
  readonly status?: number;
}

export type Route = (init: RequestInit) => Response | Promise<Response>;

export interface FakeNet {
  readonly fetch: WebFetch;
  /** Target requests (not DoH), in order. */
  readonly requests: { readonly url: string; readonly init: RequestInit }[];
  /** DoH queries as `name/type`. */
  readonly queries: string[];
}

const dnsJson = (dns: FakeDns | null | undefined, type: number): Response => {
  const records = (type === 1 ? dns?.A : dns?.AAAA) ?? [];
  const status =
    dns?.status ??
    ((dns?.A ?? []).length + (dns?.AAAA ?? []).length === 0 ? 3 : 0);
  return Response.json(
    {
      Answer: records.map((data) => ({ TTL: 300, data, name: "x", type })),
      Status: status,
    },
    { headers: { "content-type": "application/dns-json" } }
  );
};

/**
 * `dns(name)` answers DoH queries (null or undefined = NXDOMAIN); `routes` maps a
 * full target URL to a response. Unknown targets throw like a refused
 * connection.
 */
export const fakeNet = (
  dns: (name: string) => FakeDns | null | undefined,
  routes: Readonly<Record<string, Route>> = {}
): FakeNet => {
  const requests: { url: string; init: RequestInit }[] = [];
  const queries: string[] = [];
  const fetch: WebFetch = async (input, init) => {
    await Promise.resolve();
    const url = new URL(input);
    if (`${url.origin}${url.pathname}` === DOH) {
      const name = url.searchParams.get("name") ?? "";
      const type = Number(url.searchParams.get("type"));
      queries.push(`${name}/${type}`);
      return dnsJson(dns(name), type);
    }
    requests.push({ init, url: input });
    const route = routes[input];
    if (route === undefined) {
      throw new TypeError("fetch failed");
    }
    return await route(init);
  };
  return { fetch, queries, requests };
};
