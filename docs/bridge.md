<!-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# The bridge: Nebula Mail for IMAP and SMTP servers

Nebula Mail speaks JMAP. [Stalwart](https://stalw.art) speaks JMAP natively, so **with Stalwart you don't need the bridge**. For any other mail server with IMAP and SMTP (Dovecot and Postfix, a hosting provider, Gmail or Fastmail with an app password…), the bridge sits in between and offers JMAP to the web app (and to any other JMAP client):

```text
browser ──► web app ──JMAP + OAuth──► bridge ──IMAP / SMTP──────────► mail server
                                             ├─CalDAV / CardDAV───► calendar server (optional)
                                             └─ManageSieve────────► filters (optional)
```

What works through the bridge:

- **Mail** (always): reading, search, folders, flags and labels, threads, drafts, sending, attachments, live updates, undo send and scheduled send.
- **Calendars, contacts and tasks** with a CalDAV/CardDAV server (`[upstream.dav]`): events, invitations, reminders, sharing where the server allows it.
- **Filters and vacation response** with a ManageSieve server (`[upstream.sieve]`).
- **Notifications with every tab closed** (Web Push, `[push]`).

The web app hides whatever the bridge doesn't offer for your setup.

**One bridge serves one mail server.** Users sign in on the bridge's own sign-in page with their usual mail login and password; the bridge checks them with an IMAP login and uses the same login and password for SMTP, DAV and ManageSieve. Server addresses only come from the bridge's configuration file: users never type a host.

## Quick start

### 1. Get the binary

**From a release.** Each release has static Linux binaries (no dependencies) for x86_64 and aarch64: `nebula-bridge-<version>-<arch>-linux-musl.tar.gz`, with the binary, `bridge.example.toml` and the license. Check the signature and checksum first (see "Verifying a release" in [SECURITY.md](../SECURITY.md)), then:

```sh
tar xzf nebula-bridge-1.2.3-x86_64-linux-musl.tar.gz
sudo install -m 755 nebula-bridge-1.2.3-x86_64-linux-musl/nebula-bridge /usr/local/bin/
```

**From source.** You need Rust (the version is pinned in `bridge/rust-toolchain.toml`; `rustup` picks it up):

```sh
cd bridge
cargo build --release --locked -p nebula-bridge
sudo install -m 755 target/release/nebula-bridge /usr/local/bin/
```

### 2. User, directories and keys

```sh
sudo useradd --system --home /var/lib/nebula-bridge --create-home nebula-bridge
sudo chmod 700 /var/lib/nebula-bridge
sudo install -d -m 750 -o nebula-bridge /etc/nebula-bridge

# Master key: protects every stored password. Back it up (see "Backups").
nebula-bridge gen-master-key | sudo install -m 600 -o nebula-bridge /dev/stdin /etc/nebula-bridge/master.key

# OAuth client secret for the web app: prints a new secret and its hash.
nebula-bridge hash-secret < /dev/null
```

`hash-secret` prints two lines, `secret: …` and `hash: …`. The **secret** goes into the web app (`OAUTH_CLIENT_SECRET`), the **hash** into the bridge's configuration. The bridge only keeps the hash. (To hash a secret you already have, pipe it in: `printf %s "$SECRET" | nebula-bridge hash-secret`.)

### 3. Configuration

Copy `bridge/bridge.example.toml` to `/etc/nebula-bridge/bridge.toml` (it explains every option) and edit it. A minimal file:

```toml
[server]
listen = "127.0.0.1:8080"
public_url = "https://jmap.example.com" # the web app's JMAP_SERVER, exactly
app_origins = ["https://mail.example.com"] # the web app's origin
client_ip = "x-forwarded-for" # see "HTTPS in front"

[store]
path = "/var/lib/nebula-bridge/bridge.db"

[secrets]
master_key_file = "/etc/nebula-bridge/master.key"

[[oauth.clients]]
id = "nebulamail"
secret_hash = "$argon2id$v=19$…" # from hash-secret
redirect_uris = ["https://mail.example.com/auth/callback"]

[upstream.imap]
host = "imap.example.com" # port 993, implicit TLS

[upstream.smtp]
host = "smtp.example.com" # port 465, implicit TLS
```

Unknown keys are an error, and so are values out of range: the bridge refuses to start and names the key. Relative paths are relative to the configuration file's directory.

### 4. Run it as a service

With systemd or OpenRC. Either way the bridge runs as `nebula-bridge`, with a private umask (0077) and 30 seconds to stop.

#### systemd

`/etc/systemd/system/nebula-bridge.service`:

```ini
[Unit]
Description=Nebula Mail bridge
After=network-online.target
Wants=network-online.target

[Service]
User=nebula-bridge
ExecStart=/usr/local/bin/nebula-bridge serve --config /etc/nebula-bridge/bridge.toml
Restart=on-failure
TimeoutStopSec=30
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
UMask=0077
StateDirectory=nebula-bridge
StateDirectoryMode=0700

[Install]
WantedBy=multi-user.target
```

```sh
sudo systemctl enable --now nebula-bridge
journalctl -u nebula-bridge -f
```

`ProtectSystem=strict` makes the system read-only for the bridge; `StateDirectory=` keeps `/var/lib/nebula-bridge` writable. If `store.path` is elsewhere, add its directory with `ReadWritePaths=`.

#### OpenRC

For Gentoo, Alpine, Artix, Devuan… `/etc/init.d/nebula-bridge`:

```sh
#!/sbin/openrc-run

description="Nebula Mail bridge"
command="/usr/local/bin/nebula-bridge"
command_args="serve --config /etc/nebula-bridge/bridge.toml"
command_user="nebula-bridge:nebula-bridge"
supervisor="supervise-daemon"
respawn_delay=5
umask=077
output_log="/var/log/nebula-bridge/bridge.log"
error_log="/var/log/nebula-bridge/bridge.log"
# SIGTERM, up to 30 s to finish open requests, then SIGKILL.
retry="TERM/30/KILL/5"

depend() {
	need net
	after firewall
}

start_pre() {
	checkpath -d -m 0700 -o nebula-bridge:nebula-bridge /var/lib/nebula-bridge
	checkpath -d -m 0750 -o nebula-bridge:nebula-bridge /var/log/nebula-bridge
}
```

```sh
sudo chmod 755 /etc/init.d/nebula-bridge
sudo rc-update add nebula-bridge default
sudo rc-service nebula-bridge start
tail -f /var/log/nebula-bridge/bridge.log
```

`supervise-daemon` restarts the bridge if it crashes. To set the log level, put `export RUST_LOG=debug` in `/etc/conf.d/nebula-bridge`. Rotate the log with logrotate using `copytruncate`, because the bridge keeps the file open.

On Alpine, which has no `useradd`, create the user from step 2 with `addgroup -S nebula-bridge && adduser -S -D -G nebula-bridge -h /var/lib/nebula-bridge -s /sbin/nologin nebula-bridge`.

### 5. HTTPS in front

The bridge speaks plain HTTP only. Put a reverse proxy with TLS in front. With Caddy:

```text
jmap.example.com {
	reverse_proxy 127.0.0.1:8080
}
```

- Push uses a long-lived EventSource connection: don't buffer responses (Caddy doesn't by default; for nginx, `proxy_buffering off`).
- Uploads and downloads are streamed, so the proxy doesn't need to buffer them either (for nginx, `proxy_request_buffering off` on `/jmap/upload/` is fine).
- Make sure clients can't reach the bridge's port without going through the proxy.

**`server.client_ip` is required when `public_url` is https.** Behind a proxy, every connection comes from the proxy, so the bridge needs to know where to find the real client IP (used for the sign-in limits). Without the setting it doesn't start (`server.client_ip is required when public_url is https: …`).

- `"x-forwarded-for"`: the right choice when your proxy sets that header (Caddy does by default; nginx with `proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;`). The bridge takes the **last** address in the header, the one your proxy added; if it's missing or not an IP, it uses the connection's address. With several proxies (a CDN in front of nginx), the one next to the bridge must put the real client address last (nginx `real_ip`, Caddy `trusted_proxies`).
- `"peer"`: the TCP connection's address. Behind a proxy every client then shares the proxy's address, so a few failed sign-ins by anyone block everyone; the bridge starts but logs a warning.
- With `public_url` on `http://localhost` (development) you can leave it out: it is `"peer"`.

### 6. Point the web app at it

In the web app's environment (all its settings are described in `apps/web/.env.schema`):

```sh
APP_URL=https://mail.example.com
JMAP_SERVER=https://jmap.example.com      # exactly the bridge's public_url, no trailing slash
OAUTH_CLIENT_ID=nebulamail                   # oauth.clients id
OAUTH_CLIENT_SECRET=<secret from hash-secret>
```

Open the app and sign in: the button takes you to the bridge's sign-in page. The web app's full proxy mode (`JMAP_PROXY=on`, [jmap-proxy.md](jmap-proxy.md)) works with the bridge too; then `JMAP_INTERNAL_URL` is where the web server reaches the bridge, and `app_origins` doesn't matter.

### Not sure which servers a domain uses?

```sh
nebula-bridge discover example.com                     # add --private-network for a server on your LAN
nebula-bridge discover example.com --no-ispdb          # don't ask Thunderbird's public database
```

It prints what the domain offers (IMAP, SMTP, ManageSieve, CalDAV, CardDAV) as configuration to review and paste. It looks at DNS SRV records (RFC 6186, RFC 8314, RFC 6764, `_sieve._tcp`), the domain's Thunderbird autoconfig file, Autodiscover and, for IMAP and SMTP, Thunderbird's public database (ISPDB, which learns the domain you asked about). If no SRV record names ManageSieve, port 4190 of the IMAP host is tried.

- It **writes nothing** and sends no password. It connects to each server and reports `works` (valid certificate, or a real ManageSieve greeting over TLS), `answers` (a STARTTLS port that greets) or why it failed.
- DNS and autoconfig files aren't authenticated: whoever forges them could collect your users' passwords. **Check the hosts before you use them.** The bridge never discovers servers by itself at runtime.
- What it finds passes the same network checks as a configured server (standard mail ports and 443 only, public addresses only unless `--private-network`, no unencrypted servers); what was refused is listed as comments.

## Configuration reference

Every key is optional unless marked **required**. These are the defaults the code uses; `bridge/bridge.example.toml` has the same with comments.

### `[server]`

| Key | Default | Meaning |
| --- | --- | --- |
| `listen` | **required** | Address and port, e.g. `"127.0.0.1:8080"`. |
| `public_url` | **required** | The bridge's public URL, an origin only (no path). It is the OAuth issuer and the base of every JMAP URL. https, or http only for loopback. The web app's `JMAP_SERVER` must be exactly this. |
| `app_origins` | `[]` | Origins allowed to call JMAP from the browser (CORS), e.g. `["https://mail.example.com"]`. Needed unless the web app runs with `JMAP_PROXY=on`. |
| `client_ip` | `"peer"` with an http `public_url`; **required** with https | `"x-forwarded-for"` or `"peer"`. See "HTTPS in front". |

### `[store]`

| Key | Default | Meaning |
| --- | --- | --- |
| `path` | **required** | The SQLite database file. The bridge creates it with mode 0600. |

### `[secrets]`

The master key (32 random bytes, base64) comes from **one** of these places; setting it in two is an error.

| Key | Default | Meaning |
| --- | --- | --- |
| `master_key_file` | none | File with the master key (version 1). |
| `[[secrets.keys]]` | none | Several keys for rotation: each with `version` (1 or more) and `file`. The highest version is the current one. |
| env `NEBULA_BRIDGE_MASTER_KEY` | none | The key in base64, or `1:<base64>,2:<base64>` for several versions. Only if neither key above is set. |

### `[[oauth.clients]]`

One entry per web app (the confidential client of the web app's server). There is no dynamic registration.

| Key | Meaning |
| --- | --- |
| `id` | **Required.** The web app's `OAUTH_CLIENT_ID`. |
| `secret_hash` | **Required.** The argon2id hash from `nebula-bridge hash-secret`. |
| `redirect_uris` | **Required.** Exact redirect URIs: the web app's `APP_URL` + `/auth/callback`. |

### `[upstream]`, `[upstream.imap]`, `[upstream.smtp]`

| Key | Default | Meaning |
| --- | --- | --- |
| `upstream.private_network` | `false` | Allow servers on private, loopback or unique-local addresses (a Dovecot on your LAN or the same machine). Applies to IMAP, SMTP, DAV and ManageSieve. Link-local and cloud metadata addresses are always refused. |
| `upstream.sent_copy` | `"append"` | Who saves sent mail in Sent: `"append"` (the bridge, with IMAP APPEND) or `"server"` (servers that do it themselves, like Gmail; otherwise you get two copies). |
| `upstream.email_domain` | none | Domain added to logins without `@`: `alice` → `alice@example.com`, which is also the user's sending address. Leave it out if users sign in with their full address. |
| `imap.host`, `smtp.host` | **required** | Host name or IP address. |
| `imap.tls`, `smtp.tls` | `"implicit"` | `"implicit"` (TLS from the start) or `"starttls"` (required: if the server doesn't offer it, the bridge doesn't connect). Implicit TLS is preferred (RFC 8314). There is no plain-text option. |
| `imap.port` | `993` (implicit), `143` (starttls) | Only this port is ever used. |
| `smtp.port` | `465` (implicit), `587` (starttls) | Only this port is ever used. |
| `imap.tls_verify`, `smtp.tls_verify` | `true` | Check the certificate against the system's trust store. `false` is only accepted for loopback hosts (development with a self-signed certificate). |

### `[upstream.dav]` (calendars and contacts)

Without this section the web app hides Calendar and Contacts. See "Calendars, contacts and tasks".

| Key | Default | Meaning |
| --- | --- | --- |
| `url` | **required** | Where discovery starts, e.g. `https://dav.example.com/`. `/.well-known/caldav` and `/.well-known/carddav` are tried too. https; http only for loopback. No user, password, query or fragment. |
| `contacts_url` | none | Only for providers whose address books live on another host than calendars (Fastmail, iCloud). Address books are then found only from here and calendars only from `url`. Same rules as `url`; can't equal `url` or be set with `contacts = false`. |
| `calendars`, `contacts` | `true`, `true` | What to sync (at least one). |
| `tls_verify` | `true` | `false` only for loopback hosts (both `url` and `contacts_url`). |
| `allowed_hosts` | `[]` | Other origins (`"https://host[:port]"`, https only, no path) the server may send the bridge to: redirects, homes on another host. The password is **never** sent there. Can't contain `url` or `contacts_url`. |
| `scheduling` | `"auto"` | Who sends invitations: `"auto"` (the server if it announces `calendar-auto-schedule`, else the bridge by SMTP), `"bridge"` or `"server"`. Use `"server"` only if the server really schedules, or nobody sends anything. |
| `poll_seconds` | `60` | How often a signed-in account is synced while the app is open (10 to 86400). |
| `poll_idle_seconds` | `300` | Same, with no app open (10 to 86400). |

### `[upstream.dav.alerts]` (event reminders)

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `true` | `false`: no reminders at all (open app or Web Push). |
| `grace_minutes` | `10` | How late a reminder may still be sent (1 to 1440); also how long a push service keeps it for an offline browser. |
| `lead_days` | `8` | How long before an event its reminders may ring (1 to 366). An alarm "two weeks before" needs 15. |
| `horizon_hours` | `24` | How far ahead reminders are planned (2 to 168). |
| `max_events` | `2000` | Events looked at per plan, newest first (1 to 100000). |
| `max_per_event` | `64` | Reminders per event and plan, repeats included (1 to 1000). |
| `max_repeats` | `10` | Repeats of one alarm (`REPEAT`/`DURATION`), 0 to 100; 0: each alarm rings once. Never more than once a minute. |

The same, with the defaults written out:

```toml
[upstream.dav.alerts]
enabled = true
grace_minutes = 10
lead_days = 8
horizon_hours = 24
max_events = 2000
max_per_event = 64
max_repeats = 10
```

### `[upstream.sieve]` (filters and vacation response)

Without this section the web app hides Filters and the vacation response. See "Filters and vacation response".

| Key | Default | Meaning |
| --- | --- | --- |
| `host` | the IMAP host | ManageSieve server. |
| `port` | `4190` |  |
| `tls` | `"starttls"` | `"starttls"` (required) or `"implicit"`. |
| `tls_verify` | the IMAP setting if `host` is unset, else `true` | `false` only for loopback hosts. |
| `max_script_size` | `1048576` | Largest script in bytes (1024 to 16777216). |
| `max_scripts` | `100` | Most scripts per user, `vacation` included (1 to 1000). |
| `poll_seconds` | `60` | How often an open app's scripts are re-read (15 to 3600). |

The same, with the defaults written out (`host` is the IMAP host when unset):

```toml
[upstream.sieve]
# host = "imap.example.com"
port = 4190
tls = "starttls"
tls_verify = true
max_script_size = 1048576
max_scripts = 100
poll_seconds = 60
```

### `[session]`

| Key | Default | Meaning |
| --- | --- | --- |
| `access_token_minutes` | `15` | Access token lifetime (1 to 60). |
| `refresh_token_days` | `30` | Refresh token lifetime; refresh tokens rotate on every use. |
| `max_days` | `30` | Users must sign in again after this, whatever happens. |

### `[limits]` (announced in the JMAP session)

| Key                         | Default                                 |
| --------------------------- | --------------------------------------- |
| `max_size_upload`           | `50000000` bytes (1 KiB to 1 GiB)       |
| `max_concurrent_upload`     | `4`                                     |
| `max_size_request`          | `10000000` bytes (1 KiB to 64 MiB)      |
| `max_concurrent_requests`   | `4`                                     |
| `max_calls_in_request`      | `32`                                    |
| `max_objects_in_get`        | `500` (at most 1000)                    |
| `max_objects_in_set`        | `500`                                   |
| `event_streams_per_account` | `8` (open push connections per account) |

### `[login_limits]`

| Key | Default | Meaning |
| --- | --- | --- |
| `per_login` | `5` | Failed sign-ins per login name in the window. |
| `per_ip` | `20` | Failed sign-ins per IP (IPv6: per /64; also per /48 with 16 times this limit). |
| `window_minutes` | `15` | The sliding window. |

See "Sign-in limits" for how known browsers are treated.

### `[submission]` (undo send and scheduled send)

| Key | Default | Meaning |
| --- | --- | --- |
| `hold_days` | `7` | Most days ahead a message can be scheduled (0 to 30). `0` turns off undo send and scheduled send. |
| `max_held` | `100` | Messages waiting per user (1 to 10000). |
| `max_held_mb` | `200` | MiB waiting per user (1 to 100000). |
| `wait_hours` | `72` | How long a due message waits for its user to sign in (or for the SMTP server) before it is marked not delivered (1 to 720). |
| `send_while_signed_out` | `false` | Let users have scheduled mail sent after they sign out. See below. |

### `[push]` (Web Push)

| Key | Default | Meaning |
| --- | --- | --- |
| `contact` | **required** | `mailto:` address or `https:` URL for push services to contact you (the VAPID `sub`). |
| `allowed_hosts` | the browsers' push services | Push hosts the bridge may send to. Default: `fcm.googleapis.com`, `*.push.services.mozilla.com`, `*.push.apple.com`, `*.notify.windows.com`. Setting it replaces the list (`"*.example.com"` for subdomains); `["*"]` (alone) allows any public host. |
| `max_subscriptions` | `10` | Push subscriptions per login session (1 to 100). |
| `calendar_alerts` | `true` | `false`: reminders only reach an open app. |

### Environment variables

| Variable | Meaning |
| --- | --- |
| `NEBULA_BRIDGE_MASTER_KEY` | The master key(s), instead of a file (see `[secrets]`). |
| `RUST_LOG` | Log filter. Default `info`; `RUST_LOG=debug` shows more. Logs never contain passwords or tokens. |

### Commands

```text
nebula-bridge serve --config <file>              run the bridge
nebula-bridge rewrap-keys --config <file>        re-wrap keys with the current master key (also done at start)
nebula-bridge rotate-vapid-key --config <file>   delete the Web Push key and all push subscriptions
nebula-bridge discover <domain> [--private-network] [--no-ispdb]
nebula-bridge hash-secret                        hash a client secret from stdin (empty input: make one)
nebula-bridge gen-master-key                     print a new master key
nebula-bridge --version
```

## Provider examples

Users always use the same login and password for IMAP, SMTP, DAV and ManageSieve. With providers that use two-factor login, that means an **app password** that has access to all of them.

| Provider | Mail | Calendars and contacts | Filters | Tested |
| --- | --- | --- | --- | --- |
| Dovecot + Postfix | Yes | with a DAV server | Pigeonhole | In CI (Dovecot 2.4.5, Postfix 3.10) |
| Radicale | — | Yes | — | In CI (3.8.3), also with the web app's browser tests |
| Stalwart's CalDAV/CardDAV | — | Yes | Stalwart's ManageSieve (no vacation) | In CI. With Stalwart, use its JMAP directly instead. |
| Nextcloud | — | Yes | — | In CI (34.0.5) |
| Baïkal / sabre/dav | — | Yes | — | In CI (0.10.1, sharing tests) |
| Fastmail | Yes | Yes (`contacts_url`) | — | **Not tested** against Fastmail. The two-host setup is tested in CI with Radicale and Baïkal standing in. |
| iCloud | Yes | Yes (`contacts_url`) | — | **Not tested** against iCloud; its per-account hosts are tested only with fake servers. |
| Gmail / Google Workspace | Yes | **No** (Google needs OAuth) | — | **Not tested** against Gmail. |

### Dovecot and Postfix

```toml
[upstream]
private_network = true # only if the servers are on your LAN or this machine

[upstream.imap]
host = "mail.example.com"

[upstream.smtp]
host = "mail.example.com"
port = 587
tls = "starttls"

[upstream.sieve] # Dovecot's Pigeonhole ManageSieve on 4190
```

The bridge saves sent mail in Sent (`sent_copy = "append"`, the default). Filters and the vacation response work through Pigeonhole, also with an `INBOX.` folder prefix.

### Gmail / Google Workspace (mail only)

Users sign in with their full address and a Google **app password**.

```toml
[upstream]
sent_copy = "server" # Gmail saves sent mail itself

[upstream.imap]
host = "imap.gmail.com"

[upstream.smtp]
host = "smtp.gmail.com"
```

Signing in with Google's OAuth, Google Calendar and Google Contacts are not supported.

### Fastmail

Users sign in with their full address and an **app password** (Fastmail → Settings → Privacy & Security → App passwords) with access to IMAP, SMTP, CalDAV and CardDAV. Calendars and address books are on two hosts. (Fastmail also offers JMAP itself, which the web app could use directly.)

```toml
[upstream.imap]
host = "imap.fastmail.com"

[upstream.smtp]
host = "smtp.fastmail.com"

[upstream.dav]
url = "https://caldav.fastmail.com/"
contacts_url = "https://carddav.fastmail.com/"
```

### iCloud

Users sign in with their **full Apple ID address** and an **app-specific password** (appleid.apple.com → Sign-In and Security → App-Specific Passwords).

```toml
[upstream.imap]
host = "imap.mail.me.com"

[upstream.smtp]
host = "smtp.mail.me.com"
tls = "starttls" # port 587

[upstream.dav]
url = "https://caldav.icloud.com/"
contacts_url = "https://contacts.icloud.com/"
```

iCloud serves each account from its own host (`p<number>-caldav.icloud.com`, `p<number>-contacts.icloud.com`). The bridge has a built-in, narrow exception for exactly those hosts (see "Security model"), so there is nothing else to configure: no `allowed_hosts`. Set both URLs.

### Nextcloud

Point `url` at the DAV root. Users with two-factor login need a Nextcloud app password, and it must also be their IMAP password.

```toml
[upstream.dav]
url = "https://cloud.example.com/remote.php/dav/"
```

Nextcloud schedules by itself (it sends invitations), offers free/busy and sharing. New Nextcloud users also have a read-only "Contact birthdays" calendar and read-only system and "Recently contacted" address books.

### Radicale

```toml
[upstream.dav]
url = "https://dav.example.com/"
```

Radicale doesn't schedule, so the bridge sends invitations by SMTP. It has no free/busy (the app hides "Busy") and no sharing. New users start with no collections; the bridge creates them.

### Baïkal

```toml
[upstream.dav]
url = "https://dav.example.com/dav.php/"
```

Baïkal schedules by itself, but only emails invitations if its `invite_from` is set. Sharing: calendars only. Baïkal hides other users, so to share a calendar type the person's full address.

## Calendars, contacts and tasks

With `[upstream.dav]`, the bridge syncs each signed-in user's calendars and address books every `poll_seconds` (app open) or `poll_idle_seconds` (no app open). Every change goes to the DAV server first and is never applied blindly: if someone else changed the item meanwhile, it fails or is applied again on top. What other clients wrote that the app doesn't show (Apple's labels, the vCard version, unknown properties) is kept.

- **Calendars:** the app's calendar views and event details; create, edit, move and delete events and calendars (also one occurrence of a repeating event, all-day series included); name, color and order are stored on the server.
- **Contacts:** search, create, edit, groups (including Apple's vCard 3.0 groups), moving between address books, `.vcf` import and export, suggestions while writing. Photos up to 1 MiB are kept inside the contact; links to photos elsewhere are kept but never downloaded.
- **Tasks:** CalDAV `VTODO`s in calendars that take tasks: due dates, done/not done, priority, subtasks, repeating tasks, reminders, assigning tasks to other people. Tested with Radicale, Stalwart and Nextcloud.
- **New users** whose DAV account has no calendar or no address book get one of each ("Calendar", "Contacts") at sign-in, plus a "Tasks" list if no calendar takes tasks.
- If the DAV server refuses a user's password, only that user's calendars and contacts stop (mail keeps working) until they sign in again.

### Invitations

When you organize an event with guests, they get an email:

- If the DAV server schedules by itself (it announces `calendar-auto-schedule`: Stalwart, Nextcloud, Baïkal), **the server** sends invitations, updates and cancellations.
- Otherwise (Radicale, or `scheduling = "bridge"`), **the bridge** sends them through the user's SMTP: one email per guest, at most 20 guests per change, nothing for events that already ended, and a copy in Sent (unless `sent_copy = "server"`). The text is in English or Spanish, following the user's language.

Answers to your invitations arrive as emails. A scheduling server applies them by itself; with Radicale, open the answer and press **Update my event** on its card. "Propose a new time" and its answers are sent by the bridge with every server. Assigned tasks follow the same rules.

### Reminders

The bridge works out when each event's reminder is due (display and sound alarms, not email alarms) and tells the open app, or the browser with every tab closed when `[push]` is set. Dismissing and snoozing are saved in the event, so every device sees them. A reminder that can't be delivered on time (no app open, bridge down) is still sent up to `grace_minutes` late, then dropped. Tune it in `[upstream.dav.alerts]`.

### Sharing

Sharing calendars and address books works where the DAV server supports it, the way it does it. Elsewhere the app doesn't offer "Share…". The share is made on the server, so every other app sees it, and the other person must be a user of the same DAV server.

| Server | What can be shared | Levels |
| --- | --- | --- |
| Stalwart's DAV (WebDAV ACL) | calendars and address books | free/busy only, read, read and write |
| Nextcloud | calendars and address books, with users or groups | read, read and write |
| Baïkal, sabre/dav, Apple (Calendar Server sharing) | calendars only | read, read and write |
| Radicale | nothing | — |

- What others share with you appears in **their account** in the app ("Shared by alice@…").
- "Free/busy only" needs WebDAV ACL. On Nextcloud and Calendar Server sharing, the bridge refuses it rather than giving read access.
- Nextcloud doesn't list its users; the bridge searches them by name or address, so Nextcloud's user enumeration settings apply.
- A calendar or address book can be shared with up to 10 000 people, and an account can have up to 50 000 sharees in total.

### Free/busy

When you add guests, the app can show when they are busy. This works on scheduling servers (Stalwart, Nextcloud…) and only for people on the same DAV server. Your own free/busy is computed by the bridge from all your calendars. Details of events (`showDetails`) only ever come from your own calendars, and never for private or confidential events.

### Nextcloud extras

Only where Nextcloud announces them:

- **Publish a calendar** at a public link (from "Share…"). Anyone with the link sees every event of that calendar; Nextcloud serves it.
- **Subscriptions** (`webcal://` and `https://` calendars) appear as read-only calendars. **Nextcloud downloads them**, never the bridge.
- **Trash:** deleted calendars and events can be restored ("⋯" next to "My calendars" → "Trash").

## Filters and vacation response (ManageSieve)

```toml
[upstream.sieve]
# host = "imap.example.com"   # default: the IMAP host
# port = 4190
# tls = "starttls"
```

With this section the web app's **Filters**, its Sieve script editor and the **Vacation response** work against a ManageSieve server (RFC 5804: Dovecot's Pigeonhole, Cyrus, Stalwart…). The bridge offers JMAP for Sieve (RFC 9661) and `VacationResponse` (RFC 8621) and turns them into ManageSieve commands with the user's own login.

- The feature appears for a user once the bridge has logged in to ManageSieve for them. If the server is down it retries in the background (from 30 s up to every 15 minutes); if it refuses the password, that user has no filters (mail goes on).
- The vacation response is the script named `vacation`, written by the bridge. `vacation` is a reserved name.
- **Script limit:** at most `max_scripts` per user. A user who already has more on the server (made with another client) gets "server unavailable" for filters and vacation until some are deleted or the limit is raised.
- ManageSieve can't announce changes, so while the app is open the bridge re-reads the scripts every `poll_seconds`.
- Script contents are opaque: the bridge never parses or rewrites a user's scripts.
- **Stalwart's ManageSieve** reserves `vacation` itself, so through the bridge it has filters but no vacation response.

Tested with Dovecot 2.4.5 (Pigeonhole) and Stalwart's ManageSieve.

## Undo send and scheduled send

The bridge holds each delayed message in its database, encrypted with the user's key, and sends it at its time with the user's login. If the user has no session open at that time, the message waits and goes out at their next sign-in; after `wait_hours` it is marked not delivered.

With `submission.send_while_signed_out = true`, users can choose to have scheduled messages sent even after they sign out. The bridge then keeps their sealed password (as during a session) until the last such message is sent, fails, is canceled or reaches `wait_hours` past its time. Revoking a session as stolen, or the mail server refusing the password, deletes it at once. The log says `password kept after sign-out` with the account id and count.

## Notifications with every tab closed (Web Push)

```toml
[push]
contact = "mailto:admin@example.com"
```

With this section the web app can notify of new mail and event reminders even with every tab closed, as with Stalwart. The bridge sends to the browser's push service, encrypted for that browser (RFC 8291) and signed with VAPID (RFC 8292). The push service only sees an encrypted blob.

- **Outbound access:** the bridge must reach the push services over HTTPS (port 443).
- **The VAPID key** is created at the first start with `[push]` and stored in the database, sealed under the master key. Back up the database to keep it: a new key means every browser has to subscribe again.
- **Rotating it:** `nebula-bridge rotate-vapid-key --config /etc/nebula-bridge/bridge.toml`, then restart. Browsers subscribe again the next time the app opens.
- **Push addresses come from the browser** (that is, from users), so they are checked: https on port 443, public addresses only (checked at every connection), no redirects, and by default only the browsers' push services (`allowed_hosts`).
- **Limits:** `max_subscriptions` per login session; at most one message every 2 s per subscription; 30 at once per account, then one more every 10 s. Subscriptions last 7 days at most (the web app renews them) and end with the login session.
- After a restart, accounts with subscriptions start at once, so notifications keep coming.

## Security model

**Where hosts come from.** Every server address (IMAP, SMTP, DAV, ManageSieve) comes from the configuration file. Users never type a host, and nothing a server says (redirects, principal URLs, `Location` headers) can send the bridge to another host unless its origin is in `upstream.dav.allowed_hosts`.

**SSRF guard.** Every outgoing connection goes through the bridge's network guard (`net-guard`): the configured port only, every resolved IP checked, the connection pinned to the checked IP, no proxies from the environment. Private and loopback addresses only with `upstream.private_network = true`; link-local and metadata addresses never. DAV follows at most 5 redirects, each one checked again.

**Passwords go only where you configured.** The DAV password is sent only to the origin of `url` (and `contacts_url`): never to an `allowed_hosts` origin, never after a redirect to another origin. The one built-in exception is iCloud: only when `url` or `contacts_url` is exactly `https://caldav.icloud.com` or `https://contacts.icloud.com`, a host named exactly `p<1 to 4 digits>-caldav.icloud.com` (or `-contacts.` for the second) on https port 443 counts as that server. The check is on the parsed host, label by label, so lookalikes such as `p42-caldav.icloud.com.evil.com` are refused. It is not configurable.

**TLS always.** IMAP, SMTP and ManageSieve use implicit TLS or required STARTTLS; DAV uses https. Certificates are checked against the system's trust store. Turning that off, and plain http for DAV, is only possible for loopback hosts (development).

**Nothing in the data is downloaded.** Attachment URLs, photo links and calendar subscriptions in events and contacts are kept as text and never fetched by the bridge. XML from DAV servers is parsed without DTDs, with size and depth limits.

**Credentials at rest.** The bridge stores, per account:

- the mail login and the **password sealed** (XChaCha20-Poly1305) with a per-account key, which is wrapped with the **master key**. The sealed password is deleted when the user's last session ends (except for `send_while_signed_out`, above);
- **hashes** of OAuth tokens, never the tokens;
- with DAV, a copy of each event and contact **sealed with the same per-account key**, with only a few indexes in clear (UID, start and end times, sort name);
- with Web Push, each browser's push address and keys, sealed the same way.

**Not encrypted:** the mail metadata cache (folders, flags, threads, headers such as subjects and senders, a short text preview) and pending uploads. Bodies and attachments stay on the mail server and are fetched when opened. **Protect the database file like the mailboxes themselves.**

**The master key.** Whoever has both the database and the master key can open the stored passwords of users with a session open. Keep the key out of database backups and, if you can, off the disk:

- **systemd + TPM:** `systemd-creds encrypt --tpm2-device=auto master.key /etc/credstore.encrypted/nebula-bridge-master`, then `LoadCredentialEncrypted=nebula-bridge-master` in the unit and `master_key_file = "/run/credentials/nebula-bridge.service/nebula-bridge-master"` (the configuration file does not expand variables).
- **Docker / Kubernetes secrets:** mount the key as a secret file and point `master_key_file` at it.
- **Vault or a cloud KMS:** have the service manager fetch the key at start into a tmpfs file or `NEBULA_BRIDGE_MASTER_KEY`.

**Sign-in limits.** Failed sign-ins are limited per login name and per IP (`[login_limits]`). After a successful sign-in the browser gets a random "known browser" cookie (`__Host-nebula-device`, or `nebula-device` over http) lasting as long as a session; the bridge keeps only its hash, for the last 16 browsers of each account. A known browser has its own per-login allowance, so someone guessing a user's password doesn't lock that user out of the browser they normally use. The per-IP limit still applies to everyone, so users behind the same IP as an attacker (an office, carrier-grade NAT, a VPN) can still be blocked.

**Logins and case.** `Alice` and `alice` are the same account (ASCII case only; `Élise` and `élise` are two). The spelling of the first sign-in is kept and used with the servers. Signing in with another spelling also checks the password for the stored spelling, so on a server that tells them apart one user can't take over another's account.

## Operations

### Data and permissions

Everything the bridge keeps is in the one SQLite database at `store.path` (plus its `-wal` and `-shm` files). The bridge creates it with mode 0600 and at start warns (it changes nothing) if the database or its companions can be read by group or others ("use chmod 600"), or the directory is open to others ("use chmod 700"; 750 is fine). It also warns if the master key file is readable by others. Both services above (umask 0077, state directory 0700) keep everything private. A database copied from elsewhere may need `chmod 600 bridge.db*`.

### Backups

- **The master key:** always, and stored apart from the database backups.
- **The database:** `sqlite3 /var/lib/nebula-bridge/bridge.db ".backup bridge.bak"` (safe while the bridge runs).

Losing the database isn't fatal: users sign in again and the cache is rebuilt, but message ids change (open web sessions reload), held messages are lost, and with `[push]` browsers must subscribe again. Losing the master key means the stored passwords can't be opened: users just sign in again.

### Rotating the master key

1. Make a new key (`nebula-bridge gen-master-key`) and list both keys with `[[secrets.keys]]`: the old one with its version, the new one with a higher version:

   ```toml
   [[secrets.keys]]
   version = 1
   file = "/etc/nebula-bridge/master-1.key"
   [[secrets.keys]]
   version = 2
   file = "/etc/nebula-bridge/master-2.key"
   ```

2. Restart the bridge. At start it wraps every account key (and the VAPID key) again with the newest master key, and logs either `the older keys (1) are no longer needed and can be removed` or an error with how many keys it couldn't open. `nebula-bridge rewrap-keys --config …` does the same without serving.
3. Once the log says so, remove the old key from the configuration and delete its file (keep it as long as you keep database backups made with it).

### Logs

Logs go to stderr: the journal with systemd, `/var/log/nebula-bridge/bridge.log` with the OpenRC script above. Set the level with `RUST_LOG` (default `info`). Passwords and tokens are never logged.

### Stopping

On SIGTERM or Ctrl-C the bridge stops accepting connections at once, gives open requests 20 seconds to finish, then closes them and stops. The services above wait 30 s, and so does Kubernetes by default, which is enough. Docker waits only 10 s: see below.

### In a container

There is no official container image yet. The release binary is static, so it runs in any minimal image. Give it time to stop and a private volume:

```yaml
services:
  bridge:
    # …
    stop_grace_period: 30s
    volumes:
      - bridge-data:/var/lib/nebula-bridge
```

By hand, use `docker stop -t 30`. Keep the data volume readable only by the bridge's user.

### Upgrades

Back up the database, replace the binary and restart. The database schema is migrated at start; there is no downgrade, so keep the backup until you're happy with the new version.

### Network timeouts

The bridge drops clients that take more than 15 s to send a request's headers, closes connections that stop taking bytes for 60 s, and allows at most 64 concurrent requests per HTTP/2 connection.

## Limitations

- One mail server per bridge; the DAV and ManageSieve servers must accept the same login and password as IMAP.
- No Google sign-in (OAuth) and no Google Calendar or Contacts; Gmail works for mail with an app password.
- No sharing on Radicale; no address book sharing on Baïkal/sabre/dav; no "free/busy only" sharing on Nextcloud or Calendar Server sharing.
- Free/busy needs a scheduling server and only covers people on the same DAV server.
- Stalwart's ManageSieve: filters but no vacation response through the bridge.
- Email alarms in events are not sent; only display and sound reminders.
- No official container image yet.

## Trying it locally

`docker/compose.yaml` has Dovecot, Postfix, Radicale, Baïkal and Nextcloud for development in its `bridge` profile (`docker/README.md` explains the setup), and `bridge/dev/dovecot.toml` runs the bridge against Dovecot and Radicale; its header comment has the exact commands and the web app settings. The web app's browser tests against the bridge (`bun run test:bridge` in `apps/web`) cover mail, contacts, the calendar and Web Push this way.
