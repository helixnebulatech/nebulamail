// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Starts the bridge for the browser tests: a fresh database and master key
// in a temporary folder, Dovecot as the mail server (sending through
// Dovecot or Postfix, see SMTP in env.ts) and Radicale for calendars and
// contacts (DAV in env.ts). Playwright runs this
// as a web server and stops it at the end. The binary must be built first
// (`cargo build -p nebula-bridge` in bridge/), or set BRIDGE_BIN.

import { execFileSync, spawn } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  BRIDGE_PORT,
  BRIDGE_URL,
  CLIENT_ID,
  CLIENT_SECRET_HASH,
  DAV,
  DOVECOT,
  DOVECOT_IMAP_PORT,
  REV2_PROXY,
  REV2_PROXY_PORT,
  SMTP,
  WEB_ENV,
} from "./env";
import { startRev2Proxy } from "./rev2-proxy";

const binary =
  process.env.BRIDGE_BIN ??
  fileURLToPath(
    new URL("../../../bridge/target/debug/nebula-bridge", import.meta.url)
  );

const dir = mkdtempSync(path.join(tmpdir(), "nebula-bridge-e2e-"));

const masterKey = execFileSync(binary, ["gen-master-key"], {
  encoding: "utf-8",
}).trim();

const config = `
[server]
listen = "127.0.0.1:${BRIDGE_PORT}"
public_url = "${BRIDGE_URL}"
app_origins = ["${WEB_ENV.APP_URL}"]

[store]
path = "bridge.db"

[[oauth.clients]]
id = "${CLIENT_ID}"
secret_hash = "${CLIENT_SECRET_HASH}"
redirect_uris = ["${WEB_ENV.APP_URL}/auth/callback"]

[upstream]
private_network = true
sent_copy = "append"
email_domain = "nebulamail.test"

[upstream.imap]
host = "127.0.0.1"
port = ${DOVECOT.imapPort}
tls = "implicit"
tls_verify = false

[upstream.smtp]
host = "127.0.0.1"
port = ${SMTP.port}
tls = "implicit"
tls_verify = false

# Radicale has no scheduling: the bridge sends invitations by SMTP.
# The shortest sync interval, so that what another session wrote shows soon.
[upstream.dav]
url = "${DAV.url}"
poll_seconds = 10

# Filters and vacation response through Dovecot's ManageSieve.
[upstream.sieve]
port = ${DOVECOT.sievePort}

# Scheduled mail may go out after sign-out (schedule.e2e.ts).
[submission]
send_while_signed_out = true

# Web Push (web-push.e2e.ts): only the fake push service on loopback.
[push]
contact = "mailto:admin@nebulamail.test"
allowed_hosts = []
loopback = true
`;
const configPath = path.join(dir, "bridge.toml");
writeFileSync(configPath, config);

// An IMAP4rev2-only Dovecot (rev2-proxy.ts), with E2E_BRIDGE_REV2_PROXY=1.
const rev2Proxy = REV2_PROXY
  ? await startRev2Proxy({
      port: REV2_PROXY_PORT,
      upstreamPort: DOVECOT_IMAP_PORT,
    })
  : undefined;

const bridge = spawn(binary, ["serve", "--config", configPath], {
  env: { ...process.env, NEBULA_BRIDGE_MASTER_KEY: masterKey },
  stdio: "inherit",
});

const stop = () => {
  bridge.kill();
};
process.on("SIGINT", stop);
process.on("SIGTERM", stop);

bridge.on("exit", (code) => {
  rmSync(dir, { force: true, recursive: true });
  void rev2Proxy?.close();
  process.exitCode = code ?? 1;
});
