// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Ports and settings of the browser tests against the real bridge
//the production build of the app, the bridge binary and
// the Dovecot of docker/compose.yaml (profile "bridge").

export const WEB_PORT = Number(process.env.E2E_BRIDGE_WEB_PORT ?? "3118");
export const BRIDGE_PORT = Number(process.env.E2E_BRIDGE_PORT ?? "8191");

export const BRIDGE_URL = `http://localhost:${BRIDGE_PORT}`;

// The OAuth client of bridge/dev/*.toml. Its secret is public (it is in the
// repository) and only valid against this test bridge.
export const CLIENT_ID = "nebula-dev";
export const CLIENT_SECRET = "nebula-dev-bridge-secret";
export const CLIENT_SECRET_HASH =
  "$argon2id$v=19$m=19456,t=2,p=1$kiNCtYzYa8B0ksXw383Jzg$/3RUeVgqm5mXuEuRcVRlOMxNERXy6XYOwpKv6jC+Ur0";

export const WEB_ENV = {
  APP_URL: `http://localhost:${WEB_PORT}`,
  JMAP_SERVER: BRIDGE_URL,
  OAUTH_CLIENT_ID: CLIENT_ID,
  OAUTH_CLIENT_SECRET: CLIENT_SECRET,
  // A fixed test key ("e2e-test-key-not-a-secret-123456").
  SESSION_KEYS: "1:ZTJlLXRlc3Qta2V5LW5vdC1hLXNlY3JldC0xMjM0NTY",
} as const;

/** Dovecot's IMAP (implicit TLS) in docker/compose.yaml. */
export const DOVECOT_IMAP_PORT = 2993;

/**
 * With `E2E_BRIDGE_ALERT_PROXY=1` the bridge reaches Dovecot's IMAP
 * through server-alerts.e2e.ts's proxy, which adds `[ALERT]`s; the other
 * tests don't run then (the proxy only listens during that file).
 */
export const ALERT_PROXY = process.env.E2E_BRIDGE_ALERT_PROXY === "1";
export const ALERT_PROXY_PORT = 2996;

/**
 * With `E2E_BRIDGE_REV2_PROXY=1` the bridge reaches Dovecot's IMAP through
 * rev2-proxy.ts, an IMAP4rev2-only server (RFC 9051) that start-bridge.ts
 * starts with the bridge; every test runs against it.
 */
export const REV2_PROXY = process.env.E2E_BRIDGE_REV2_PROXY === "1";
export const REV2_PROXY_PORT = 2997;

const imapPort = (): number => {
  if (ALERT_PROXY) {
    return ALERT_PROXY_PORT;
  }
  return REV2_PROXY ? REV2_PROXY_PORT : DOVECOT_IMAP_PORT;
};

// Dovecot takes any user with one password (docker/compose.yaml).
export const DOVECOT = {
  imapPort: imapPort(),
  password: process.env.TEST_DOVECOT_PASSWORD ?? "password",
  // ManageSieve (Pigeonhole), for filters and the vacation response.
  sievePort: 24_190,
  smtpPort: 2465,
} as const;

/**
 * Calendars and contacts: the Radicale of docker/compose.yaml (profile
 * "bridge"), with Dovecot's users and password and no scheduling (the
 * bridge mails invitations itself). Without Radicale the mail tests still
 * pass: the bridge just doesn't offer calendars or contacts.
 */
export const DAV = {
  url: process.env.E2E_BRIDGE_DAV_URL ?? "http://127.0.0.1:5232/",
} as const;

/**
 * Where the bridge sends: Dovecot's submission (default), which relays to
 * Stalwart, or with `E2E_BRIDGE_SMTP=postfix` the test Postfix, which
 * announces DSN and delivers to Dovecot (docker/README.md). Only with
 * Postfix do bounces, read receipts and calendar invitations come back to
 * Dovecot's mailboxes: the tests that need it (receipts.e2e.ts and the
 * invitation in calendar.e2e.ts) are tagged `@postfix` and skipped without
 * it (`E2E_BRIDGE_SMTP=postfix bun run test:bridge -- --grep @postfix`).
 */
export const SMTP =
  process.env.E2E_BRIDGE_SMTP === "postfix"
    ? ({ name: "postfix", port: 2466 } as const)
    : ({ name: "dovecot", port: DOVECOT.smtpPort } as const);
