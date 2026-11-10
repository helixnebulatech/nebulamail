<!-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors -->
<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# @nebulamail/net-guard

SSRF protection for every server-side fetch of a URL we did not write: the image proxy, JMAP/OIDC discovery in the BFF, and List-Unsubscribe. It shares its spec and test vectors (`security/ssrf-vectors.json`) with the Rust crate `bridge/crates/net-guard`.

## What it guarantees

- Only `http`/`https`, no userinfo, only allowed ports (80/443 by default; `UrlPolicy` for other lists).
- Hosts are never checked as text. WHATWG URL parsing normalizes them first (decimal/octal/hex IPv4, `127。0。0。1`, IDN).
- An address must be **global unicast**. This is an allow-list, checked by our own range tables and by ipaddr.js; both must agree. IPv4 embedded in `::ffff:0:0/96`, `64:ff9b::/96` and `2002::/16` is extracted and checked. IPv4-compatible `::a.b.c.d`, Teredo and `64:ff9b:1::/48` are always blocked.
- Names that are never public are refused as `dns` before any lookup (like Stalwart 1.0): single labels (`intranet`, `localhost`) and the suffixes `localhost`, `invalid`, `local`, `onion`, `alt`, `arpa`, `internal`, `corp`, `home`, `mail`, `lan`, `intranet`, `localdomain` (`isNonPublicName`, same list as the Rust `DomainName::is_non_public`). `.test` and `.example` are left out on purpose: no real resolver answers them, and the tests use them as public names.
- DNS: A **and** AAAA are both resolved. If **any** answer is not public, or DNS fails or returns nothing, the URL is rejected.
- Results are branded types (`PublicAddress`, `ParsedTarget`, `ValidatedTarget`). Only the guard can produce them.

## Core (any runtime)

```ts
import { guardUrl, checkImage } from "@nebulamail/net-guard";

const result = await guardUrl(url, resolver); // resolver: (host) => Promise<{ A, AAAA }>
if (!result.ok) return reject(result.reason); // "scheme" | "port" | "private-ip" | "dns" | ...
// result.target.addresses: the validated IPs. Connect to one of these, nothing else.
```

`checkImage(contentType, bytes)` accepts png, jpeg, gif, webp, avif, ico and bmp when the magic bytes match the declared type. SVG is never accepted.

## Node adapter (also Deno and Bun)

```ts
import { guardedFetch } from "@nebulamail/net-guard/node";

const res = await guardedFetch(url, { expect: "image" });
if (res.ok) send(res.status, res.imageType, res.body);
```

- The connection is pinned to the validated IP. Each hop resolves the name once with c-ares (`cancelableResolver`: A and AAAA, the DNS servers of `/etc/resolv.conf`, no `/etc/hosts`; an abort or the deadline cancels the lookup and its retries), and a throw-away `undici.Agent` dials that IP. TLS still checks the certificate against the hostname. There is no second lookup, so DNS rebinding cannot work.
- Redirects are followed manually (at most 5), and the full guard runs again on each hop.
- Limits: connect 5 s, total 10 s (DNS + every hop + body), body 5 MB while streaming (with or without Content-Length).
- Decompression: requests send `Accept-Encoding: identity`, but gzip/deflate/br/zstd responses are decoded with a 5 MB limit on the output (protection against compression bombs).
- `HTTP(S)_PROXY` and the global dispatcher are ignored. Requests carry no cookies, no referrer and only generic headers. Nothing is logged, and errors are reduced to a `BlockReason`.
- `createGuardedFetch({ resolver, policy, limits })` takes a custom resolver or policy. Limits can only be tightened, never loosened.
- The same import works on **Deno** (unchanged: Deno loads the real `undici`) and on **Bun** (`createGuardedFetch` returns the Bun adapter below). `guardedFetch` builds its fetch on first use, so an unsupported runtime fails there and not on import.

### POST (List-Unsubscribe one-click)

```ts
const res = await guardedFetch(url, {
  body: {
    data: "List-Unsubscribe=One-Click",
    type: "application/x-www-form-urlencoded",
  },
  method: "POST",
  readBody: false, // only the status matters
  redirects: "same-host",
});
```

- Same guard, pinning, limits and vectors as GET. The body is at most 16 KB (`MAX_REQUEST_BODY_BYTES`) and its type must be a plain media type (no line breaks); a GET with a body or a POST without one throws `TypeError` before any lookup.
- Redirects as browsers do: 307/308 repeat the POST and its body (to the next guarded hop); 303, and 301/302 after a POST, continue as a GET without body.
- `redirects`: `"follow"` (default), `"same-host"` (a redirect to another hostname is not followed: its 3xx status is returned) or `"none"`.
- `readBody: false` never reads the response body.

## Cloudflare Workers (`@nebulamail/net-guard/workers`) and other runtimes

```ts
import {
  createDohResolver,
  createWorkersFetch,
} from "@nebulamail/net-guard/workers";

const fetch = createWorkersFetch({ resolver: createDohResolver() });
const res = await fetch(url, { expect: "image" }); // same FetchResult as Node
```

- **Not pinned.** Workers' `fetch` resolves the name itself and cannot be told which IP to use. Each hop runs the full guard first, with A and AAAA from DoH (`createDohResolver`, JSON API, Cloudflare by default, https only; answers cached for min(TTL, 60 s)). A failed DoH query is `dns`; NXDOMAIN is an empty answer, also `dns`.
- Residual risk: DNS rebinding between our DoH lookup and Cloudflare's own. It is accepted because Workers requests leave from Cloudflare's network (not your servers) and Cloudflare refuses private destinations. When that is not enough, use the Node adapter.
- Same as Node: schemes, ports, userinfo, IP literals, manual redirects with the guard on every hop (max 5), 10 s total, 5 MB body limit while streaming, image type and magic bytes. The runtime decodes gzip/br itself, so the limit counts decoded bytes (bombs are cut at 5 MB).
- GET only (POST throws `TypeError`). `connectTimeoutMs` cannot be applied; the total deadline covers it.
- Tests run on Node with a fake `fetch` that plays the DoH server and the targets, including every URL vector of `security/ssrf-vectors.json`.

Do not use Vercel's Edge runtime (no DNS control, no pinning).

## Bun (`@nebulamail/net-guard/bun`)

Bun replaces `undici` with a stub, so the Node adapter cannot pin there. `@nebulamail/net-guard/node` picks this adapter by itself under Bun; import `@nebulamail/net-guard/bun` (`createBunFetch`) only to require Bun. Same `FetchResult`, same options, GET and POST.

- Each hop runs the full guard, then `Bun.fetch` asks for `http(s)://<validated IP>:<port>/path` with `Host: <original authority>` and `tls.serverName: <hostname>`: Bun never resolves the name, and the certificate is checked against the hostname (also the SNI). IPv6 goes in brackets; an `https` IP literal gets no SNI and is checked against the IP.
- Always `tls.rejectUnauthorized: true` (Bun otherwise honours `NODE_TLS_REJECT_UNAUTHORIZED=0`), `proxy: false` (Bun otherwise uses `HTTP_PROXY`, `HTTPS_PROXY` and `ALL_PROXY`), `redirect: "manual"`, `decompress: false` (we decode with the 5 MB limit) and `keepalive: false` (no connection is ever reused, so one verified for a name never carries another).
- Bun does not try other addresses, so the adapter walks the validated list: a GET moves on after any connection or TLS error; a POST only after errors before anything was sent (refused, unreachable, TLS), so it is never sent twice.
- DNS: the same cancelable c-ares resolver as Node (`cancelableResolver`): an abort or the deadline cancels the lookup, nothing is left running. Bun rounds the per-try timeout up to whole seconds (the default, 2 s, is exact). Without `Resolver#cancel` the default resolver refuses to start.
- No separate connect timeout: the 10 s total covers it. `Bun.fetch` is used, not `globalThis.fetch`, so a library patching the global cannot drop these options.
- Fails closed: outside Bun, or on Bun older than `MIN_BUN_VERSION` (1.4.3, the version CI tests), creating it throws.

## Deno

The Node adapter works unchanged on Deno 2: Deno loads the real `undici` from npm. **Never** use Deno's own `fetch` for this: `Deno.createHttpClient` cannot take a resolver, and its `proxy: { transport: "tcp" }` sends `https://` requests in plain text. The image proxy has a start script with minimal permissions (`packages/image-proxy/scripts/start-deno.sh`).

## Tests on every runtime

```sh
bun run test       # Node
bun run test:bun   # inside Bun (vitest workers are Bun): the Bun adapter
bun run test:deno  # inside Deno: the Node adapter on Deno's node: layer
```

- `src/node/fetch.test.ts` (every `fetch` vector: rebinding, redirects to private addresses, sizes, bombs, slow-loris, proxy variables, POST) runs against whichever adapter `createGuardedFetch` picks. Its test-only `dial` hook maps an already validated public IP to a local server; under Bun it goes to the Bun adapter's own hook.
- `src/bun/fetch.test.ts` adds, under Bun: a throw-away CA made with `openssl`, HTTPS servers for `img.test`, `other.test` and `v6.test` behind a TCP relay that reads the SNI from the ClientHello and counts connections; certificate for another name, unknown CA, `NODE_TLS_REJECT_UNAUTHORIZED=0`, IPv6 (AAAA and literal), next address for GET and POST, no reuse, redirects, and `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` set (checking that plain `fetch` does use them). Under Node and Deno it checks that the adapter refuses to start.
- `src/http/resolver.test.ts`, the default resolver of each adapter (in `src/node/fetch.test.ts`) and the URL vectors through c-ares (`src/vectors.test.ts`) run against a fake DNS server (`src/http/fake-dns.ts`) on all three runtimes.
- `test:bun`/`test:deno` set `EXPECT_RUNTIME`, and a test fails if the suite did not really run there. CI runs both (job `runtimes`).

## Abuse limits (rate limiting, budgets, concurrency)

(`@nebulamail/net-guard`):

```ts
import {
  createConcurrencyLimiter,
  createFetchBudget,
  createRateLimiter,
} from "@nebulamail/net-guard";

// Token buckets by key prefix; `batch:global` is one bucket for everybody.
const allow = createRateLimiter({
  rules: {
    "batch:ip": { capacity: 30, refillPerSecond: 0.5 },
    "batch:global": { capacity: 600, refillPerSecond: 20 },
  },
});
allow("batch:ip:203.0.113.9"); // true / false

// One email: every hop of every fetch takes a request; bodies fit the bytes left.
const budget = createFetchBudget({
  maxRequests: 180,
  maxBytes: 10 * 1024 * 1024,
});
const res = await guardedFetch(url, {
  allowRedirect: (next) => !isTracker(next), // caller rule on every redirect target
  budget,
  expect: "image",
});
if (res.ok && !budget.takeBytes(res.body.byteLength)) stopTheRest();

// Downloads in flight for the whole process, with a bounded queue.
const slots = createConcurrencyLimiter({ max: 32, maxQueue: 1024 });
await slots.run(async () => await guardedFetch(url), signal);
```

- The rate limiter is in memory: one process, isolate or instance. With several replicas each counts alone, so the real limit is about N × the rule. `RateLimiter` may be async, so a remote limiter (Cloudflare's Rate Limiting binding, used by the image proxy on Workers) fits the same type.
- `allowRedirect` and `budget` are caller rules on top of the guard, which still runs in full on every hop. Their refusals have their own reasons, outside `BlockReason` (the contract shared with Rust and `ssrf-vectors.json`): `refused` (`allowRedirect` said no; the target is never requested) and `budget` (no request left). A body over the bytes left is `size`. `FetchFailureReason` is the union.
- `createConcurrencyLimiter().run` rejects with `QueueFullError` when the queue is full, and with the signal's reason when the signal aborts while waiting (the task never runs).
