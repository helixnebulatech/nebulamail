<!-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# @nebulamail/image-proxy

Image proxy for remote images in emails. It runs on its **own origin** (e.g. `img.example.com`, never the app's), so it never sees the app's `__Host-` cookies. It only downloads URLs that arrive inside a ticket signed by the BFF, and every download goes through `@nebulamail/net-guard`.

## Entry points

| Import | Runs on | What |
| --- | --- | --- |
| `@nebulamail/image-proxy/tickets` | anywhere (WebCrypto) | `parseTicketKeys`, `signTicket`, `verifyTicket`, `imageUrl`, `MAX_TICKET_TTL_MS`, `signPreferenceRequest` (preload opt-out) |
| `@nebulamail/image-proxy` | anywhere (`Request`/`Response`) | `createProxyHandler(config)`, `createMemoryStorage`, types |
| `@nebulamail/image-proxy/node` | Node | `createNodeProxy`, `createDiskStorage`, `createRateLimiter` |
| `@nebulamail/image-proxy/s3` | anywhere (WebCrypto + `fetch`) | `createS3Storage`, `signRequest` (SigV4), `parseS3Env` |
| `@nebulamail/image-proxy/workers` | Cloudflare Workers | `createWorkersProxy`, `createR2Storage` (entry: `src/workers/worker.ts`) |
| `@nebulamail/image-proxy/vercel` | Vercel Functions, Node.js runtime | `createVercelProxy`, `createBlobStorage` (entry: `vercel/entry.ts`) |

## Which adapter?

|  | Node / Docker | Vercel (Node.js runtime) | Cloudflare Workers |
| --- | --- | --- | --- |
| Modes | `batch`; `cache` with disk or S3; `preload` | `batch`; `cache` with Vercel Blob or S3; `preload` | `batch`; `cache` with R2 or S3 (no `preload`) |
| Connection pinned to the checked IP | Yes | Yes (same code as Node) | **No** (platform limit, see below) |
| DNS | c-ares resolver, each answer checked | c-ares resolver, each answer checked | DoH (A + AAAA), each answer checked |
| Size limits | 5 MB per image, 10 MB per email | **3 MB** per image and per email (4.5 MB response limit) | 5 MB per image, 10 MB per email |
| Rate limit | In memory, per process | In memory, per instance | Cloudflare Rate Limiting bindings (per location), in memory per isolate for rules without one |
| Recommended | Yes | Yes | When you accept the residual risk |

All three share the same core (tickets, trackers, `GET` never downloads, response headers) and the same env validation. Every one runs the full `security/ssrf-vectors.json` battery in its tests.

## Tickets (BFF side)

```ts
const keys = await parseTicketKeys(env.IMAGE_PROXY_KEYS); // "2:<base64url 32 bytes>,1:<older>"
const token = await signTicket(keys, {
  accountId,
  emailId,
  url,
  expiresAt: Date.now() + 5 * 60_000,
});
// batch mode: POST { tickets: [...] } to `${proxyOrigin}/batch`
// cache mode: POST first, then <img src={imageUrl(proxyOrigin, token)}>
```

- Token: `1.<keyVersion>.<base64url JSON [accountId, emailId, url, expiresAt]>.<base64url HMAC>`, characters `[A-Za-z0-9_.-]`. The URL is readable in the token (the proxy needs it), so do not log full proxy paths in a reverse proxy.
- The MAC is HMAC-SHA256 over a length-prefixed encoding of (domain, version, key version, accountId, emailId, sha256(url), expiresAt). Checked with `crypto.subtle.verify`; tokens over 8 KB are refused before any crypto.
- Ids must be JMAP ids (`[A-Za-z0-9_-]{1,255}`), URLs absolute http(s) and at most 2048 characters, lifetime at most 10 minutes.
- Rotation: put the new key first. Old tickets keep working until the old key is removed. New key: `generateTicketKeyEntry(2)`.

## HTTP API

| Route | What |
| --- | --- |
| `POST /batch` `{ tickets, mode? }` | All tickets must be valid and for the **same account and email** (otherwise 400). Downloads every image at once: concurrency 6 per email and 32 in the whole process, 5 MB per image, 10 MB and 180 HTTP requests (redirects included) per email, 60 images, 30 s. `mode` defaults to `cache` when storage exists, else `batch`. The user may ask for `batch` even with storage. |
|  | Batch answer: `{ images: { [ticket]: { type, data /* base64 */ } \| { error } } }`. Cache answer: `{ images: { [ticket]: { cached: true } \| { error } } }`. |
| `GET /i/<ticket>` | Cache mode only. **Served from storage only; a GET never downloads.** 404 if not stored. Looks up the account's entry, then the preloaded one. |
| `POST /hooks/preload-urls` | Preload mode only (otherwise 404): image URLs and hashed recipients from the relay. See "Preload › Relay". |
| `POST /hooks/stalwart` | Preload with `IMAGE_PRELOAD_FULL_EMAIL=true` only (otherwise 404): Stalwart's MTA Hook with the whole email. See "Preload" (risks). |
| `POST /preload/preferences` | Preload mode only (otherwise 404): the BFF's signed opt-out / opt-in. See "Preload › Opt-out". |
| `GET /p/<email token>/<sha256 of the URL>` | Preload mode with email tokens only: one preloaded image of an email, for the static build. Storage only, never a download; 403 for a bad token, 404 if the image isn't that email's or isn't stored. See "Preload › Email tokens". |
| `GET /healthz` | `{ status, modes }` |

- Errors per image: `tracker`, `blocked`, `not-image`, `too-large`, `timeout`, `upstream`, `storage`. Request errors: `{ error: "<code>" }`. Neither ever contains the URL or the ticket.
- CORS: `Access-Control-Allow-Origin: <appOrigin>` exactly, `Vary: Origin`, no credentials. A preflight is answered only for `POST` + `content-type`. Other origins (or none) get 403.
- Images: `Content-Type` sniffed from the bytes (png, jpeg, gif, webp, avif, ico, bmp; never SVG), `X-Content-Type-Options: nosniff`, `Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; sandbox`, `Referrer-Policy: no-referrer`, `Cross-Origin-Resource-Policy: cross-origin`, `Cache-Control: private, max-age=<ticket lifetime left>, immutable`. No cookies.
- Trackers are checked again before downloading (mail-render's `isTrackerUrl`), in case a signing key leaks, **and on every redirect target** before it is requested (`allowRedirect` in net-guard): a redirect to a tracker gives `tracker`.
- `http://` URLs are tried over `https://` first. The proxy falls back to `http://` only when https fails at the connection level (refused, reset, TLS error). Both requests go through net-guard.

### Why GET never downloads

If each image were downloaded when the browser requests it, CSS like `:hover { background: url(…) }` would tell the sender which rule fired. Downloads therefore happen only in `POST /batch`, for the whole email. The `GET /i/` code path only reads storage and never calls the fetcher; tests assert this with a mock fetcher, both in the core and over HTTP.

## Node server

Every variable of every adapter is in `.env.example`.

```sh
IMAGE_PROXY_KEYS=1:<base64url 32 bytes> APP_URL=http://localhost:3001 bun run serve:dev
```

| Variable | Default |  |
| --- | --- | --- |
| `IMAGE_PROXY_KEYS` | required | Signing keys, shared with the BFF |
| `APP_URL` | required | Its origin is the only CORS origin allowed |
| `IMAGE_PROXY_PORT` | `3003` |  |
| `IMAGE_PROXY_CLIENT_IP` | `socket` | Client address for the per-IP rate limits: `socket`, `x-forwarded-for` (last entry) or `x-real-ip`. **Only behind a proxy that sets that header**, otherwise anyone fakes any IP; a missing or invalid header falls back to the socket |
| `IMAGE_PROXY_HOST` | `127.0.0.1` | `0.0.0.0` in Docker |
| `IMAGE_CACHE_DIR` | none | Enables cache mode with disk storage |
| `IMAGE_CACHE_MAX_BYTES` | 10 GiB (with `IMAGE_CACHE_DIR`) | Disk cap; it cannot be turned off. Over it, the account using the most loses its oldest entries first; shared (preloaded) entries go last. `preload-state/` lives on the same volume and is not counted: leave room for it |
| `IMAGE_CACHE_S3_*` | none | Cache mode with S3 instead of disk (see "S3 storage"); not both |
| `IMAGE_CACHE_TTL_DAYS` | `30` | Cache retention in whole days, from 1, **no maximum** (all adapters) |
| `IMAGE_PRELOAD_SECRET` | none | Enables preload (32+ characters, see "Preload"); needs a cache |
| `IMAGE_PRELOAD_FULL_EMAIL` | `false` | `true`: also accept the whole email from Stalwart (**privacy risk**, see "Preload") |
| `IMAGE_PRELOAD_MAX_MESSAGE_BYTES` | 25 MB | Bigger hook requests are accepted without preloading |
| `IMAGE_PRELOAD_MAX_BYTES_PER_RECIPIENT`, `IMAGE_PRELOAD_MAX_IMAGES_PER_RECIPIENT` | 256 MiB, 10 000 | Preload quota per recipient address (see "Preload") |
| `IMAGE_PRELOAD_INDEX_KEY` | `IMAGE_PRELOAD_SECRET` | Key of the recipient hashes (32+ characters) |

The same server runs on Node, Bun and Deno, always with the connection pinned to the validated IP. From the repo root (they read `apps/web/.env.local` and watch for changes):

| Runtime | Dev | Production (in `packages/image-proxy`) | How it pins |
| --- | --- | --- | --- |
| Node 26 | `bun run dev:proxy` | `bun run start` | net-guard's Node adapter (`undici`) |
| Bun ≥ 1.4.3 | `bun run dev:proxy:bun` | `bun run start:bun` | net-guard picks its Bun adapter (`Bun.fetch` to the IP, `tls.serverName`, `proxy: false`) |
| Deno 2 | `bun run dev:proxy:deno` | `bun run start:deno` | the Node adapter with the real `undici` from npm |

- Node 26 runs the TypeScript directly; `scripts/node-resolve-ts.ts` only adds the `.ts` extension to the repo's extensionless imports. Bun needs nothing. Deno uses `--sloppy-imports`.
- **Deno runs with minimal permissions** (`scripts/start-deno.sh`; extra `deno run` flags go after it, and `DENO` may point to the binary): `--allow-net` (Deno cannot express "public addresses only": net-guard decides), `--allow-env` only for the variables in the table above plus three `undici` reads at load time (`HTTP(S)_PROXY` are not allowed, so nothing can read them), `--allow-read` for `node_modules`, the HTML parser's worker module (`mail-render/src/parse-worker.ts`, full-email preload) and `IMAGE_CACHE_DIR`, `--allow-write` only for `IMAGE_CACHE_DIR`. No run, FFI or sys. The server reads its variables by name (`pickServerEnv`), never enumerating the environment. Deno's own `fetch`/`Deno.createHttpClient` proxy transport is never used for downloads: it sends https in clear. The S3 cache client uses the runtime's `fetch` (admin endpoint, not net-guard).
- Bun ignores `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and `NODE_TLS_REJECT_UNAUTHORIZED=0` for downloads (tested). It refuses to start on Bun older than 1.4.3.

Disk storage: files named `sha256(key)` (never a URL), written to a temp file and renamed, `0600` in `0700` directories, expiry and owner in a small header, expired files deleted on read and by an hourly sweep. The owner is an HMAC of the account hash under `owner.key` (random, `0600`, in the cache directory), so a directory listing cannot group the files of one account. Eviction is fair: down to 90 % of the cap, always from the account that uses the most bytes (its oldest files first); only when no account's files are left, the shared ones (`*preload` images, email token records), oldest first, logged as `shared-evicted`. Owner, size and time of every file are kept in memory (read once from the headers), so an eviction never reopens every file.

Rate limits (in memory, per process): `batch:ip` 30 burst, 1 every 2 s; `batch:account` 60 burst, 1/s; `batch:global` 600 burst, 20/s; `image:ip` 1200 burst, 20/s; `image:global` 30 000 burst, 1000/s; `preload:global` 120 burst, 2/s; `preload-prefs:ip` and `preload-prefs:global` 200 burst, 20/s. IPv6 clients are grouped by /64. `*:ip` is charged before any check; `*:global` only after a valid ticket, email token or BFF signature, so requests without credentials cannot spend everybody's bucket (and `batch:account` before `batch:global`). Behind a reverse proxy, set `IMAGE_PROXY_CLIENT_IP` (server) or pass `clientAddress` (`createNodeProxy`) to read the header that proxy sets. **With several replicas each one counts on its own**, so the real limit is about N × the numbers above (decided: no Redis); put a rate limit in the load balancer if you need an exact one.

## Cloudflare Workers

Deployed and tested for real on 2026-10-26 (`nebula-image-proxy.<account>.workers.dev`, batch and R2 cache mode; every blocked URL vector of `security/ssrf-vectors.json` that needs no fake DNS, plus redirects to `127.0.0.1` and `169.254.169.254` and names that resolve to private IPs, came back `blocked`). Steps (with your own Cloudflare account):

1. `cp wrangler.example.toml wrangler.toml` in this folder and set `name`, `APP_URL` and the route. Serve it on its own hostname, never the app's.
2. `wrangler secret put IMAGE_PROXY_KEYS` (same keys as the BFF).
3. Cache mode (optional): `wrangler r2 bucket create nebula-image-cache`, keep the `[[r2_buckets]]` binding `IMAGE_CACHE_BUCKET`, and add a lifecycle rule as a second net with the same days as `IMAGE_CACHE_TTL_DAYS`: `wrangler r2 bucket lifecycle add nebula-image-cache --prefix img/ --expire-days 30`. Or use S3 instead (the `IMAGE_CACHE_S3_*` variables, keys as secrets). Without either only batch mode works.
4. Rate limits: keep the five `[[ratelimits]]` bindings (names fixed, numbers yours; `period` 10 or 60 s). They need no extra setup in the dashboard. Counts are per Cloudflare location and approximate (Cloudflare's documentation); a rule without a binding falls back to an in-memory limit per isolate.
5. `wrangler deploy --dry-run` to check, then `wrangler deploy`. Wrangler bundles `src/workers/worker.ts` (no `nodejs_compat` needed: the bundle has no `node:` imports). Checked with wrangler 4.153 `--dry-run` (2026-10-26): every binding is recognised, about 940 KiB.

| Variable / binding | Default |  |
| --- | --- | --- |
| `IMAGE_PROXY_KEYS` (secret) | required | Signing keys, shared with the BFF |
| `APP_URL` | required | Its origin is the only CORS origin allowed |
| `IMAGE_CACHE_TTL_DAYS` | `30` | Whole days, from 1, no maximum |
| `IMAGE_PROXY_DOH_URL` | `https://cloudflare-dns.com/dns-query` | DoH JSON endpoint, https only |
| `IMAGE_CACHE_BUCKET` (R2 binding) | none | Enables cache mode |
| `IMAGE_CACHE_S3_*` | none | Cache mode with S3 instead of R2 (not both) |
| `RATE_LIMIT_BATCH_IP`, `RATE_LIMIT_BATCH_ACCOUNT`, `RATE_LIMIT_BATCH_GLOBAL`, `RATE_LIMIT_IMAGE_IP`, `RATE_LIMIT_IMAGE_GLOBAL` (Rate Limiting bindings) | none | One per rule; missing ones use memory |
| Cron trigger | `17 * * * *` | Deletes cache entries older than the retention |

The env is checked like the Node env on the first request; a bad env answers `500 {"error":"misconfigured"}` and logs the variable name, never its value.

Storage: R2 keys are `img/v1:<sha256(account)>:<sha256(url)>`. An entry expires `IMAGE_CACHE_TTL_DAYS` after upload (R2's `uploaded` date): deleted on read and by the cron.

### Security trade-offs on Workers, in plain words

- Workers' `fetch` takes a URL and does its own DNS lookup. We cannot tell it which IP to use. So before each request (and each redirect) we ask DoH for A and AAAA and refuse if any answer is private. Then Cloudflare looks the name up again.
- Between the two lookups an attacker who controls a domain can switch the answer (DNS rebinding). What protects you there is Cloudflare, not our code: Workers requests leave from Cloudflare's network, not from your servers, so `127.0.0.1` or `10.x` there is not _your_ machine, and Cloudflare documents that it refuses to connect to private and reserved addresses (we cannot test this). The attacker can at most make the Worker download from a different public host.
- Things we still check ourselves: scheme, port 80/443, no `user@host`, IP literals (decimal, hex, IPv6 tricks), every DoH answer, every redirect hop (max 5), 5 MB per image counted after decompression, 10 s per image, image type and magic bytes.
- Keep the Worker on a hostname with nothing else behind it (a dedicated zone or `workers.dev`): a Worker request to a hostname of its own zone can go straight to that zone's origin. Give it no bindings other than the R2 bucket.
- Every download carries `CF-Worker: <your zone>`: senders see the proxy's zone name (never the user's IP).
- Platform limits (check Cloudflare's current numbers): the free plan allows very little CPU time and about 50 subrequests per request. A batch of 60 images needs DoH queries (cached up to 60 s per name) plus one or more downloads per image, so plan on the paid plan. A refused subrequest shows up as `upstream` for that image.
- If this is not acceptable, run the Node adapter (or Vercel) and point the app at it.

## Vercel

Node.js runtime (or Vercel's Bun runtime: net-guard then picks its Bun adapter; only the Node runtime was deployed for real). The Edge runtime cannot control DNS or pin connections, so it would only have our text checks; the adapter refuses it (`EdgeRuntime` set). On Vercel the code is the same as the Node server: c-ares resolver, every address checked, the connection pinned to the checked IP.

Deployed and tested for real on 2026-10-26 (batch mode, same SSRF checks as Workers, all blocked). Steps:

1. New Vercel project with **Root Directory** `packages/image-proxy`, framework "Other", and "Include files outside the root directory" on. Vercel installs the monorepo from the root with bun (it detects `bun.lock`).
2. Env vars: `IMAGE_PROXY_KEYS`, `APP_URL`, optionally `IMAGE_CACHE_TTL_DAYS`. Rate limits are in memory per instance (several instances: approximate); add a Vercel Firewall rate-limit rule for a hard one.
3. `vercel.json` (already here) runs `bun scripts/vercel-build.ts`: it bundles `vercel/entry.ts` into one file and writes it with Vercel's Build Output API as the function `/api/proxy` (Node 24, `maxDuration` 60), routes every path to it (`?__path=` keeps the original path), adds the `/cron/sweep` cron and a `robots.txt` that disallows everything. Why not Vercel's own builder: it compiles each file separately and Node cannot resolve the workspace's extensionless imports (`ERR_MODULE_NOT_FOUND`); and an `outputDirectory` of `.` would publish the sources as static files. The entry is not in `api/` on purpose: Vercel would build that folder itself.
4. Deploy from a clean copy of the repo (`git archive HEAD`) or from Git, never from a working tree with `bridge/target` or worktrees in it (the CLI uploads everything not ignored).
5. Use its own domain (e.g. `img.example.com`), never the app's.

Cache mode with Vercel Blob (optional):

1. Create a Blob store and connect it to the project (sets `BLOB_READ_WRITE_TOKEN`).
2. `bun add @vercel/blob` in `packages/image-proxy`. It is not a dependency by default because it pulls about 30 packages.
3. Pass the module in `vercel/entry.ts`:

   ```ts
   import * as blob from "@vercel/blob";

   import { createVercelProxy } from "../src/vercel/index";

   const proxy = createVercelProxy({ blob });
   export default { fetch: proxy.fetch };
   ```

4. Set `IMAGE_CACHE_STORAGE=vercel-blob` and `CRON_SECRET` (16+ characters; Vercel Cron sends it as a bearer token to `/cron/sweep`). Without `CRON_SECRET` the proxy refuses to start: Blob has no lifecycle rules, so the cron is what enforces the retention.

| Variable | Default |  |
| --- | --- | --- |
| `IMAGE_PROXY_KEYS` | required | Signing keys, shared with the BFF |
| `APP_URL` | required | Its origin is the only CORS origin allowed |
| `IMAGE_CACHE_TTL_DAYS` | `30` | Whole days, from 1, no maximum |
| `IMAGE_CACHE_STORAGE` | `none` | `none` (batch only), `vercel-blob` or `s3` |
| `CRON_SECRET` | none | Required with any storage (the cron enforces the retention) |
| `BLOB_READ_WRITE_TOKEN` or `BLOB_STORE_ID` | none | Required with `vercel-blob` (read by `@vercel/blob`) |
| `IMAGE_CACHE_S3_*` | none | Required with `s3` (see "S3 storage") |
| `IMAGE_PRELOAD_SECRET` | none | Preload (relay); needs a storage (see "Preload") |
| `IMAGE_PRELOAD_FULL_EMAIL` | `false` | `true`: also the whole email from Stalwart (**privacy risk**); needs the `htmlExtractor` option |
| `IMAGE_PRELOAD_MAX_BYTES_PER_RECIPIENT`, `IMAGE_PRELOAD_MAX_IMAGES_PER_RECIPIENT`, `IMAGE_PRELOAD_INDEX_KEY` | see "Preload" | Same as on Node |

Blob storage: blobs are **private** (`access: "private"`), named `img/v1/<sha256(account)>/<sha256(url)>`, and expire `IMAGE_CACHE_TTL_DAYS` after upload (deleted on read and by the daily cron).

Cache mode with S3 on Vercel: `IMAGE_CACHE_STORAGE=s3`, the `IMAGE_CACHE_S3_*` variables and `CRON_SECRET` (the same `/cron/sweep` deletes old objects). No extra module.

Vercel answers at most 4.5 MB per response, so the adapter limits images to 3 MB per email (base64 adds a third) and per image. Bigger emails get `too-large` for the images that do not fit.

The client IP for rate limiting comes from `x-real-ip`, which Vercel sets.

## S3 storage

Any S3-compatible service: AWS S3, MinIO, Garage, Ceph RGW, R2's S3 API. No SDK: `fetch` plus our own AWS Signature V4 (`src/s3/sigv4.ts`, WebCrypto), so it runs on Node, Workers and Vercel alike. SigV4 is checked against AWS's published test suite and the four S3 examples of the S3 API reference, and was run against a real Garage v2.4.1 (put, get, list, delete; a wrong key gets 403).

| Variable | Default |  |
| --- | --- | --- |
| `IMAGE_CACHE_S3_BUCKET` | none | Turns S3 on. 3–63 characters `[a-z0-9.-]` |
| `IMAGE_CACHE_S3_ENDPOINT` | required | Origin only: `https://s3.eu-west-1.amazonaws.com`, `http://minio:9000`, `https://<account>.r2.cloudflarestorage.com` |
| `IMAGE_CACHE_S3_ACCESS_KEY_ID`, `IMAGE_CACHE_S3_SECRET_ACCESS_KEY` | required | A key that can only read, write, list and delete in this bucket |
| `IMAGE_CACHE_S3_SESSION_TOKEN` | none | Temporary (STS) credentials |
| `IMAGE_CACHE_S3_REGION` | `us-east-1` | Garage: `garage` (its `s3_region`); R2: `auto` |
| `IMAGE_CACHE_S3_URL_STYLE` | `path` | `path` (`<endpoint>/<bucket>/<key>`, MinIO and Garage) or `virtual` (`<bucket>.<host>`, AWS) |

- Objects are `img/v1/<sha256(account)>/<sha256(url)>`, never a URL. They expire `IMAGE_CACHE_TTL_DAYS` after their `Last-Modified`: deleted on read and by the sweep (hourly on Node and Workers, the daily cron on Vercel). Add a bucket lifecycle rule on `img/` with the same days as a second net.
- **The endpoint is trusted configuration** written by the admin, often a private address (`http://minio:9000` in Docker), so S3 requests do **not** go through net-guard; they never carry a URL from an email. What we enforce: only the configured endpoint, redirects refused, 10 s per request, size limits on what we read, and errors with the HTTP status only (S3 error bodies can echo keys). Use `https` unless the storage is on the same private network.
- The sweep deletes one object at a time (S3's batch `DeleteObjects` needs `Content-MD5`, which WebCrypto lacks).

## Preload (Stalwart MTA Hook)

`preload` mode: images are downloaded when the email arrives, so the sender learns only that the email arrived, never when (or if) it is opened, like Proton. It is optional and off unless you set `IMAGE_PRELOAD_SECRET`. Without it, `cache` mode still hides the user's IP; it only lets the sender see _when_ an email is opened.

Stalwart's MTA Hook always sends the **whole email** (headers, body and attachments; Stalwart has no option to send less). So someone has to read the email to find its images, and there are two ways:

|  | Relay (optional) | Full email to the proxy |
| --- | --- | --- |
| Who reads the email | The relay, on the mail server (next to Stalwart) | The proxy, wherever it runs (Vercel, a VPS…) |
| What the proxy sees | Image URLs of every incoming email, when it arrived, and keyed hashes of the recipients | **Every incoming email in full**: subject, sender, recipients, text, attachments |
| Proxy endpoint | `POST /hooks/preload-urls` (on with `IMAGE_PRELOAD_SECRET`) | `POST /hooks/stalwart` (only with `IMAGE_PRELOAD_FULL_EMAIL=true`) |
| Extra to run | The relay (Node), on the mail server | Nothing |
| HTML parser (parse5, in a worker thread) | In the relay; the proxy needs none | In the proxy |

> **Risks, read before turning preload on.**
>
> - **Full email** (`IMAGE_PRELOAD_FULL_EMAIL=true`): whoever runs the proxy, **the hosting provider included**, can read all the mail that reaches your server, also the emails nobody opens. Only use it when the proxy runs on a machine as trusted as the mail server itself. The proxy prints a warning at start.
> - **Relay**: the proxy still sees the image URLs of all incoming mail, also of emails nobody opens. URLs can say something: which newsletters and services you get mail from, and some carry an id or your address (trackers are removed first, which helps, but not every id is a tracker). Recipients reach the proxy only as keyed hashes.
> - **Relay**: the relay parses hostile HTML on the mail server. Run it as its own user, listening only on loopback, with no outgoing access except the proxy (see "Relay › Running it").
> - **Both**: preload downloads images for emails nobody opens, so it stores more than `cache` mode. Use the per-recipient quota below.

Proxy side (Node or Vercel; not Workers):

| Variable | Default |  |
| --- | --- | --- |
| `IMAGE_PRELOAD_SECRET` | none | Turns preload on (`POST /hooks/preload-urls`). 32+ random characters (`openssl rand -hex 32`); the relay's bearer token (and the hook's, with the full email) |
| `IMAGE_PRELOAD_FULL_EMAIL` | `false` | `true` also opens `POST /hooks/stalwart`, where Stalwart sends the whole email. **Read the risks above.** On Vercel it also needs the `htmlExtractor` option |
| `IMAGE_PRELOAD_MAX_MESSAGE_BYTES` | 25 MB | Full email only: bigger hook requests (the whole email in JSON) are accepted without preloading |
| `IMAGE_PRELOAD_MAX_BYTES_PER_RECIPIENT` | 256 MiB | Preloaded bytes per recipient address before preload stops for it |
| `IMAGE_PRELOAD_MAX_IMAGES_PER_RECIPIENT` | 10 000 | Preloaded images per recipient address before preload stops for it |
| `IMAGE_PRELOAD_INDEX_KEY` | the secret | 32+ characters. Key of the recipient hashes (the relay needs the same one). Set it **before** you ever rotate `IMAGE_PRELOAD_SECRET`: without it a new secret makes new hashes, so quotas and opt-outs start from zero |
| a cache | required | `IMAGE_CACHE_DIR` or `IMAGE_CACHE_S3_*` (Node); `IMAGE_CACHE_STORAGE` (Vercel) |

Vercel keeps the background work alive with its `waitUntil` (read from the request context, like `@vercel/functions`).

### Relay (optional)

```text
Stalwart ──MTA Hook (whole email, loopback)──▶ relay ──URLs + hashed recipients (https)──▶ proxy
```

The relay (`src/relay/relay.ts`, Node server `src/node/relay-server.ts`) receives the hook on the mail server, reads the email there with the same code as the proxy (`src/preload/parse.ts`: the same HTML parts, the same extractor and parser as the BFF, trackers dropped), normalizes and hashes the envelope recipients with the proxy's index key, and sends the proxy only:

```json
{
  "v": 1,
  "urls": ["https://images.example.com/a.png"],
  "recipients": ["<64 hex>"]
}
```

The proxy checks the URLs again (http/https only, no credentials, trackers, at most 60) and preloads them exactly as with the full email: same quota, opt-outs and email tokens. Nothing else of the email leaves the mail server. Stalwart always gets `accept` at once; the relay calls the proxy in the background (at most 4 at a time, 100 waiting). With email tokens it waits for the proxy's token (5 s at most) and, if the proxy does not answer, accepts anyway and only removes forged token headers. Logs carry counts only.

| Variable | Default |  |
| --- | --- | --- |
| `IMAGE_PRELOAD_RELAY_SECRET` | required | Bearer token Stalwart's MTA Hook sends to the relay. 32+ characters, **different** from the proxy's secret (the relay refuses to start otherwise), so Stalwart's configuration never holds a key the proxy accepts |
| `IMAGE_PRELOAD_UPSTREAM` | required | The proxy's base URL, `https://…` (`http://` only to loopback) |
| `IMAGE_PRELOAD_SECRET` | required | The proxy's `IMAGE_PRELOAD_SECRET` |
| `IMAGE_PRELOAD_INDEX_KEY` | the secret | The proxy's `IMAGE_PRELOAD_INDEX_KEY`, if it has one: the hashes must match, or quotas and opt-outs do not apply |
| `IMAGE_PRELOAD_EMAIL_TOKENS` | `false` | `true`: ask the proxy for an email token header (static build; the proxy needs it on too) |
| `IMAGE_PRELOAD_MAX_MESSAGE_BYTES` | 25 MB | Bigger emails are accepted without preloading |
| `IMAGE_PRELOAD_RELAY_HOST`, `IMAGE_PRELOAD_RELAY_PORT` | `127.0.0.1`, `3004` | Keep it on loopback |

#### Running it

```sh
cd packages/image-proxy
IMAGE_PRELOAD_RELAY_SECRET=… IMAGE_PRELOAD_SECRET=… IMAGE_PRELOAD_UPSTREAM=https://img.example.com \
  bun run start:relay
```

Ready-made files in `deploy/relay/`: an environment file (`relay.env.example`), a hardened systemd unit and an OpenRC service, and the Stalwart hook (`stalwart-mta-hook.ndjson`). They run it as its own user, read-only, without privileges. Also limit its outgoing traffic to the proxy (firewall by user, e.g. nftables `meta skuid`): it never needs anything else.

Stalwart side (WebUI: Settings › MTA › Filters › MTA Hooks, or `x:MtaHook/set`), pointing at the relay:

```json
{
  "url": "http://127.0.0.1:3004/hooks/stalwart",
  "stages": { "data": true },
  "enable": { "else": "is_empty(authenticated_as)" },
  "httpAuth": {
    "@type": "Bearer",
    "bearerToken": {
      "@type": "EnvironmentVariable",
      "variableName": "IMAGE_PRELOAD_RELAY_SECRET"
    }
  },
  "tempFailOnError": false,
  "timeout": 10000,
  "httpHeaders": {}
}
```

### Full email to the proxy (opt-in)

> **Every incoming email reaches the proxy.** See the risks above. Prefer the relay.

Set `IMAGE_PRELOAD_FULL_EMAIL=true` on the proxy. On Vercel also pass the extractor; `scripts/vercel-build.ts` ships its worker thread as `parse-worker.mjs` next to the function:

```ts
import { createWorkerExtractor } from "@nebulamail/mail-render/server-extract";

import { createVercelProxy } from "../src/vercel/index";

const proxy = createVercelProxy({
  htmlExtractor: createWorkerExtractor({
    workerUrl: new URL("parse-worker.mjs", import.meta.url),
  }),
});
export default { fetch: proxy.fetch };
```

The HTML is parsed in worker threads (`@nebulamail/mail-render/server-extract`), with the same parse5 as jsdom's `DOMParser` and the same extraction code as the BFF, so the URLs match the tickets. parse5 is quadratic on some inputs (`security/parser-vectors.json`), so each parse has a 2 s deadline and a memory cap; past either the worker is terminated and the email gets no preloaded images (logged as `preload-skipped: unparsable`). The same in the relay and the BFF.

Stalwart side, pointing at the proxy:

```json
{
  "url": "https://img.example.com/hooks/stalwart",
  "stages": { "data": true },
  "httpAuth": {
    "@type": "Bearer",
    "bearerToken": {
      "@type": "EnvironmentVariable",
      "variableName": "IMAGE_PRELOAD_SECRET"
    }
  },
  "tempFailOnError": false,
  "timeout": 5000,
  "httpHeaders": {}
}
```

### Both ways

(`bearerToken` can also be `{"@type": "Value", "secret": "…"}`; check the `SecretKey` variants of your Stalwart version.)

- `tempFailOnError: false` matters: if the relay or the proxy is down, Stalwart accepts the email anyway instead of answering 4xx. Neither ever rejects mail: once authenticated, every answer is `accept`.
- A short `timeout` is enough: nothing is downloaded before answering. With email tokens (below) the email's HTML is read first, up to 512 KB (more goes without a token); give it some margin (5 s is fine for the proxy; 10 s for the relay, which also waits for the proxy).
- The hook runs on every SMTP session, including emails your users send (their images get preloaded too). To limit it to incoming mail use `enable`, e.g. `{"else": "is_empty(authenticated_as)"}`.
- The hook URL should reach the relay or the proxy over loopback, a private network or https; the token is the only auth.
- Stalwart's hook client follows redirects and honours `HTTP_PROXY`/`HTTPS_PROXY` from Stalwart's environment. Give it the final URL (no redirect), and if Stalwart runs with an outbound proxy, add the relay's or the image proxy's host to its `NO_PROXY`, or the token goes through that proxy too.
- `timeout` and `allowInvalidCerts` come from Stalwart's shared `HttpClient` settings.

What happens with each email (in the relay, or in the proxy with the full email): Stalwart sends the top-level headers as pairs and the body as `contents` (checked live); the proxy finds every inline `text/html` part (base64 and quoted-printable decoded, charsets via `TextDecoder`; at most 100 parts, depth 8, 5 MB of HTML), extracts image URLs with mail-render's extraction (the same code and parser as the BFF, so the URLs match the tickets byte for byte; parsed in a worker thread with a 2 s deadline), drops trackers, keeps the first 60 and downloads them as one email (same limits, net-guard, tracker check on every redirect, global slots). Entries go under a shared key, `storageKey("*preload", sha256(url))`: at DATA time there is no JMAP account or email id yet, and `*` is never in a JMAP id. `POST /batch` and `GET /i/` look up the account's entry first, then this one, so the browser gets preloaded images without a download. Limits: `preload:global` rate limit (2/s, bursts of 120) before the body is read, at most 2 emails at once and 50 waiting (more are skipped and logged as `busy`).

### Quota per recipient (2026-10-27)

The hook sees envelope addresses, not JMAP accounts, so the limit is per **recipient address**:

- Each envelope recipient is normalized (lower case, `<>` and the `+tag` removed: `Alice+news@x` counts as `alice@x`) and hashed with HMAC-SHA256 under `IMAGE_PRELOAD_INDEX_KEY` (default: the secret). Only the hash is stored; logs carry counts only.
- The proxy keeps small JSON records next to the cache (`preload/state.ts`): `usage:<rcpt>` (the images charged to that recipient, with sizes and expiry), `refs:<urlHash>` (who uses a shared preloaded image) and `prefs:<rcpt>` (opt-out). Disk: `<IMAGE_CACHE_DIR>/preload-state/`; S3 and Blob: `pre/v1/` in the same bucket. They are **outside `img/`**: the sweep, the disk size cap and an `img/` lifecycle rule never delete them. Do not put a lifecycle rule on `pre/`.
- An image counts for every recipient of the email (also if it was already preloaded). When a recipient's live entries reach the bytes or images limit, its next emails are **not preloaded** for it; they behave like `cache` (fetched the first time they are shown). Entries leave the count when they expire (`IMAGE_CACHE_TTL_DAYS`). The check is made before downloading, so a recipient can go over by at most one email (10 MB).
- If nobody is left (all opted out or full), nothing is downloaded. No envelope recipient, or more than 100: not preloaded (fail closed). An unreadable record counts as full / opted out.
- The recipients are added to `refs:` **before** the image is looked up or downloaded, so another recipient's opt-out in between never deletes an image they are about to use. A missing `refs:` never means "nobody uses it": the image is kept until it expires.
- The sweep (hourly on Node, the cron on Vercel) also drops the expired entries of `usage:` and `refs:` and the records left empty; `prefs:` is never swept. A `refs:` pinned for more than 20 000 recipients expires with its last use.
- Read-modify-write is serialized per record inside one process. With several instances (Vercel) two writes can race: the quota is approximate, like the rate limits.
- Accepted risk: a sender can fill a recipient's quota (about 26 emails of 10 MB), so that recipient's next emails fall back to `cache`. The user is told (Settings › Privacy and the privacy bar of each email, with the date it frees up), and the log says `preload-skipped` `quota` when no recipient of an email had room. Limits per sender would not help (bulk senders share IPs and bounce domains). Raise `IMAGE_PRELOAD_MAX_BYTES_PER_RECIPIENT` or lower the retention if it matters.

### Opt-out (2026-10-27)

Each user can turn preload off in the app (Settings › General › Privacy, shown only when the proxy has preload). The proxy cannot read the user's preferences, so:

1. The browser calls the BFF (`POST /api/images/preload { optOut? }`, same-origin, session cookie).
2. The BFF reads the user's own addresses from JMAP with the user's token (login name and mail identities; Stalwart refuses identities with addresses that are not the account's, checked live) — never from the browser.
3. It calls `POST /preload/preferences` with `{ v: 1, addresses, optOut?, at, expiresAt }` and `Authorization: Nebula-Preload <keyVersion>.<HMAC>`: HMAC-SHA256 with the ticket keys (`IMAGE_PROXY_KEYS`) over a domain string and SHA-256 of the body, checked in constant time. The domain differs from the tickets', so a ticket MAC never works here. `expiresAt` at most 2 minutes away; `at` orders the choices, so a replayed older request cannot undo a newer one. Without `optOut` it only reads the state. Answers `{ optOut, full }`: `full` is `null`, or `{ until }` when an address that still preloads is over its quota, with the time (ms) when the last of them has room again (`null` if unknown, e.g. a read error); the app shows it in Settings and in the privacy bar. 401 for a bad signature, 400 for an expired or malformed body, 404 without preload (the app then hides the switch). Rate limit `preload-prefs:global`.
4. The proxy stores `prefs:<hash>` per address. The hook skips those recipients.

What happens to the images already preloaded: on opt-out the proxy deletes (in the background) every preloaded image that **only** that recipient used, and its usage record. Images that other recipients also got stay until they expire: deleting them would let any local user evict the copies others rely on (and the sender would then learn when those others open the email). Images the user opened in `cache` mode stay in the cache as usual; for nothing stored on the server at all, choose `batch` mode. An interrupted deletion is finished by the next opt-out request (the entries that could not be released stay in the usage record). With an email to an opted-out user and others, the images are preloaded for the others; the opted-out user can be served those shared copies (the sender learns nothing about them).

### Email tokens, for the static build (2026-10-29)

The static build of the app (docs/static.md) has no server of its own to sign tickets. With `IMAGE_PRELOAD_EMAIL_TOKENS=true` the hook gives each email it preloads a token instead:

1. At DATA the hook reads the email at once (not in the background) and, if it has images to preload, answers `accept` with two kinds of modifications: `deleteHeader` for every `X-Nebula-Preload` the email already had (anyone can write one) and `addHeader` `X-Nebula-Preload: 2.<keyVersion>.<issuedAt>.<id>.<HMAC>`. The id is random (64 hex), `issuedAt` is the proxy's time in Unix seconds; the MAC is HMAC-SHA256 with `IMAGE_PROXY_KEYS` over a domain string of its own, the key version, `issuedAt` and the id. As in milter, `deleteHeader`'s `index` is the occurrence of that header name, from 1. Stalwart puts the added header on top.
2. In the background, before downloading, the proxy stores the hashes of that email's images under the id (a small record in the image storage, `storageKey("*mail", id)`: it expires and is swept with the images). Never a URL or an address.
3. The app reads the header with JMAP and, when the user asks to see the images, uses `<proxy>/p/<token>/<sha256(url)>`. The proxy checks the MAC and that the hash is in the record, then serves the preloaded copy. A GET never downloads.

Anyone who has the email has its token, and it only gives that email's own images, already downloaded: no download, no other email's images, nothing about who else got them. The token doesn't expire; the record and the images do (`IMAGE_CACHE_TTL_DAYS`). Emails without images get no token, nor do emails with more than 512 KB of HTML (Stalwart is waiting: they are still preloaded, in the background, but stay blocked in option D) or whose recipients all opted out or are over quota.

Forged headers are removed only when the hook reads the email; when it doesn't (rate limit, body over `IMAGE_PRELOAD_MAX_MESSAGE_BYTES`, proxy down, timeout), a header copied from another email stays in the message. The app ignores it: it only uses a token whose `issuedAt` matches the email's `receivedAt` (at most 5 minutes after it, or up to an hour before it). Stalwart sets `receivedAt` when it delivers, and no sender controls it (checked in Stalwart 0.16.27's code: only mail the user imports takes it from `Received:`). Keep the proxy's clock right (NTP).

There is no per-user opt-out from the static build (it needs the BFF to sign it): preload stays server-wide.

Node and Vercel only (preload needs a spec-compliant HTML parser in a worker thread); not on Workers.

Live check (2026-10-31, throwaway Stalwart 1.0.1, Node relay and proxy, mail delivered over SMTP): `deploy/relay/stalwart-mta-hook.ndjson` is accepted as is; an email with an image and two forged `X-Nebula-Preload` headers arrived with only our token, and `GET /p/` served the image with it; with no image the forged header was removed; over 512 KB of HTML it arrived without a token; with the relay down it was delivered.

Live check (2026-10-29, Stalwart 0.16.27, Node proxy with disk cache): an email with an image, a tracker and two forged `X-Nebula-Preload` headers arrived with only ours (on top); the static app showed the image from `/p/…` and the browser never contacted the image's host.

Live check (2026-10-27, Stalwart 0.16 in Docker, Node proxy with disk cache): one email to alice and bob preloaded one image and wrote two usage records and one refs record, all hashes (no address or URL in any file); alice's opt-out kept the shared image; a new email only for alice was skipped (`opted-out`); bob's opt-out then deleted the image; a bad signature got 401. (Stalwart needed `x:Action` `ReloadSettings` after `x:MtaHook/set`.)

## Rate limiting

Keys: `batch:ip:<ip>`, `batch:account:<id>`, `batch:global`, `image:ip:<ip>`, `image:global`, `preload:global`, `preload-prefs:ip:<ip>`, `preload-prefs:global`. The core asks the injected `rateLimit(key)` (may be async); a limiter that throws allows, so a broken remote limiter cannot take the proxy down.

| Adapter | Where the counters live |
| --- | --- |
| Node | Memory of the process. Several replicas: each counts alone (≈ N × the limit) |
| Vercel | Memory of each function instance (same caveat); a Vercel Firewall rule for a hard limit |
| Workers | Cloudflare's Rate Limiting binding per rule (`[[ratelimits]]`, see `wrangler.example.toml`), shared by the isolates of a location; memory per isolate for rules without a binding or when a binding call fails |

No Redis (decided 2026-10-26).

## Tests

`bun run test`: tickets (tampering, rotation, expiry, canonical encoding), preload quota and opt-out (`preload/usage.test.ts`: full recipients, expiry, shared images kept, replay, forged and ticket MACs refused, only hashes stored), handler (both modes, budgets, concurrency, CORS, headers, trackers, a GET never fetching), disk, R2 and Blob storage, rate limiter, the Node server end to end against a local image server, the real dev server process, the Worker (`fetch(request, env)` with a fake env, a fake R2 binding and a fake network playing DoH and the image servers), the Vercel function (real pinned Node fetch to a local server, rewritten paths, cron auth, Edge refused), and, for **each of the three adapters**, every blocked URL vector of `security/ssrf-vectors.json` inside a correctly signed ticket (no request may reach the target). Test factories (`create*ProxyForTests`) are refused outside Vitest and not exported.

`bun run test:bun` and `bun run test:deno` run the whole suite inside Bun and Deno (net-guard's Bun adapter, and the Node adapter on Deno; the dev server test then starts `src/node/server.ts` with Bun, or with `scripts/start-deno.sh` and its minimal permissions). CI job `runtimes`.

The Workers tests do not use `@cloudflare/vitest-pool-workers`/Miniflare (a large dependency tree): they run the handler on Node with mocks, so they prove our checks, not Cloudflare's behaviour.

## TODO

- External adapters (imgproxy, go-camo). Their SSRF defaults were checked on 2026-10-29: imgproxy allows private IPs unless `IMGPROXY_ALLOW_PRIVATE_SOURCE_ADDRESSES=false`; neither limits ports or, by default, size.
- Deploy and check the Workers and Vercel adapters on the real platforms.
- https→http fallback after a connect _timeout_ (today only refused/reset/TLS errors fall back, to keep the per-image time bounded).
