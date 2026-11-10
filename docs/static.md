<!-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# The static version, served by Stalwart

Stalwart can serve a web app from a zip file, the same way it serves its own web interface. The static version of Nebula Mail is such a zip. You run no server of ours: only Stalwart.

```text
browser ──► Stalwart (serves the zip, sign-in, OAuth, JMAP)
```

This guide is for Stalwart administrators. It needs **Stalwart 1.1.0** (the only version supported).

## Static or server version?

The recommended setup is the **server version**: the app with a small backend in front of Stalwart. Choose the static version when you want nothing but Stalwart, and accept these differences:

|  | Server version | Static version |
| --- | --- | --- |
| Long-lived login (refresh token) | In an HttpOnly cookie on the server | In memory, in a worker of the browser tab (never written to storage) |
| Remote images | Through an image proxy: the sender never sees the user's IP address | Blocked, or loaded **straight from the sender** after a warning (the sender sees the IP address and the time). An image proxy is possible only with preloading (below). |
| One-click unsubscribe | Sent by the server | Off by default; when a user turns it on, the request leaves **from the user's browser** (the list sees the IP address) |
| Notifications with every tab closed | New mail and reminders with details | New mail shows sender and subject (Stalwart sends them in the push); event and task reminders only say "Event reminder" |
| Content-Security-Policy | HTTP header, with a nonce per request | `<meta>` tag fixed at build time (details below) |
| Settings | Environment variables, read at start | Baked into the zip when it is built |

What this means for security: a script injected into the page could use the open session while the tab is open, but it cannot copy the long-lived login out of the worker. Closing the last tab signs the user out. The rest of the app (mail, contacts, calendars, filters, settings sync, the "Verified sender" badge chosen by each user) is the same.

## 1. Get the zip

### From a release

Each release has `nebula-mail-static-<version>.zip`, a `SHA256SUMS` file, and a Sigstore signature (`*.sigstore.json`) for every file. Check them before you install (the release notes have the exact commands; `OWNER/REPO` is this repository):

```sh
cosign verify-blob SHA256SUMS \
  --bundle SHA256SUMS.sigstore.json \
  --certificate-identity "https://github.com/OWNER/REPO/.github/workflows/release.yml@refs/tags/v1.2.3" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
sha256sum --check --ignore-missing SHA256SUMS
```

The release zip uses the **default options**: mounted at `/nebulamail`, OAuth client `nebulamail`, all four scopes, no image proxy. If you need anything else, build your own zip.

### Build it yourself

You need [Bun](https://bun.sh), the version in `packageManager` of the root `package.json`. The build always runs on Bun (never Node), so the same sources give the same zip.

```sh
bun install --frozen-lockfile
cd apps/web
# The build also loads the server version's settings schema (.env.schema).
# These placeholders satisfy it; none of them ends up in the zip.
export APP_URL=https://mail.example.com JMAP_SERVER=https://mail.example.com \
  OAUTH_CLIENT_ID=unused OAUTH_CLIENT_SECRET=unused \
  SESSION_KEYS="1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
bun run build:static
# With options, for example: NEBULA_OAUTH_SERVICES="mail calendars" bun run build:static
```

The result is `apps/web/dist-static/nebula-mail-static.zip`, and its SHA-256 in `nebula-mail-static.zip.sha256`.

Build options are environment variables, read **when building** (see `apps/web/static.env.example`). To change one, build again.

| Variable | Default | Meaning |
| --- | --- | --- |
| `NEBULA_BASE_PATH` | `nebulamail` | Where Stalwart mounts the app (the `urlPrefix` of the `Application`). One path segment of letters, digits, `-`, `.`, `_` or `~`. Stalwart's own prefixes (`jmap`, `dav`, `.well-known`, `auth`, `api`, `login`, `logo`, `mail`, `calendar`, `admin`, `account`…) are refused by the build. |
| `NEBULA_OAUTH_CLIENT_ID` | `nebulamail` | The OAuth client you register in Stalwart. |
| `NEBULA_OAUTH_SERVICES` | all four | What the app asks for at sign-in: `mail` plus any of `contacts`, `calendars`, `files` (spaces or commas). Must match the client's `scopes`. An unknown name, or a list without `mail`, fails the build. |
| `NEBULA_IMAGE_PROXY_ORIGIN` | none | Your image proxy (https), for preloaded images. |
| `NEBULA_STYLE_NONCE` | random | Fixes the style nonce, to rebuild a zip byte for byte. Releases use `nebula-mail-<version with dashes>`, for example `nebula-mail-1-2-3`. |

The build fails if the code still calls anything that only the server version has. The zip contains the manifest Stalwart requires (`stalwart-app.json`: id `nebula-mail`, the version, `minServerVersion` `1.0.0`) and never an `oauth-client.json` (see the warning in step 3).

To check a release yourself: check out its tag, build with `NEBULA_STYLE_NONCE` as above and default options, and compare the SHA-256.

## 2. Choose the host name

Give the app **its own host name**, for example `nebulamail.example.com`, and keep Stalwart's web interface (`/admin`, `/account`) on the server's main name, for example `mail.example.com`.

Why: browsers keep each origin (scheme, host, port) apart. If the app and the admin panel share one, a flaw in one reaches the other. For example, a script injected into the panel could ask the app's session worker for a token.

Stalwart answers every path on every host name it serves. So point the new name at the same server (DNS and TLS certificate), then limit what that name serves with `Http.allowedEndpoints`. This rule answers 404 on the app host for everything the app doesn't use, the admin panel included:

```json
{
  "else": "200",
  "match": {
    "0": {
      "if": "contains(headers, 'host: nebulamail.example.com') && !(starts_with(path, '/nebulamail/') || path == '/nebulamail' || starts_with(path, '/jmap') || path == '/.well-known/jmap' || path == '/.well-known/oauth-authorization-server' || path == '/login' || path == '/api/auth' || path == '/api/auth/client' || path == '/auth/token' || path == '/auth/revoke' || path == '/logo' || starts_with(path, '/logo/') || starts_with(path, '/ui-'))",
      "then": "404"
    }
  }
}
```

- `/login`, `/api/auth`, `/api/auth/client`, `/logo` and `/ui-<hash>.css|js` are Stalwart's sign-in page and what it loads. `/auth/token` gives the app its tokens; `/auth/revoke` ends the session when the user signs out. Everything else, dynamic client registration (`/auth/register`) included, answers 404 on that host.
- `headers` is a list of `name: value` lines with the name in lower case. Add the port if it isn't the default (`host: nebulamail.example.com:8443`). Stalwart only speaks HTTP/1.1, so browsers always send `Host`.
- Stalwart **skips this rule for loopback addresses** (`[IP]`, `::1`), so test from another machine. **Behind a reverse proxy** on the same machine, every request comes from loopback and the rule does nothing until Stalwart sees the real address: set `Http.clientIpSource` to `"xForwardedFor"` or `"forwarded"` (whatever your proxy sends) and `Http.forwardedHops` (`1` for one proxy), and make Stalwart's HTTP listener reachable only through the proxy. Rate limits and automatic bans need the real address too.
- **Passkeys:** Stalwart 1.1.0 only accepts passkey sign-in on the origin of its public URL. On a separate app host, users sign in with their password (and TOTP code, if set).

The app talks to Stalwart only on its own host name, so **no CORS is needed**: leave `Http.usePermissiveCors` off.

A shared origin (`mail.example.com/nebulamail` next to `/admin`) also works, without that isolation.

## 3. Register the OAuth client

Stalwart refuses client ids it doesn't know. Create an `OAuthClient` with no secret (a public client; the app uses PKCE), the **exact** URL of the app's login page, and the scopes the zip asks for:

```json
{
  "clientId": "nebulamail",
  "description": "Nebula Mail",
  "redirectUris": { "https://nebulamail.example.com/nebulamail/login": true },
  "scopes": {
    "mail": true,
    "contacts": true,
    "calendars": true,
    "files": true
  },
  "grantTypes": { "authorizationCode": true, "refreshToken": true }
}
```

- `scopes`: `mail` is required. `contacts` (address books), `calendars` (calendar, tasks, invitations) and `files` (settings synced between devices; without it they are kept in a mail folder) are optional. They must be **exactly** what the zip asks for (`NEBULA_OAUTH_SERVICES`). If the app asks for a scope the client lacks, every sign-in fails with `invalid_scope`. What isn't granted is hidden in the app.
- Don't add `security`, `settings` or `full`: the app doesn't use them, and they would give a script injected in the page more power.
- `grantTypes` above is Stalwart's default. Writing it keeps the device code grant off if someone edits the client by hand.
- With these scopes the app's tokens reach mail, contacts, calendars and files, and nothing else: never the management API, even for an admin.
- After signing in, Stalwart asks each user to allow Nebula Mail, listing what it may use.

> **Never make Nebula Mail a first-party app.** Leave the `Application`'s `oauthClientId` and `oauthScopes` empty (step 4), and never put an `oauth-client.json` in the zip (the build refuses one). Stalwart gives first-party apps full access, management included for an admin, so a script injected in the page would hold every permission of the account.

What users may see from other apps:

- Any app can register itself (RFC 7591 dynamic registration, limited by `Http.rateLimitAnonymous`), but only as a public client that redirects to a loopback address or a private-use scheme (`com.example.app:/…`), never to a website. Its consent screen says **"Unverified application"**: tell users to deny those unless they just started the sign-in from an app of their own. The nebulamail, registered above, is never marked unverified.
- Leave `OidcProvider.clientMetadataDocuments` at `"disabled"` (the default): otherwise any https site could act as a client.

## 4. Install the zip

Publish the zip at an https URL you control, **after checking it** (step 1): Stalwart downloads it and does not check signatures. Then add an `Application`:

```json
{
  "description": "Nebula Mail",
  "resourceUrl": "https://files.example.com/nebula-mail-static-1.2.3.zip",
  "urlPrefix": { "/nebulamail": true },
  "unpackDirectory": "/var/lib/stalwart/apps"
}
```

- `urlPrefix` must be the zip's `NEBULA_BASE_PATH`.
- A `file:///…` URL also works, for a zip already on the server.
- Run the **`UpdateApps`** action to install now, and after every new release (`x:Action/set` with `{"create": {"u": {"@type": "UpdateApps"}}}`). It always downloads the URL again. Without it, Stalwart may keep serving its cached copy for up to `autoUpdateInterval` (90 days by default). Use a new URL for each version, so it's clear which one runs.
- If a download or unpack fails, Stalwart keeps serving the previous version and logs the error (for example a zip without `stalwart-app.json`, or one for a newer Stalwart).
- Stalwart serves `index.html` with `cache-control: no-cache` and every other file as immutable for a year. The build puts a hash in the name of every file that can change.

Other settings apply without a restart after the `ReloadSettings` action (`{"create": {"r": {"@type": "ReloadSettings"}}}`).

## 5. Response headers

Put in `Http.responseHeaders` what a `<meta>` CSP can't do. Stalwart adds these headers to **all** its responses (the admin panel too), and they **replace** any header of the same name that Stalwart sets. So only these:

```json
{
  "X-Frame-Options": "SAMEORIGIN",
  "X-Content-Type-Options": "nosniff",
  "Referrer-Policy": "no-referrer",
  "Cross-Origin-Opener-Policy": "same-origin",
  "Permissions-Policy": "camera=(), microphone=(), geolocation=()"
}
```

Also set `Http.enableHsts` to `true` once https works.

Don't add a `Content-Security-Policy` here: it would replace the stricter ones Stalwart sends itself, such as `frame-ancestors 'none'` on its sign-in page. `SAMEORIGIN` and not `DENY`, because an email whose images load from the sender is shown in a page of the zip framed inside the app.

## 6. Other recommended settings

- Use the admin account only for administration, never to read mail. If you can, offer the admin panel on a private listener (another port or a VPN), and don't keep `STALWART_RECOVERY_ADMIN` set in production.
- **Sharing:** set `Sharing.allowDirectoryQueries` to `true` so users can find people to share folders, calendars and address books with. Stalwart leaves it off; the share dialogs then show existing shares but can't add anyone. It lets every user list the server's accounts (names and addresses), so leave it off if they mustn't see each other.
- **Token lifetime:** `OidcProvider.accessTokenExpiry` of 15 minutes (`900000`; default one hour, minimum 10 minutes). The app refreshes tokens by itself, and a leaked token is useful for less time. `refreshTokenExpiry` (30 days by default) is how long an unused session lives.
- **Rate limits:** the defaults (`Http.rateLimitAnonymous` 100 and `rateLimitAuthenticated` 1000 requests a minute per address) suit the app. You can lower the anonymous one (sign-in page and dynamic registration), but not below what several users behind one address (an office) need to sign in.
- **Memory:** `SystemSettings.maxBufferedMemory` defaults to a quarter of the memory Stalwart detects; past it, Stalwart answers 503 for a moment. In a container, set a memory limit or this value.
- `Http.redirectRoot` is what `/` answers (`/account` by default). On the app host the rule in step 2 already answers 404 there.

## 7. Preloaded images (optional)

Without a proxy, users choose between "Always blocked" (the default) and "Ask, then load from the sender". With the image proxy of this repository and Stalwart's MTA Hook, images are downloaded **when the email arrives**, and users get a third choice, "Ask, then show the proxy's copy": the sender never learns the user's IP address or when the email was opened.

1. Run `packages/image-proxy` on Node or Vercel (not Cloudflare Workers, which can't preload), on its own host name, with storage, `IMAGE_PRELOAD_SECRET` and `IMAGE_PRELOAD_EMAIL_TOKENS=true`. Its `APP_URL` is the app's origin. See its README, sections "Preload" and "Email tokens, for the static build".
2. Add the MTA Hook in Stalwart as that README says, preferably through the **relay** on the mail server (also with `IMAGE_PRELOAD_EMAIL_TOKENS=true`), so the proxy only receives image URLs. Sending whole emails to the proxy (`IMAGE_PRELOAD_FULL_EMAIL=true`) works too, but then whoever runs the proxy can read all incoming mail. Either way set `tempFailOnError: false`, so mail still arrives if the relay or proxy is down.
3. Build the zip with `NEBULA_IMAGE_PROXY_ORIGIN=https://img.example.com`. The page's CSP then allows images from that origin, and nothing else from it.

The hook adds a signed `X-Nebula-Preload` header to each email it preloads (and removes any copy the email already had). The app uses it to fetch that email's copies, without any token of the user's, and ignores a header whose time doesn't match when the email arrived. Keep the proxy's clock right (NTP). Emails the proxy didn't preload stay blocked in that mode. Preloading is server-wide: users choose whether to see the copies, but can't opt out of the download (that needs the server version).

## How the page is protected

`index.html` carries a strict Content-Security-Policy in a `<meta>` tag:

- Scripts only by hash, with `'strict-dynamic'`: a file runs only if a hashed inline script loads it, never just because it is on the same origin (Stalwart also serves attachments there).
- Trusted Types are required, so the page can only load the zip's own scripts and workers.
- Connections, workers, fonts and frames only on the page's own origin. Images from that origin, `data:`, `blob:`, and the image proxy if the zip was built with one. No plugins (`object-src 'none'`); forms only to the same origin.
- `<style>` elements carry a nonce fixed at build time. Knowing it lets someone add CSS, never run code.

Two helper pages in the zip have their own policy:

- `email-frame.<hash>.html` shows an email whose images load from the sender: `https:` images, nothing that runs. The app's own page never loads remote images.
- `unsubscribe.<hash>.html` sends a one-click unsubscribe: only its own hashed script and style, `connect-src https:`, no forms. It sends nothing until the user clicks, since anyone can link to it. The POST goes out from a sandboxed frame with `credentials: "omit"` and no referrer, so the list gets no cookies and `Origin: null` (RFC 8058 §3.1). The browser can't read the answer, so the page says the request was sent, not that it worked. It is offered only for https lists whose `List-Unsubscribe-Post` header is signed by an aligned DKIM signature that your Stalwart verified.

Emails are shown in sandboxed iframes without scripts, with their own CSP. Attachments are downloaded, never opened as pages on the app's origin.

## Login details

- Sign-in uses Stalwart's own page, then returns to the app (OAuth code flow with PKCE).
- The refresh token lives only in memory, in a worker shared by the app's tabs. It is never written to browser storage. Closing the last tab signs the user out.
- In Chrome the worker outlives the last tab for a short while, so a reload keeps the session. In other browsers, reloading the only open tab may ask the user to sign in again. Where shared workers don't exist, each tab has its own session.
- Signing out revokes the session in Stalwart. Signing in again revokes the previous session only after the new one has its tokens.

## Test it

`bun run test:static` (in `apps/web`; the `static-stalwart` CI job) installs the zip in a throwaway Stalwart 1.1.0 as this guide says: the registered client with its scopes, the response headers above, the zip as an `Application` from a read-only mount, `ReloadSettings` and `UpdateApps`. Then, in Chromium, it checks the headers and the page's CSP, sign-in with PKCE and the consent screen, the inbox and one email, sign-out (the refresh token stops working), that revoking one login leaves others alone, and that an HTML attachment never opens as a page. Every request of the browser, workers included, goes through a small proxy: the test fails if anything goes to another origin or outside the paths of the `allowedEndpoints` rule.

```sh
cd apps/web
bun run build:static   # with the placeholders of step 1
export STALWART_STATIC_RECOVERY_ADMIN="admin:$(openssl rand -hex 16)"
docker compose -p nebula-static-test -f ../docker/compose.yaml --profile static up -d --wait --no-deps stalwart-static
bun run test:static
docker compose -p nebula-static-test -f ../docker/compose.yaml --profile static rm -sfv stalwart-static
```

The container (`stalwart-static` in `docker/compose.yaml`, port 8095) has its own compose project and never touches a development Stalwart. `scripts/ci-local.sh static-stalwart` runs all of this.
