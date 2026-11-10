<!-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# Full-proxy mode: all JMAP through the web server

This guide is for people who run the **server version** of Nebula Mail (the app with its own small backend). Read [getting-started.md](getting-started.md) first: it covers building, the OAuth client and the usual settings. The static zip ([static.md](static.md)) has no backend, so this mode doesn't exist there.

By default the browser gets a short-lived JMAP access token from the backend (`POST /auth/token`) and talks to Stalwart directly. With `JMAP_PROXY=on` the browser **never** receives a token: every JMAP request (API calls, uploads, downloads, push) goes to `/jmap-proxy/…` on the app's own origin with the session cookie, and the backend adds the token and forwards it.

```text
Default (JMAP_PROXY off)
  browser ──cookie──► web app   /auth/token  → short access token
  browser ──Bearer──► Stalwart  JMAP (needs CORS)

Full proxy (JMAP_PROXY=on)
  browser ──cookie──► web app /jmap-proxy/… ──Bearer──► Stalwart (can be internal)
  browser ──────────► Stalwart's sign-in page only
```

## When to use it

- **No token in the browser.** In the default mode a script injected into the page (an XSS) could read the access token (valid 15 minutes) and use it from anywhere until it expires. With the proxy it can only act from inside that open page; there is no credential to take away.
- **Stalwart off the Internet.** Stalwart's HTTP can live on an internal network (`http://stalwart:8080` in Docker); only its sign-in page needs to be public.

The cost: all mail traffic, attachments included, passes through the web app, and push needs a host that keeps responses open for minutes (see [Push](#push-and-hosting)).

## Turning it on

| Variable | Default | What it does |
| --- | --- | --- |
| `JMAP_PROXY` | empty = `off` | `on` or `off`. Any other value stops the server at start. |
| `JMAP_INTERNAL_URL` | empty = use `JMAP_SERVER` | Where the backend reaches Stalwart (or the bridge), e.g. `http://stalwart:8080`. Never sent to browsers. |
| `JMAP_SERVER` | required | Unchanged: Stalwart's **public** URL, exactly as its OAuth `issuer`. The browser is sent there to sign in. |

`JMAP_INTERNAL_URL` rules (a bad value stops the server at start):

- An origin only: no path, query, fragment or `user:password@`.
- Plain `http` only for hosts that can't be on the Internet: loopback, private IPs (10/8, 172.16/12, 192.168/16, 100.64/10, fc00::/7) and non-public names (single labels like `stalwart`, `localhost`, `*.internal`, `*.lan`, `*.local`, `*.home.arpa`…). Anything else needs `https`.
- Never a link-local address or a cloud metadata name (`169.254.169.254`, `metadata.google.internal`…), even with https.

`JMAP_INTERNAL_URL` is useful **without** the proxy too: then only the backend's own calls go to it (OAuth discovery, token refresh and revocation, image tickets, one-click unsubscribe) and the browser still talks JMAP to `JMAP_SERVER`. In both modes, any URL on `JMAP_SERVER`'s origin that the backend would call is moved to the internal origin; nothing else is.

Nothing changes in the OAuth client registration: the redirect URI stays `<APP_URL>/auth/callback`.

## What changes when it is on

- `/auth/token` answers `404 {"error":"jmap-proxy"}`. The app (and its service worker) read that answer and switch to the proxy.
- The page's CSP no longer lists Stalwart in `connect-src`: pages can only connect to their own origin (and the image proxy, if configured). Stalwart stays in `form-action`, for the redirect to its sign-in page.
- Signing in sets a **second cookie**, `__Host-wm-proxy` (`SameSite=Strict`), next to the session cookie `__Host-wm-session` (`SameSite=Lax`). On `localhost` with a port, both names get the port appended (`__Host-wm-proxy-3001`).
- **Sessions from before** the switch have no proxy cookie: those users are asked to sign in again, once.
- **JMAP over WebSocket** (RFC 8887) is removed from the session the browser sees; the app uses push over SSE instead.
- **Uploads and downloads** stream through the backend. Downloads are fetched by the page and saved from a `blob:` URL, as in the default mode.
- **Web Push** (notifications with the tab closed) keeps working: Stalwart sends to the browser's push service directly (so it needs outbound Internet access), and the service worker's JMAP calls go through the proxy.

## How it works

Routes, all on the app's origin:

| Route | What |
| --- | --- |
| `GET /jmap-proxy/grant` | What the page needs from the backend (image and badge settings). Never a token. |
| `GET /jmap-proxy/session` | Stalwart's JMAP session, with `apiUrl`, `uploadUrl`, `downloadUrl` and `eventSourceUrl` rewritten to the routes below. |
| `POST /jmap-proxy/api` | One JMAP API request. |
| `POST /jmap-proxy/upload/{accountId}` | Upload, streamed. |
| `GET /jmap-proxy/download/{accountId}/{blobId}/{name}?type=` | Download, streamed. Always `Content-Disposition: attachment`. |
| `GET /jmap-proxy/eventsource?types=&closeafter=&ping=` | Push (Server-Sent Events), streamed. |

A wrong method answers 405, an unknown path 404, a malformed parameter 400. With `JMAP_PROXY` off every `/jmap-proxy/` path answers 404.

For each request the backend takes the access token from the session cookie (refreshing it when needed, exactly like `/auth/token` does, and returning the updated cookie) and sends `Authorization: Bearer` to Stalwart. If Stalwart answers 401 it refreshes once and retries; for uploads it doesn't retry itself (the body is already consumed) and the app sends the file again.

### Limits and timeouts

|  |  |
| --- | --- |
| API request body | Stalwart's `maxSizeRequest`, never more than 50 MB (10 MB until the backend has read the session once). Larger: 413. |
| Upload | Stalwart's `maxSizeUpload` (50 MB if it doesn't say). Larger: 413. |
| Open streams (push, uploads, downloads) | 12 per session, 1024 per backend process. Beyond that: 429 with `Retry-After: 5`. |
| Timeouts | session 15 s, API 60 s, upload 10 min, download 30 s until the headers (then the body streams as long as it takes), push 5 min (see below). On timeout: 504. |

Each tab sends a random `X-Nebula-Client` id; a new push stream from the same tab (same id and types) ends its previous one, so reconnects never pile up waiting for a free slot. A stream replaced this way before it opened answers 409.

Token refreshes of one session are shared between parallel requests, and Stalwart's endpoints are cached for 10 minutes, but both only **within one process**. With several app replicas, use sticky sessions so requests of one user hit the same process.

### Security model

**What reaches Stalwart (SSRF).** The proxy only talks to the configured server (`JMAP_INTERNAL_URL`, else `JMAP_SERVER`):

- The URLs in Stalwart's session are accepted only on its public or internal origin, and templates must spell that origin out (no variable in the host). A session pointing anywhere else gives 502.
- From the browser only these values get into a URL, each checked and then re-encoded into Stalwart's own URL template: account and blob ids (JMAP `Id` syntax, RFC 8620 §1.2), a file name (no control characters, never `.` or `..`, at most 255 characters), a media type, and the push parameters (`types`, `closeafter` = `no` or `state`, `ping` up to 4 digits). An encoded `/` (`%2F`), `../` or CRLF never reach Stalwart as such.
- Redirects are never followed, except while discovering the session (`/.well-known/jmap` → the session URL), and then only within the same server.
- Request headers forwarded: `accept`, `accept-language`, `content-type`, `last-event-id`; the backend adds `Authorization` and `Accept-Encoding: identity`. Response headers forwarded: `content-type`, `content-length` (only when the body wasn't compressed) and `retry-after`. Cookies never cross in either direction, nor `Location` or CORS headers.
- Every proxy answer carries `Cache-Control: no-store`, `Content-Security-Policy: sandbox; default-src 'none'` and `X-Content-Type-Options: nosniff`, so nothing from Stalwart is ever rendered as a page on the app's origin. Stalwart's own error texts are replaced by fixed codes (`{"error":"unavailable"}`, `"timeout"`, `"too-large"`…).

**CSRF.** The session cookie now authorizes JMAP, so every proxy request, GETs included, needs **all** of:

1. The header `X-Nebula-Proxy: 1` (pages) or `X-Nebula-Proxy: background` (service worker; it doesn't count as use of the app for the idle time-out). A page on another site can't send a custom header without a CORS preflight, and nothing here answers one.
2. `Sec-Fetch-Site: same-origin`; only in browsers that don't send it, an `Origin` equal to `APP_URL`.
3. Not a navigation: `Sec-Fetch-Mode: navigate` is refused. A link, a form or a new tab to `/jmap-proxy/download/…` gets 403.
4. The `__Host-wm-proxy` cookie, matching the one stored in the session. Being `SameSite=Strict`, other sites can't make the browser send it. The session cookie stays `Lax`, so a link to the app from another site still opens it signed in.

Requests that fail 1–3 get 403. A missing or wrong proxy cookie gets 401 and the session cookies are cleared.

**Browsers:** Safari before 16.4 sends neither `Sec-Fetch-Site` nor `Origin` on same-origin GETs, so it can't use the proxy (403: it fails safe). Current browsers work.

### Signing out

Sign-out clears both cookies and revokes the refresh token at Stalwart. The next proxy request answers 401. An open push stream lasts at most 5 minutes before the proxy ends it, and its reconnection then fails.

## Push and hosting

Push is a long-lived response from the backend. The proxy ends each stream after **5 minutes**; the app reconnects at once and resumes with `Last-Event-ID`. If the host cuts responses earlier, the app reconnects the same way (backing off up to 60 s after repeated failures).

- **Reverse proxy in front of the app:** it must not buffer `/jmap-proxy/eventsource`. The proxy sends `X-Accel-Buffering: no`, which nginx honours; other proxies need their own setting. The app asks for a ping every 30 s, so a read timeout of 60 s or more is enough. Also allow request bodies as large as Stalwart's `maxSizeUpload` on `/jmap-proxy/upload/` (nginx: `client_max_body_size`).
- **Serverless hosts with short response limits** (for example Vercel functions): push still works but reconnects often, each reconnection costing an invocation.
- **Hosts that buffer whole responses:** push never delivers. While no push connection is delivering, the app checks for changes every 60 s instead, so mail still arrives, just later.

## Deploying with Stalwart on an internal network

Example with Docker Compose: Stalwart's HTTP listener only on the internal network, the web app on both.

```yaml
services:
  stalwart:
    image: docker.io/stalwartlabs/stalwart:v1.1.0
    environment:
      STALWART_PUBLIC_URL: https://mail.example.com
    ports:
      - "25:25" # SMTP from the Internet (MX): always public
      # Only if users also have other mail clients:
      # - "993:993"   # IMAPS
      # - "465:465"   # SMTP submission
      # No 8080/443 here: HTTP is internal.
    networks: [internal]

  nebulamail:
    # your build of apps/web (see getting-started.md)
    environment:
      APP_URL: https://nebulamail.example.com
      JMAP_SERVER: https://mail.example.com
      JMAP_INTERNAL_URL: http://stalwart:8080
      JMAP_PROXY: "on"
      # OAUTH_CLIENT_ID, OAUTH_CLIENT_SECRET, SESSION_KEYS… as usual
    networks: [internal, public]

networks:
  internal:
    internal: true
  public: {}
```

Then publish **only Stalwart's sign-in page** on its public name (`mail.example.com`) through a reverse proxy. The browser needs exactly these paths:

| Path | Why |
| --- | --- |
| `GET /login` | The sign-in and consent page (the OAuth authorization endpoint) |
| `POST /api/auth` | Where that page sends the user name, password (or passkey) and consent |
| `GET /api/auth/client` | Which app is signing in (shown on the consent screen) |
| `GET /logo`, `GET /logo/…` | The logo on the page |
| `GET /ui-<hash>.css`, `GET /ui-<hash>.js` | The page's style and script. The hash changes with every Stalwart version, so match it with a pattern: without these nobody can sign in |

The browser test (`bun run test:proxy`, below) fails if the browser asks Stalwart for anything else.

Everything else of Stalwart's HTTP stays internal: `/jmap/…`, `/.well-known/jmap`, `/auth/token`, `/auth/revoke`, `/.well-known/oauth-authorization-server`, `/.well-known/openid-configuration`, `/admin`, `/account`, the management API under `/api/`, DAV. The backend reaches what it needs through `JMAP_INTERNAL_URL`, not through this reverse proxy.

nginx:

```nginx
server {
    listen 443 ssl;
    server_name mail.example.com;
    # ssl_certificate …

    proxy_set_header Host $host;
    proxy_set_header X-Forwarded-For $remote_addr;

    location = /login           { proxy_pass http://stalwart:8080; }
    location = /api/auth        { proxy_pass http://stalwart:8080; }
    location = /api/auth/client { proxy_pass http://stalwart:8080; }
    location = /logo            { proxy_pass http://stalwart:8080; }
    location ^~ /logo/          { proxy_pass http://stalwart:8080; }
    location ~ ^/ui-[0-9a-f]+\.(css|js)$ { proxy_pass http://stalwart:8080; }
    location /                  { return 404; }
}
```

Caddy:

```caddyfile
mail.example.com {
    @login path /login /api/auth /api/auth/client /logo /logo/*
    handle @login {
        reverse_proxy stalwart:8080
    }
    @ui path_regexp ^/ui-[0-9a-f]+\.(css|js)$
    handle @ui {
        reverse_proxy stalwart:8080
    }
    respond 404
}
```

Notes:

- Tell Stalwart the real client address (its rate limits and automatic ban use it): `Http.clientIpSource` = `"xForwardedFor"` and `Http.forwardedHops` = `1`. The listener must then be reachable only through the reverse proxy, or anyone could claim any address.
- No CORS is needed in this mode (leave `Http.usePermissiveCors` off): the browser never calls Stalwart's JMAP.
- Ports: 25 stays public. 993/465/587, 4190 (ManageSieve) and the DAV paths (`/dav/…`, `/.well-known/caldav`, `/.well-known/carddav`) only if other clients use them.

### Without a reverse proxy

Stalwart can restrict what it serves on the public name itself with `Http.allowedEndpoints` (same syntax and caveats as in [static.md](static.md#use-a-subdomain)):

```json
{
  "else": "200",
  "match": {
    "0": {
      "if": "contains(headers, 'host: mail.example.com') && !(path == '/login' || path == '/api/auth' || path == '/api/auth/client' || path == '/logo' || starts_with(path, '/logo/') || matches('^/ui-[0-9a-f]+\\.(css|js)$', path))",
      "then": "404"
    }
  }
}
```

Stalwart skips these rules for loopback addresses, and behind a reverse proxy on the same machine every request comes from loopback until `Http.clientIpSource` is set.

## With the bridge

The [bridge](bridge.md) works as the upstream the same way: `JMAP_SERVER` is the bridge's `server.public_url` (its own OAuth issuer) and `JMAP_INTERNAL_URL` is where the backend reaches it. Its session URLs are absolute on `public_url`; the proxy moves them to the internal origin like Stalwart's. Its `503` with `Retry-After` reaches the browser and the app waits and retries. Its CORS setting doesn't matter in this mode.

On the bridge's public name, the browser only needs its sign-in page, `GET` and `POST /oauth/authorize`, and the page's font under `/assets/`.

## Keeping Stalwart small without the proxy

If you keep the default mode (short tokens in the browser), you can still hide most of Stalwart. On its public name the browser then needs the sign-in paths above, plus `GET /.well-known/jmap` (which redirects to `/jmap/session`) and everything under `/jmap/` (session, API, upload, download, event source), with CORS for the app's origin (`Http.usePermissiveCors`). With `JMAP_INTERNAL_URL` set, the OAuth endpoints (`/auth/token`, `/auth/revoke`, discovery) can stay internal; without it the backend must reach them on the public name.

```nginx
location = /.well-known/jmap { proxy_pass http://stalwart:8080; }
location ^~ /jmap/ {
    proxy_pass http://stalwart:8080;
    proxy_buffering off;          # push
    proxy_read_timeout 1h;
    client_max_body_size 50m;     # at least Stalwart's maxSizeUpload
}
```

## Tests

- Unit tests: `apps/web/src/server/jmap-proxy/*.test.ts` (settings, routes and path encoding, header allowlists, hostile sessions, CSRF, redirects, cookies never forwarded, 401 and refresh, stream limits, sign-out), `apps/web/src/server/auth/handlers.test.ts` (proxy cookie, `/auth/token` 404) and `packages/jmap-client/src/cookie-auth.test.ts`.
- In a browser against the development Stalwart (`docker/compose.yaml`, after `bun run dev:setup`), from `apps/web`:

  ```sh
  MAIL_LIVE_URL=http://localhost:8080 MAIL_LIVE_ALICE_PASSWORD=… bun run test:proxy
  ```

  The app runs with `JMAP_PROXY=on` and `JMAP_INTERNAL_URL` pointing at Stalwart under another name (`http://127.0.0.1:8080`), so the URL rewriting is tested too. It signs in, opens the inbox, sends an email with an attachment, waits for it by push, downloads the attachment, sees an email delivered behind the app's back arrive by push, and signs out. On every request of the page and its service worker it checks: no `Authorization` header, nothing sent to Stalwart but the sign-in paths, no access or refresh token in any answer, `/auth/token` 404, and a `connect-src` without Stalwart.
