<!-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# Local servers

Stalwart (latest release only) for development. Nothing here runs until you start it.

The dev Stalwart keeps its data between runs: never recreate it casually (`down -v`, removing its volumes).

1. `cp docker/.env.example docker/.env` and set a password.
2. `docker compose -f docker/compose.yaml up -d stalwart`
3. `bun run dev:setup` finishes Stalwart's initial setup and prepares it for development:
   - domain `nebulamail.test` with the accounts `alice@` and `bob@`, a few sample emails for alice, and the group `ventas@` («Ventas», with bob; for the bridge's `alice_shares_with_a_group`, which shares with it through Stalwart's DAV);
   - OAuth client `nebula-dev` (confidential, redirect `http://localhost:3001/auth/callback`, the scopes mail, contacts, calendars and files: without them Stalwart 1.0 refuses the app's login or leaves calendars, contacts and synced preferences out);
   - 15 min access tokens and CORS for JMAP (Bearer tokens only), applied with Stalwart's `ReloadSettings` action (no restart);
   - logs on stdout instead of files in `/var/log/stalwart/`, which doesn't exist in the image: read them with `docker compose -f docker/compose.yaml logs stalwart`. Raising the level to `trace` saves the `Authorization` headers (passwords) in the log: don't share it without removing them;
   - Stalwart adds its `Authentication-Results` header on every path, not only on port 25, so the one on top is always the server's and the "Verified sender" badge is safe to turn on (Settings → "Show “Verified sender” badge"). In production, turn on the same option (`addAuthResultsHeader`) before telling users to enable it, or force it for everyone with `VERIFIED_SENDER_BADGE=true` (`false` hides it for everyone). Read [docs/verified-sender.md](../docs/verified-sender.md) first: it lists when the badge can be wrong.

   Passwords and secrets go to git-ignored files: `docker/.env` (admin and test accounts) and `apps/web/.env.local` (what the app needs). Running it again only creates what is missing.

`bun run dev:seed-load` adds 3000 test emails (about 1700 conversations, some unread or starred) to a separate **Load test** folder in alice's account, to try long lists and threads; `--count=N` for another number, `--remove` deletes the folder again. The inbox and the sample emails are not touched.

`bun run dev:share` prepares shared folders and a quota: alice shares **Compartida con Bob** with bob (read and organize) and bob shares **Compartida con Alice** with alice (read only), each with one email; alice gets a 100 MB disk quota so the sidebar shows the storage bar. Safe to run again.

`bun run dev:seed-calendar` adds calendar test data for alice in the current week: her own events (one repeating, one all-day, one with an HTML description in New York time), invitations from bob sent by Stalwart itself (one to answer, one cancelled), bob's answer to one of hers, and an invitation from outside (`carol@partner.example`) imported into her inbox, which Stalwart doesn't put in the calendar. `--smtp` sends that one through SMTP (port 2525, a minute or two); `--remove` deletes everything it made. Running it again adds only what is missing.

Stalwart's web interface (http://localhost:8080/admin) only signs in over https on the server's host name since 1.0: on this plain-http port it answers `Unknown client id`. To use it, start Stalwart in recovery mode (`STALWART_RECOVERY_MODE=1` in `docker/.env`, then `docker compose -f docker/compose.yaml up -d stalwart`) and sign in with `STALWART_RECOVERY_ADMIN`; recovery mode runs no mail services, so remove the variable and start it again afterwards. `dev:setup` and the scripts don't need it: they use the management API (JMAP) with the admin of `docker/.env`.

## The CI locally

`scripts/ci-local.sh` runs the jobs of `.github/workflows/ci.yml` on this machine (all, or the ones named; see CONTRIBUTING.md). Its `bridge-servers` job is the CI's job of the same name against these containers: unlike CI, which starts a fresh Stalwart and runs `dev:setup`, it uses the running `nebula-dev-stalwart-1` and `nebula-dev-dovecot-1` with their data and `docker/.env` as they are (never recreated, `docker/.env` never written; `CI_LOCAL_DEV_SETUP=1` runs `dev:setup`). It starts Radicale and Postfix only if they are not running, and stops at the end only what it started. One run sends about 15-25 messages through Stalwart (see its send limit below); `CI_LOCAL_SKIP_MAIL=1` sends none there.

## Throwaway Stalwart for the static zip

`stalwart-static` (profile `static`) is a second Stalwart, same version (1.0), only for `bun run test:static`: it serves the static zip as the static version would be installed. Start it in its own compose project (`-p nebula-static-test`, port 8095, anonymous volumes) and remove it afterwards; the development Stalwart and `docker/.env` are never involved. Its recovery admin comes from `STALWART_STATIC_RECOVERY_ADMIN` (random, set by whoever starts it).

## Dovecot (bridge tests)

A plain IMAP server for the bridge, in the `bridge` profile. Start it after Stalwart's setup, because its submission relays sent mail to Stalwart as alice (with `TEST_ALICE_PASSWORD` from `docker/.env`):

```sh
docker compose -f docker/compose.yaml --profile bridge up -d dovecot
```

Any user name logs in (`alice`, `bob`) with the password `password` (or `TEST_DOVECOT_PASSWORD`). Ports: IMAPS `2993`, IMAP with STARTTLS `2143`, submission `2465` (TLS) and `2587` (STARTTLS), ManageSieve (Pigeonhole, STARTTLS) `24190`, all with a self-signed certificate. Mail sent through it lands in Stalwart. The container is only for tests: it may be recreated (`docker compose -f docker/compose.yaml --profile bridge up -d --no-deps --force-recreate dovecot`; the mail stays in its volume).

Folders named `nebula-mail-test-*-noflags` lose the `w` right in this Dovecot (`dovecot/acl.conf`, mounted only in the `dovecot` service): their PERMANENTFLAGS are `(\Deleted \Seen)`, without `\*` nor `\Flagged`, for the test of keyword changes on copies that can't all keep them (`keyword_change_checks_every_copy_first` in `mail/tests/server.rs`). Every other folder keeps all rights. It also applies with `NEBULA_TEST_SERVER=postfix`, which uses this same Dovecot, but only to those folders. After changing it: `up -d --no-deps --force-recreate dovecot` (the volume stays).

Sieve tests, against Stalwart's ManageSieve (`4190`) or Dovecot's: `NEBULA_TEST_SIEVE=1 NEBULA_TEST_SERVER=dovecot cargo test -p nebula-managesieve --test server` and `… -p nebula-jmap --test sieve` in `bridge/`. With `NEBULA_TEST_SERVER=postfix`, a rule stored through the bridge also files a message Postfix delivers into Dovecot. Nothing is sent through Stalwart, and the tests put each account's scripts back as they were. The web app's filters and vacation response through the bridge: `e2e-bridge/filters.e2e.ts` (part of `bun run test:bridge`, user `filters-e2e`).

Folders under `INBOX.`, `.` between levels, with real vacation responses: a Dovecot and a Postfix of their own in the `bridge-prefix` profile, which change nothing in the others. IMAPS `2994`, ManageSieve `24191`, submission `2467`, same users and password:

```sh
docker compose -f docker/compose.yaml --profile bridge-prefix up -d --build --no-deps dovecot-prefix postfix-prefix
NEBULA_TEST_SIEVE=1 NEBULA_TEST_SERVER=dovecot-prefix cargo test -p nebula-jmap --test sieve -- --test-threads=1   # in bridge/
```

Sieve's own mail (vacation responses) goes from Dovecot to port `10025` of its Postfix, not published and without login, only for `@nebulamail.test` (`dovecot/nebula.conf`, `postfix/entrypoint.sh`). The `dovecot` and `postfix` containers get the same after `up -d --build --no-deps --force-recreate dovecot postfix`; until then their Sieve just can't send.

Bridge integration tests against either server: `NEBULA_TEST_IMAP=1 NEBULA_TEST_SMTP=1 NEBULA_TEST_SERVER=dovecot cargo test --test server -- --test-threads=1` in `bridge/` (`stalwart` is the default). One test at a time: they share alice's account, and Stalwart's search index falls behind when several of them write at once. Stalwart also limits alice's sends to bob (about 25 an hour): repeating the tests many times gets later sends deferred. The bridge with the web app: `bridge/dev/dovecot.toml`.

## Postfix (bridge tests, DSN)

Neither Stalwart (as set up here) nor Dovecot's submission announces DSN (RFC 3461), so the bridge's own ENVID and the matching of real delivery reports are tested against Postfix 3.10 (`docker/postfix`, built locally on Alpine), also in the `bridge` profile. Start it after Dovecot, where it delivers:

```sh
docker compose -f docker/compose.yaml --profile bridge up -d --build postfix
```

Submission on `2466` (TLS) and `2588` (STARTTLS), self-signed certificate for `localhost`. Also `2469` (TLS, same login), which doesn't announce 8BITMIME (`smtpd_discard_ehlo_keywords`; Postfix still takes 8-bit mail there): `seven_bit_downgrade_reaches_the_recipient_intact` (`jmap`, only with `NEBULA_TEST_SERVER=postfix`) sends through it, so the bridge's 7-bit downgrade meets a real server. After pulling this, recreate the container (`up -d --build --no-deps postfix`) to get the port. Users `alice` and `bob` with Dovecot's password (`alice` or `alice@nebulamail.test` both log in). Mail to `alice@` and `bob@nebulamail.test` goes into Dovecot's users `alice` and `bob`, the same mailboxes as the Dovecot tests; other addresses there are refused. Postfix does the final delivery itself (its `pipe` agent runs `msmtp`, which hands the message to Dovecot's LMTP), so a success report (`NOTIFY=SUCCESS`) says `delivered`, as a real mailbox server's would; with Postfix's LMTP client it would say `relayed`, because Dovecot's LMTP has no DSN. Mail to any address `@bounce.nebulamail.test` is accepted and then bounced, so a real bounce (DSN) comes back to the sender. Nothing goes to Stalwart, so its send limit doesn't matter here.

Tests: `NEBULA_TEST_IMAP=1 NEBULA_TEST_SMTP=1 NEBULA_TEST_SERVER=postfix cargo test -p nebula-smtp -p nebula-jmap --test server -- --test-threads=1` in `bridge/` (IMAP is Dovecot's, submission Postfix's). `own_envid_matches_real_reports` (`jmap`) only runs here. Postfix's log: `docker compose -f docker/compose.yaml logs postfix`. Mail to `nobody@bounce.nebulamail.test` doesn't bounce (Postfix's aliases send `nobody` to root): use another name, like `nobody-123@`.

## CalDAV/CardDAV (bridge tests)

The bridge's DAV client is tested against two servers:

- **Stalwart**, which already serves CalDAV and CardDAV on its HTTP port: `http://127.0.0.1:8080/` (discovery through `/.well-known/caldav` → `/dav/cal`, `/.well-known/carddav` → `/dav/card`), with alice's Stalwart login. It announces `calendar-auto-schedule`.
- **Radicale** 3.8 (`ghcr.io/kozea/radicale`, config in `docker/radicale`), in the `bridge` profile: no JMAP and no scheduling, like most small DAV servers.

```sh
docker compose -f docker/compose.yaml --profile bridge up -d --build radicale
```

Radicale listens on `http://127.0.0.1:5232/` (plain HTTP, loopback only). Users `alice` and `bob` with Dovecot's password (`password`, or `TEST_DOVECOT_PASSWORD`), written by `radicale/entrypoint.sh` when the container starts; a new user has no calendars until a client creates one. Data in the `radicale-data` volume.

Radicale, Baïkal and Nextcloud get their config files built into a small image of their own (`radicale/Dockerfile`, `baikal/Dockerfile`, `nextcloud/Dockerfile`), not bind-mounted. A bind-mounted file ties the container to the checkout that created it: once that checkout (a git worktree) is deleted, Docker Desktop on WSL2 turns the missing paths into empty directories when the container starts again, and it fails with «not a directory». Start them with `--build` (as below) so a change to those files reaches the container. Dovecot still mounts `dovecot/*.conf`: create its container from a checkout that stays (the main one), or recreate it if the start fails like that.

Tests: `NEBULA_TEST_DAV=1 NEBULA_TEST_SERVER=stalwart cargo test -p nebula-dav -p nebula-groupware --test server -- --test-threads=1` in `bridge/` (or `NEBULA_TEST_SERVER=radicale`). They discover alice's homes, create scratch calendars and address books with unique names, write, read, sync, move and delete in them, and delete them at the end; the `nebula-groupware` one checks that the bridge's sync sees what another client writes there (new state, push event, sealed copy). The hostile-server tests (`crates/dav/tests/hostile.rs`) and the fake-server sync tests (`crates/groupware/tests/sync.rs`) need no server.

`bridge/dev/dovecot.toml` uses Radicale for calendars and contacts, `bridge/dev/dev.toml` uses Stalwart's DAV.

**Baïkal** 0.10.1 (sabre/dav 4.7; `ckulka/baikal:0.10.1-nginx`, pinned by digest; files in `docker/baikal`), in the `bridge` profile, only for the sharing tests (Calendar Server sharing):

```sh
docker compose -f docker/compose.yaml --profile bridge up -d --build baikal
```

It listens on `http://127.0.0.1:5233/dav.php/` (plain HTTP, loopback only). Users `alice` and `bob` with Dovecot's password, created in its SQLite database by `baikal/init-db.sh` the first time; its admin interface has a password nobody knows and it never sends mail (empty `invite_from`). Data in the `baikal-data` volume. Test: `NEBULA_TEST_BAIKAL=1 cargo test -p nebula-jmap --test server sharing` in `bridge/`, with Dovecot running (the bridge logs in by IMAP). Stop it afterwards: `docker compose -f docker/compose.yaml --profile bridge stop baikal`.

**Nextcloud** 34 (`nextcloud:34.0.5-apache`, pinned by digest; files in `docker/nextcloud`), in the `bridge` profile, for the bridge's DAV tests against Nextcloud. The image is big (about 2 GB unpacked) and the first start installs Nextcloud (about a minute, more on a busy machine). It runs on **PostgreSQL** 17 (`postgres:17.11-alpine`, pinned by digest; service `nextcloud-db`, no port on the host), as real installs do: with SQLite two writes at once to the same event sometimes gave 500 «database is locked». `up -d nextcloud` starts the database first and waits for its health check. CI runs the tests below after Baïkal's (step «DAV integration tests (Nextcloud)», also in `scripts/ci-local.sh`).

```sh
docker compose -f docker/compose.yaml --profile bridge up -d --build nextcloud dovecot
until curl -sf http://127.0.0.1:5234/status.php | grep -q '"installed":true'; do sleep 2; done
cd bridge
NEBULA_TEST_DAV=1 NEBULA_TEST_SERVER=nextcloud \
  cargo test --locked -p nebula-dav -p nebula-groupware -p nebula-jmap --test server -- --test-threads=1
docker compose -f docker/compose.yaml --profile bridge stop nextcloud nextcloud-db
```

It listens on `http://127.0.0.1:5234/` (plain HTTP, loopback only; DAV root `/remote.php/dav/`, `.well-known` redirects there). Users `alice` and `bob` (addresses `@nebulamail.test`) with Dovecot's password, and the group `ventas` («Ventas», with bob; for `alice_shares_with_a_group`), created by `nextcloud/post-install.sh` right after the install (on an install made before the group: `docker exec -u www-data nebula-dev-nextcloud-1 php occ group:add --display-name=Ventas ventas`, then `… occ group:adduser ventas bob`), which also turns off the password policy (it would refuse `password`), the app store, update checks, brute-force throttling (tests log in many times) and every mail (`mail_smtpmode = null`, `dav sendInvitations = no`): Nextcloud delivers invitations between its own users into their calendars, and nothing ever leaves the container. The installing admin (`setup`) is disabled. Files in the `nextcloud-pg-data` volume and the database in `nextcloud-db-data` (the old `nextcloud-data`, from the SQLite install, is no longer used: `docker volume rm nebula-dev_nextcloud-data`). To start over: `docker compose -f docker/compose.yaml --profile bridge rm -sfv nextcloud nextcloud-db`, then `docker volume rm nebula-dev_nextcloud-pg-data nebula-dev_nextcloud-db-data`. The bridge logs in by IMAP to Dovecot with the same password. Each test is slow on Nextcloud (a request takes about a second): the whole run takes a while.

## Delivery status and read receipts in the browser

Browser tests of the app (production build, real sign-in) for the delivery icons in Sent and read receipts. They send real mail: run them once, not in a loop.

Against Stalwart, after `bun run dev:setup` (the app runs on port 3001, the only redirect `dev:setup` registers; a `bun run dev` already on 3001 is used as is). 4 messages per run, one of them from alice to bob:

```sh
cd apps/web
set -a; . ../../docker/.env; set +a
MAIL_LIVE_URL=http://localhost:8080 \
MAIL_LIVE_ALICE_PASSWORD="$TEST_ALICE_PASSWORD" MAIL_LIVE_BOB_PASSWORD="$TEST_BOB_PASSWORD" \
bun run test:live
```

Through the bridge, with Dovecot and Postfix running and the bridge built (`cargo build -p nebula-bridge` in `bridge/`). The bridge sends through Postfix, so bounces and receipts come back to Dovecot; 6 messages, none through Stalwart:

```sh
cd apps/web
E2E_BRIDGE_SMTP=postfix bun run test:bridge -- receipts.e2e.ts
```

Without `E2E_BRIDGE_SMTP=postfix`, `test:bridge` skips that file. Screenshots for a look by eye: `apps/web/test-results/live-*.png`.

## Calendar and contacts in the browser, through the bridge

The web app against the bridge with Dovecot and Radicale: `e2e-bridge/contacts.e2e.ts` (list, search, create, edit, delete, groups, importing a `.vcf`, suggestions in the composer, a card another client writes) and `e2e-bridge/calendar.e2e.ts` (views, create, edit, drag, delete, a daily series with "only this one" and one deleted day, one day of an all-day series, reminders while the app is open, and an invitation from alice to bob that he accepts in the mail). Each test checks Radicale itself, and deletes what it made at the end. With Dovecot, Radicale and Postfix running and the bridge built:

```sh
docker compose -f docker/compose.yaml --profile bridge up -d --build dovecot radicale postfix
cd apps/web
bun run test:bridge -- contacts.e2e.ts calendar.e2e.ts
# The invitation (and receipts.e2e.ts): mail through Postfix to Dovecot.
E2E_BRIDGE_SMTP=postfix bun run test:bridge -- --grep @postfix
```

The tests that send mail through Postfix are tagged `@postfix` and skip themselves without `E2E_BRIDGE_SMTP=postfix`; nothing goes through Stalwart. The reminders test waits a minute for the bridge to send them. A new Radicale user starts without collections: the bridge makes a "Calendar" and a "Contacts" at sign-in.
