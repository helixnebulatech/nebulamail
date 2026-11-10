// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// An IMAP server that sends `[ALERT]`s (RFC 3501 §7.1, RFC 9051 §7.1),
// for server-alerts.e2e.ts: a TLS proxy in front of the test Dovecot
// (which never sends one) that adds them, as a real server would:
//
// - `* OK [ALERT] <text>` before the tagged answer of every SELECT and
//   EXAMINE (a quota warning while the user works);
// - `<tag> NO [ALERT] <text>` to the login of the user `locked` (an
//   account the administrator locked), never forwarded to Dovecot.
//
// Everything else goes through unchanged. The proxy's certificate is a
// throwaway self-signed one made with `openssl` (the bridge's test config
// has `tls_verify = false`).

import { execFileSync } from "node:child_process";
import { once } from "node:events";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import type { Server } from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import { connect, createServer } from "node:tls";
import type { TLSSocket } from "node:tls";

export interface AlertProxy {
  close: () => Promise<void>;
}

const LOOPBACK = ["127", "0", "0", "1"].join(".");

export const selfSigned = (): { cert: Buffer; key: Buffer } => {
  const dir = mkdtempSync(path.join(tmpdir(), "alert-proxy-"));
  try {
    const key = path.join(dir, "key.pem");
    const cert = path.join(dir, "cert.pem");
    execFileSync(
      "openssl",
      [
        "req",
        "-x509",
        "-newkey",
        "ec",
        "-pkeyopt",
        "ec_paramgen_curve:P-256",
        "-nodes",
        "-subj",
        "/CN=localhost",
        "-days",
        "1",
        "-keyout",
        key,
        "-out",
        cert,
      ],
      { stdio: "ignore" }
    );
    return { cert: readFileSync(cert), key: readFileSync(key) };
  } finally {
    rmSync(dir, { force: true, recursive: true });
  }
};

/** The user of `LOGIN <user> <password>`. */
const loginUser = (rest: string): string | null =>
  rest.split(" ")[0]?.replaceAll('"', "") ?? null;

/** The user of an AUTHENTICATE PLAIN response (RFC 4616). */
const plainUser = (base64: string): string | null => {
  // authzid NUL authcid NUL password.
  const parts = Buffer.from(base64, "base64").toString("utf-8").split("\0");
  return parts[1] ?? null;
};

/** A command line: tag, command and the rest. */
const COMMAND = /^(?<tag>\S+) (?<command>\S+)(?: (?<rest>.*))?$/u;

/**
 * What the proxy does with a client's line: `user` for a login (or the
 * tag of an AUTHENTICATE PLAIN whose credentials come next), and whether
 * the tagged answer gets the quota alert.
 */
const classify = (
  line: string
): {
  tag: string;
  user: string | null;
  awaitsCredentials: boolean;
  alert: boolean;
} | null => {
  const groups = COMMAND.exec(line)?.groups;
  const tag = groups?.tag;
  const command = groups?.command?.toUpperCase();
  if (tag === undefined || command === undefined) {
    return null;
  }
  const rest = groups?.rest ?? "";
  if (command === "LOGIN") {
    return {
      alert: false,
      awaitsCredentials: false,
      tag,
      user: loginUser(rest),
    };
  }
  if (command === "AUTHENTICATE") {
    const [mechanism = "", initial = ""] = rest.split(" ");
    const plain = mechanism.toUpperCase() === "PLAIN";
    return {
      alert: false,
      awaitsCredentials: plain && initial === "",
      tag,
      user: plain && initial !== "" ? plainUser(initial) : null,
    };
  }
  return {
    alert: command === "SELECT" || command === "EXAMINE",
    awaitsCredentials: false,
    tag,
    user: null,
  };
};

export interface AlertProxyOptions {
  /** Where Dovecot's implicit-TLS IMAP listens. */
  upstreamPort: number;
  /** Where the proxy listens. */
  port: number;
  /** The text of the alert sent with SELECT and EXAMINE. */
  quotaAlert: string;
  /** The text of the alert that refuses the login of `locked`. */
  lockedAlert: string;
}

const handle = (
  client: TLSSocket,
  options: AlertProxyOptions,
  open: Set<TLSSocket>
): void => {
  const upstream = connect({
    host: LOOPBACK,
    port: options.upstreamPort,
    rejectUnauthorized: false,
  });
  open.add(client);
  open.add(upstream);
  /** Tags of SELECT/EXAMINE whose tagged answer gets the alert before. */
  const alertBefore = new Set<string>();
  /** Tag of an AUTHENTICATE PLAIN waiting for its credentials line. */
  let pendingAuth: string | undefined;
  let fromClient = "";
  let fromServer = "";

  const refuse = (tag: string) => {
    client.write(`${tag} NO [ALERT] ${options.lockedAlert}\r\n`);
  };

  const toUpstream = (text: string) => {
    upstream.write(Buffer.from(text, "latin1"));
  };

  const clientLine = (line: string): void => {
    if (pendingAuth !== undefined) {
      const tag = pendingAuth;
      pendingAuth = undefined;
      if (plainUser(line) === "locked") {
        // Dovecot is waiting for this line: cancel its exchange (RFC 9051
        // §6.2.2) and answer for it.
        toUpstream("*\r\n");
        refuse(tag);
        alertBefore.add(`drop:${tag}`);
        return;
      }
      toUpstream(`${line}\r\n`);
      return;
    }
    const parsed = classify(line);
    if (parsed?.user === "locked") {
      refuse(parsed.tag);
      return;
    }
    if (parsed?.awaitsCredentials === true) {
      pendingAuth = parsed.tag;
    }
    if (parsed?.alert === true) {
      alertBefore.add(parsed.tag);
    }
    toUpstream(`${line}\r\n`);
  };

  const serverLine = (line: string): string => {
    const tag = line.split(" ", 1)[0] ?? "";
    if (alertBefore.delete(`drop:${tag}`)) {
      // Dovecot's answer to the cancelled exchange: already answered.
      return "";
    }
    if (alertBefore.delete(tag)) {
      return `* OK [ALERT] ${options.quotaAlert}\r\n${line}\r\n`;
    }
    return `${line}\r\n`;
  };

  client.setEncoding("latin1");
  upstream.setEncoding("latin1");
  client.on("data", (chunk: string) => {
    fromClient += chunk;
    let end = fromClient.indexOf("\r\n");
    while (end !== -1) {
      clientLine(fromClient.slice(0, end));
      fromClient = fromClient.slice(end + 2);
      end = fromClient.indexOf("\r\n");
    }
  });
  upstream.on("data", (chunk: string) => {
    fromServer += chunk;
    let out = "";
    let end = fromServer.indexOf("\r\n");
    while (end !== -1) {
      out += serverLine(fromServer.slice(0, end));
      fromServer = fromServer.slice(end + 2);
      end = fromServer.indexOf("\r\n");
    }
    if (out !== "") {
      client.write(Buffer.from(out, "latin1"));
    }
  });
  const close = () => {
    open.delete(client);
    open.delete(upstream);
    client.destroy();
    upstream.destroy();
  };
  client.on("close", close);
  client.on("error", close);
  upstream.on("close", close);
  upstream.on("error", close);
};

/** Starts the proxy; resolves once it listens. */
export const startAlertProxy = async (
  options: AlertProxyOptions
): Promise<AlertProxy> => {
  const open = new Set<TLSSocket>();
  const server: Server = createServer(selfSigned(), (client) => {
    handle(client, options, open);
  });
  server.listen(options.port, LOOPBACK);
  // Rejects on "error" (a port in use).
  await once(server, "listening");
  return {
    close: async () => {
      // The bridge keeps its connections (IDLE): end them too.
      for (const socket of open) {
        socket.destroy();
      }
      const closed = once(server, "close");
      server.close();
      await closed;
    },
  };
};
