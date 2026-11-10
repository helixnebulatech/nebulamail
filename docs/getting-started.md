<!-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# Getting started: self-hosting the server version

This guide sets up the **server version** of Nebula Mail, the recommended one: the web app plus a small backend of its own. The backend handles OAuth sign-in (the session lives in an encrypted `HttpOnly` cookie), signs the tickets for the image proxy and does one-click unsubscribe. It has no database.

Nebula Mail works with [Stalwart](https://stalw.art). Only the latest Stalwart release is supported (1.1.0 at the time of writing). For other mail servers, see the bridge below.

## Choose your setup

| Setup | Use it when | Guide |
| --- | --- | --- |
| Server version | You can run a Bun process next to Stalwart. Recommended. | This page |
| Server version with `JMAP_PROXY=on` | You want Stalwart's JMAP off the Internet and no access token in the browser. | This page, then [jmap-proxy.md](jmap-proxy.md) |
| Static zip served by Stalwart | You want nothing to run except Stalwart. | [static.md](static.md) |
| Bridge | Your mail server is not Stalwart (IMAP/SMTP, CalDAV/CardDAV, ManageSieve). | [bridge.md](bridge.md), then this page |

There is no published release of the server version and no Docker image yet: you build it from source.

## 1. Requirements

- [Bun](https://bun.sh), the version in `packageManager` of the root `package.json` (1.4.3 now). Bun is the only supported package manager.
- Git, to get the source.
- A Stalwart server (latest release) with an admin account, reachable over HTTPS on its public URL (for example `https://mail.example.com`).
- A host name for the app with HTTPS (for example `https://nebulamail.example.com`). The session cookies are `__Host-` cookies, which browsers only accept over HTTPS (or on `http://localhost`).
- Optional: a second host name for the image proxy (for example `https://img.example.com`).

## 2. Build

```sh
git clone https://github.com/helixnebulatech/nebulamail.git
cd nebulamail
bun install --frozen-lockfile
```

The build checks that the configuration (section 4) is present, but does not keep it: every variable is read again when the server starts, so one build runs anywhere. Set the variables before building, either the real ones or placeholders like the CI uses:

```sh
cd apps/web
APP_URL=https://nebulamail.example.com \
JMAP_SERVER=https://mail.example.com \
OAUTH_CLIENT_ID=placeholder \
OAUTH_CLIENT_SECRET=placeholder-not-a-secret \
SESSION_KEYS=1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA \
bun run build
```

The output goes to `apps/web/dist/` (`dist/client/` for the browser files, `dist/server/server.js` for the server).

## 3. Configure Stalwart

Every setting below is in Stalwart's web interface; the names are those of its configuration objects. Settings apply without a restart after the `ReloadSettings` action (Management › Actions). For a development server, `bun run dev:setup` does the same automatically ([docker/README.md](../docker/README.md)).

### Public URL

`JMAP_SERVER` (section 4) must be exactly Stalwart's OAuth issuer, as Stalwart publishes it in `/.well-known/oauth-authorization-server`: its public URL, without a trailing slash. Check it:

```sh
curl -s https://mail.example.com/.well-known/oauth-authorization-server | grep -o '"issuer":"[^"]*"'
```

### Register the OAuth client

Create an `OAuthClient` for the app. It is a **confidential** client (it has a secret, kept by the app's server):

```json
{
  "clientId": "nebulamail",
  "description": "Nebula Mail",
  "redirectUris": { "https://nebulamail.example.com/auth/callback": true },
  "scopes": {
    "mail": true,
    "contacts": true,
    "calendars": true,
    "files": true
  },
  "grantTypes": { "authorizationCode": true, "refreshToken": true },
  "secret": "<the client secret>"
}
```

- **Redirect URI**: exactly `<APP_URL>/auth/callback`.
- **Scopes**: the app asks for `urn:ietf:params:oauth:scope:mail`, `…:contacts`, `…:calendars` and `…:files`. A registered client may only ask for its own scopes, otherwise Stalwart refuses every login with `invalid_scope`. To offer fewer services, remove scopes here **and** set the same list in `OAUTH_SERVICES` (section 4). `mail` is required; without `contacts` or `calendars` the app hides those parts; without `files` the settings synced between devices are kept in a mail folder instead.
- **Secret**: generate one with `openssl rand -base64 32` and put the same value in `OAUTH_CLIENT_SECRET`. Stalwart stores only a hash of it.
- **Never make it first party.** Don't put this client id in an `Application`'s `oauthClientId`: Stalwart treats such a client as first party, and its tokens carry every permission of the account (management included, for an admin), with no consent screen. Stalwart computes `firstParty` itself; setting it does nothing.

After signing in, Stalwart asks each user to allow Nebula Mail. Users see and revoke it under "Connected Applications" in their account.

### Other settings

| Setting | Value | Why |
| --- | --- | --- |
| `Http.usePermissiveCors` | `true` | Needed in the default mode: the browser calls Stalwart's JMAP from the app's origin. The app sends only Bearer tokens, never cookies, to Stalwart. Leave it off with `JMAP_PROXY=on`. |
| `OidcProvider.accessTokenExpiry` | `900000` (15 minutes) | Recommended. The app refreshes tokens by itself, and a leaked token is good for less time. Stalwart's default is one hour. |
| `Sharing.allowDirectoryQueries` | `true` | Optional. Without it users can't look anyone up in the share dialogs (folders, calendars, address books). It lets every user list the server's accounts (names and addresses): leave it off if they mustn't see each other. |
| `MtaStageData.addAuthResultsHeader` | `{"else": "true", "match": {}}` | Only if you turn on the "Verified sender" badge (`VERIFIED_SENDER_BADGE`): Stalwart then adds its own `Authentication-Results` header on every path, not only on port 25, so local users can't fake the badge. Read [verified-sender.md](verified-sender.md) first. |

## 4. Environment variables

The server reads its configuration from environment variables when it starts. They are described and validated by `apps/web/.env.schema`; a missing required variable or a contradiction stops the server. Set them in the process environment (for example in your service manager), or in `apps/web/.env.local`, which git ignores.

### Required

| Variable | Meaning |
| --- | --- |
| `APP_URL` | Public URL of the app, without a trailing slash, e.g. `https://nebulamail.example.com`. OAuth redirects go to `<APP_URL>/auth/callback`. |
| `JMAP_SERVER` | Stalwart's public URL, written exactly as its OAuth `issuer` (no trailing slash): `https://mail.example.com/` fails. With the bridge, the bridge's public URL. |
| `OAUTH_CLIENT_ID` | The `clientId` registered in Stalwart. |
| `OAUTH_CLIENT_SECRET` | That client's secret. |
| `SESSION_KEYS` | AES-256-GCM keys for the session cookie, newest first: `2:<key>,1:<older key>`. Each key is 32 random bytes in base64url. The first one encrypts; all of them decrypt. |

Generate a session key:

```sh
echo "1:$(openssl rand -base64 32 | tr '+/' '-_' | tr -d '=')"
```

To rotate it, put a new key with a higher number first and keep the old one after it until the old sessions have expired: nobody is signed out.

### Optional

| Variable | Default | Meaning |
| --- | --- | --- |
| `OAUTH_SERVICES` | all four | Services asked for at login: any of `mail contacts calendars files` (spaces or commas), `mail` required. Must match the client's `scopes` in Stalwart. An unknown name stops the server. |
| `JMAP_PROXY` | `off` | `on`: all JMAP goes through this server and the browser never gets an access token. See [jmap-proxy.md](jmap-proxy.md). |
| `JMAP_INTERNAL_URL` | `JMAP_SERVER` | Where this server reaches Stalwart when that isn't its public URL, e.g. `http://stalwart:8080` in Docker. An origin only; plain `http` only for loopback, private addresses and internal names. Never sent to browsers. See [jmap-proxy.md](jmap-proxy.md). |
| `NODE_ENV` | `development` | `development`, `production` or `test`. |

### Remote images

| Variable | Default | Meaning |
| --- | --- | --- |
| `IMAGE_PROXY_ORIGIN` | none | URL of the image proxy (section 6). Must be a different origin from `APP_URL`. |
| `IMAGE_PROXY_KEYS` | none | HMAC keys for image tickets, shared with the proxy, same format as `SESSION_KEYS`. Use different keys from the session ones. |
| `REMOTE_IMAGES_DEFAULT` | `ask` | When remote images load for users who haven't chosen: `never`, `ask`, `trusted-senders` or `always`. |
| `REMOTE_IMAGES_VIA_DEFAULT` | the proxy if there is one, else `direct` | How they load: `proxy` or `direct` (straight from the sender, who then sees the user's IP address). |
| `REMOTE_IMAGES_ALLOW_DIRECT` | allowed only without a proxy, or with `REMOTE_IMAGES_VIA_DEFAULT=direct` | `true`/`false`: whether users may load images straight from the sender. |
| `REMOTE_IMAGES_FORCE_NEVER` | `false` | `true`: remote images never load for anyone. |

Without both `IMAGE_PROXY_ORIGIN` and `IMAGE_PROXY_KEYS` there is no proxy. Each user can change these in Settings › General › Privacy, within what you allow. Values that contradict each other (for example `REMOTE_IMAGES_VIA_DEFAULT=direct` with `REMOTE_IMAGES_ALLOW_DIRECT=false`) stop the server at start.

### "Verified sender" badge

| Variable | Default | Meaning |
| --- | --- | --- |
| `VERIFIED_SENDER_BADGE` | empty: each user decides (off by default) | `true` or `false` forces it for everyone. **Read [verified-sender.md](verified-sender.md) first**: the badge can be wrong unless Stalwart adds its own `Authentication-Results` header to all mail (section 3). Without the badge, the app still warns when the check fails. |

## 5. Run it

From `apps/web`, with the variables set, start the built app with Vite's preview server (what the project's own browser tests run against the production build):

```sh
cd apps/web
bunx vite preview --host 127.0.0.1 --port 3000 --strictPort
```

`bun run serve` is the same command with Vite's defaults. It needs the checkout with its dependencies installed (`bun install`), not only `dist/`. Run it under a service manager (systemd, OpenRC…) with `apps/web` as the working directory, and put a reverse proxy with HTTPS in front.

### Reverse proxy and HTTPS

- Terminate HTTPS at the proxy and forward everything on the app's host name to the app. The app takes its own address from `APP_URL`, not from the request, so `APP_URL` must be the public `https://` URL.
- With an `https://` `APP_URL`, the app sends `Strict-Transport-Security` (two years, `includeSubDomains`, `preload`) and its other security headers itself. Make sure every subdomain of that name can serve HTTPS before you deploy.
- Serve the app on its own host name, not on the same one as Stalwart's admin panel: browsers separate what sites store by origin.
- With `JMAP_PROXY=on`, push is a long-lived response on `/jmap-proxy/eventsource`: don't buffer it (nginx: the app already sends `X-Accel-Buffering: no`). Details in [jmap-proxy.md](jmap-proxy.md).
- If Stalwart itself is behind nginx, its push stream `/jmap/eventsource/` needs `proxy_buffering off`.

nginx:

```nginx
server {
    listen 443 ssl;
    server_name nebulamail.example.com;
    # ssl_certificate …

    location / {
        proxy_pass http://127.0.0.1:3000;
        proxy_set_header Host $host;
    }
}
```

Caddy:

```caddyfile
nebulamail.example.com {
    reverse_proxy 127.0.0.1:3000
}
```

Open `https://nebulamail.example.com` and sign in: the button takes you to Stalwart's sign-in page, then back to the inbox.

## 6. Image proxy (optional)

Without an image proxy, remote images in emails either load straight from the sender (who sees the user's IP address and when the email was opened) or never load. The proxy, `packages/image-proxy`, downloads them for the user on its own origin, only for URLs in tickets signed by the app's server.

1. Give it its own host name (`https://img.example.com`), not just another port of the app's: cookies ignore ports.
2. Generate a key (same command as for `SESSION_KEYS`) and set it as `IMAGE_PROXY_KEYS` in **both** the app and the proxy.
3. Proxy: `APP_URL` = the app's URL (its only allowed CORS origin). App: `IMAGE_PROXY_ORIGIN=https://img.example.com`.
4. Start it in `packages/image-proxy` with `bun run start:bun` (Bun), `bun run start` (Node 26) or `bun run start:deno` (Deno 2). It listens on `127.0.0.1:3003` by default (`IMAGE_PROXY_HOST`, `IMAGE_PROXY_PORT`). Behind a reverse proxy, set `IMAGE_PROXY_CLIENT_IP` so its per-IP rate limits see the real address.

Every option (cache on disk or S3, preloading images when mail arrives through a Stalwart hook, Cloudflare Workers, Vercel) is in [packages/image-proxy/README.md](../packages/image-proxy/README.md).

## 7. Notifications (Web Push)

Nothing to configure in the app: there are no VAPID keys of its own. Notifications with every tab closed use the JMAP server's Web Push support (the `urn:ietf:params:jmap:webpush-vapid` capability): the browser subscribes, and Stalwart pushes straight to the browser's push service. Stalwart must be able to reach those services (outgoing HTTPS). With the bridge, see "Notifications with every tab closed" in [bridge.md](bridge.md).

## 8. Updating

```sh
git pull
bun install --frozen-lockfile
cd apps/web && bun run build
```

Then restart the server. Sessions survive as long as `SESSION_KEYS` keeps the key they were encrypted with. Only the latest Stalwart release is supported, so update Stalwart when a new one comes out, and check this guide for new settings.

## Troubleshooting

- **`invalid_scope` at login**: the client's `scopes` in Stalwart and `OAUTH_SERVICES` don't match.
- **The server stops at start**: a required variable is missing, or a value is invalid (`JMAP_PROXY`, `JMAP_INTERNAL_URL`, `OAUTH_SERVICES`, contradicting `REMOTE_IMAGES_*`). Read the error in its output.
- **Sign-in fails after Stalwart's page**: check that `JMAP_SERVER` is exactly the `issuer` (no trailing slash) and that the redirect URI is exactly `<APP_URL>/auth/callback`.
- **The app can't load mail after signing in (default mode)**: CORS is off in Stalwart (`Http.usePermissiveCors`).
