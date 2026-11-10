// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// An IMAP server that speaks only IMAP4rev2 (RFC 9051), for testing the
// bridge live against one: no real server announces
// `IMAP4rev2` without `IMAP4rev1`. A TLS proxy in front of the test
// Dovecot (which supports both) that:
//
// - removes `IMAP4rev1` from every capability list it forwards (greeting,
//   `* CAPABILITY`, `[CAPABILITY …]` codes), so the bridge sees a
//   rev2-only server (RFC 9051 Appendix E);
// - right after a successful login sends `ENABLE IMAP4rev2` (RFC 9051
//   §6.3.1) to Dovecot on its own and swallows the answer, so Dovecot
//   really behaves as rev2 (UTF-8 mailbox names, LIST in SELECT, no
//   `\Recent`…). The client sees the login's OK only after that.
//
// Literals (`{n}`, `{n+}`, `~{n}`) pass through untouched in both
// directions. Two ways to use it:
//
//   bun e2e-bridge/rev2-proxy.ts   # on REV2_PROXY_PORT (2997); REV2_PROXY_LOG=1
//                                  # prints the commands (not logins)
//   NEBULA_TEST_IMAP=1 NEBULA_TEST_SERVER=dovecot \
//     NEBULA_TEST_IMAP_PORT=2997 cargo test --test server   # in bridge/
//
// or `E2E_BRIDGE_REV2_PROXY=1 bun run test:bridge`, which starts it and
// points the bridge at it (e2e-bridge/env.ts).

import { once } from "node:events";
import type { Server } from "node:net";
import { connect, createServer } from "node:tls";
import type { TLSSocket } from "node:tls";

import { selfSigned } from "./alert-proxy";

export interface Rev2Proxy {
  close: () => Promise<void>;
}

export interface Rev2ProxyOptions {
  /** Where Dovecot's implicit-TLS IMAP listens. */
  upstreamPort: number;
  /** Where the proxy listens. */
  port: number;
  /** Print each command's first line to stderr (never a login's). */
  log?: boolean;
}

const LOOPBACK = ["127", "0", "0", "1"].join(".");

/** The tag of the proxy's own `ENABLE IMAP4rev2`. */
const ENABLE_TAG = "rev2proxy";

/** A literal at the end of a line: `{n}`, `{n+}`, `{n-}` or `~{n}`. */
const LITERAL = /\{(?<size>\d+)[+-]?\}$/u;

/** `* CAPABILITY …` (RFC 9051 §7.2.2). */
const CAPABILITY_LINE = /^(?<head>\* CAPABILITY )(?<list>.*)$/iu;

/** A `[CAPABILITY …]` response code (RFC 9051 §7.1). */
const CAPABILITY_CODE =
  /^(?<head>\S+ (?:OK|PREAUTH|NO|BAD|BYE) \[CAPABILITY )(?<list>[^\]]*)(?<tail>\].*)$/iu;

const withoutRev1 = (list: string): string =>
  list
    .split(" ")
    .filter((atom) => atom.toUpperCase() !== "IMAP4REV1")
    .join(" ");

/** A server line with `IMAP4rev1` taken out of its capability list. */
export const rewriteCapabilities = (line: string): string => {
  const code = CAPABILITY_CODE.exec(line)?.groups;
  if (code !== undefined) {
    return `${code.head}${withoutRev1(code.list ?? "")}${code.tail}`;
  }
  const untagged = CAPABILITY_LINE.exec(line)?.groups;
  if (untagged !== undefined) {
    return `${untagged.head}${withoutRev1(untagged.list ?? "")}`;
  }
  return line;
};

/** What a stream is made of: lines (without CRLF) and literal bytes. */
type Unit =
  | { kind: "line"; text: string; commandStart: boolean }
  | { kind: "raw"; text: string };

/**
 * Splits a latin1 stream (bytes as chars, lossless) into lines and the
 * literals that follow them. `commandStart` is false for a line that
 * continues one ended by a literal.
 */
const framer = (emit: (unit: Unit) => void): ((chunk: string) => void) => {
  let buffer = "";
  let literalLeft = 0;
  let commandStart = true;
  return (chunk) => {
    buffer += chunk;
    for (;;) {
      if (literalLeft > 0) {
        if (buffer === "") {
          return;
        }
        const raw = buffer.slice(0, literalLeft);
        buffer = buffer.slice(raw.length);
        literalLeft -= raw.length;
        emit({ kind: "raw", text: raw });
        continue;
      }
      const end = buffer.indexOf("\r\n");
      if (end === -1) {
        return;
      }
      const text = buffer.slice(0, end);
      buffer = buffer.slice(end + 2);
      emit({ commandStart, kind: "line", text });
      const size = LITERAL.exec(text)?.groups?.size;
      literalLeft = size === undefined ? 0 : Number(size);
      commandStart = size === undefined;
    }
  };
};

/** The tag of a LOGIN or AUTHENTICATE command line, if it is one. */
const loginTag = (line: string): string | null => {
  const [tag, command] = line.split(" ", 2);
  const name = command?.toUpperCase();
  return tag !== undefined && (name === "LOGIN" || name === "AUTHENTICATE")
    ? tag
    : null;
};

const send = (socket: TLSSocket, text: string): void => {
  socket.write(Buffer.from(text, "latin1"));
};

const handle = (
  client: TLSSocket,
  options: Rev2ProxyOptions,
  open: Set<TLSSocket>
): void => {
  const upstream = connect({
    host: LOOPBACK,
    port: options.upstreamPort,
    rejectUnauthorized: false,
  });
  open.add(client);
  open.add(upstream);
  /** A LOGIN/AUTHENTICATE whose command line(s) are still coming. */
  let loginCommand: string | undefined;
  /**
   * From a complete login command to its answer and the ENABLE: the
   * client's next commands wait in `queued` (a pipelining client must not
   * reach Dovecot before rev2 is on). Only replies to a `+` go through.
   */
  let pendingLogin: string | undefined;
  /** Our ENABLE runs; the login's OK waits for it. */
  let heldLoginOk: string | undefined;
  let continuation = false;
  let queued = "";

  const flush = () => {
    if (queued !== "") {
      send(upstream, queued);
      queued = "";
    }
  };

  const fromClient = framer((unit) => {
    const text = unit.kind === "line" ? `${unit.text}\r\n` : unit.text;
    if (
      options.log === true &&
      unit.kind === "line" &&
      unit.commandStart &&
      !continuation &&
      loginTag(unit.text) === null
    ) {
      process.stderr.write(`rev2-proxy C: ${unit.text.slice(0, 200)}\n`);
    }
    if (pendingLogin !== undefined || heldLoginOk !== undefined) {
      if (unit.kind === "line" && continuation) {
        // An AUTHENTICATE exchange (RFC 9051 §6.2.2).
        continuation = false;
        send(upstream, text);
      } else {
        queued += text;
      }
      return;
    }
    send(upstream, text);
    if (unit.kind !== "line") {
      return;
    }
    if (unit.commandStart) {
      loginCommand = loginTag(unit.text) ?? undefined;
    }
    if (loginCommand !== undefined && !LITERAL.test(unit.text)) {
      pendingLogin = loginCommand;
      loginCommand = undefined;
    }
  });

  const fromServer = framer((unit) => {
    if (unit.kind === "raw") {
      send(client, unit.text);
      return;
    }
    const line = unit.text;
    const [tag = "", status = ""] = line.split(" ", 2);
    if (heldLoginOk !== undefined) {
      if (tag === "*" && status.toUpperCase() === "ENABLED") {
        return;
      }
      if (tag === ENABLE_TAG) {
        if (status.toUpperCase() !== "OK") {
          // Not what this proxy is for: say so and stop.
          send(client, "* BYE rev2-proxy: ENABLE IMAP4rev2 failed\r\n");
          client.end();
          return;
        }
        send(client, `${heldLoginOk}\r\n`);
        heldLoginOk = undefined;
        flush();
        return;
      }
    }
    const rewritten = rewriteCapabilities(line);
    if (pendingLogin !== undefined && unit.commandStart) {
      if (tag === "+") {
        continuation = true;
      } else if (tag === pendingLogin) {
        pendingLogin = undefined;
        continuation = false;
        if (status.toUpperCase() === "OK") {
          heldLoginOk = rewritten;
          send(upstream, `${ENABLE_TAG} ENABLE IMAP4rev2\r\n`);
          return;
        }
        send(client, `${rewritten}\r\n`);
        flush();
        return;
      }
    }
    send(client, `${rewritten}\r\n`);
  });

  client.setEncoding("latin1");
  upstream.setEncoding("latin1");
  client.on("data", fromClient);
  upstream.on("data", fromServer);
  const fail = () => {
    client.destroy();
    upstream.destroy();
  };
  const closed = (socket: TLSSocket, other: TLSSocket) => () => {
    open.delete(socket);
    other.end();
  };
  // A side that closes ends the other gracefully (TLS close_notify): a
  // destroy with the peer's last bytes unread would reset its connection
  // (a client's LOGOUT racing Dovecot's close).
  client.on("end", () => {
    upstream.end();
  });
  upstream.on("end", () => {
    client.end();
  });
  client.on("close", closed(client, upstream));
  upstream.on("close", closed(upstream, client));
  client.on("error", fail);
  upstream.on("error", fail);
};

/** Starts the proxy; resolves once it listens. */
export const startRev2Proxy = async (
  options: Rev2ProxyOptions
): Promise<Rev2Proxy> => {
  const open = new Set<TLSSocket>();
  const server: Server = createServer(selfSigned(), (client) => {
    handle(client, options, open);
  });
  server.listen(options.port, LOOPBACK);
  // Rejects on "error" (a port in use).
  await once(server, "listening");
  return {
    close: async () => {
      for (const socket of open) {
        socket.destroy();
      }
      const closed = once(server, "close");
      server.close();
      await closed;
    },
  };
};

// Run directly: listen until stopped.
if (process.argv[1] === import.meta.filename) {
  const port = Number(process.env.REV2_PROXY_PORT ?? "2997");
  const upstreamPort = Number(process.env.REV2_PROXY_UPSTREAM_PORT ?? "2993");
  await startRev2Proxy({
    log: process.env.REV2_PROXY_LOG === "1",
    port,
    upstreamPort,
  });
  process.stdout.write(
    `rev2-proxy: IMAP4rev2-only on ${LOOPBACK}:${port} -> ${upstreamPort}\n`
  );
}
