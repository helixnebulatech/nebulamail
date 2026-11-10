<!-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# Preload relay: deployment files

Optional. Only if you turn preload on (`IMAGE_PRELOAD_SECRET` in the proxy) and want the image proxy **not** to see your mail. Read "Preload" in `../../README.md` first, risks included.

| File | What |
| --- | --- |
| `relay.env.example` | The relay's variables. Copy to `/etc/nebula-preload-relay/relay.env` (`root:nebula-relay`, `0640`) |
| `nebula-preload-relay.service` | systemd unit: own user, no capabilities, read-only system, syscall filter |
| `nebula-preload-relay.openrc` | The same for OpenRC |
| `stalwart-mta-hook.ndjson` | The Stalwart hook pointing at the relay (`stalwart-cli apply --file …`), only for incoming mail. Stalwart reads the token from its environment (`IMAGE_PRELOAD_RELAY_SECRET`); then `x:Action` `ReloadSettings` |

Steps:

1. `useradd --system --no-create-home --shell /usr/sbin/nologin nebula-relay`
2. The repository at `/opt/nebulamail` with `bun install` (or change the paths).
3. Fill in `relay.env`: two different secrets (`openssl rand -hex 32`), the proxy's URL.
4. Start the service; `curl http://127.0.0.1:3004/healthz` answers `{"ok":true,"relay":true}`.
5. Firewall: the relay's user may only connect to the proxy (e.g. nftables `meta skuid "nebula-relay" ip daddr <proxy> tcp dport 443 accept`, the rest dropped).
6. Stalwart: give it `IMAGE_PRELOAD_RELAY_SECRET` in its environment and apply `stalwart-mta-hook.ndjson`.
