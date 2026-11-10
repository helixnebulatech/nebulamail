// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Puts a message in a Dovecot mailbox from outside the bridge (IMAP
// APPEND over TLS), as another client or a delivery would: the bridge
// only learns of it by syncing, so it is new mail to it (EmailPush), not
// one of its own writes. Test helper: Dovecot's certificate of
// docker/compose.yaml is self-signed, on loopback only.

import { once } from "node:events";
import { connect } from "node:tls";

import { DOVECOT } from "./env";

const HOST = "127.0.0.1";

const quoted = (text: string): string =>
  `"${text.replaceAll("\\", "\\\\").replaceAll('"', '\\"')}"`;

/** APPENDs `raw` to `mailbox` of `user`; rejects if the server says no. */
export const imapAppend = async (
  user: string,
  raw: string,
  mailbox = "INBOX"
): Promise<void> => {
  const message = Buffer.from(raw.replaceAll(/\r?\n/gu, "\r\n"), "utf-8");
  const socket = connect({
    host: HOST,
    port: DOVECOT.imapPort,
    // Dovecot's test certificate, on loopback.
    rejectUnauthorized: false,
  });
  let buffer = "";
  socket.setEncoding("utf-8");
  socket.on("data", (chunk: string) => {
    buffer += chunk;
  });
  const until = async (pattern: RegExp): Promise<string> => {
    const deadline = Date.now() + 15_000;
    while (!pattern.test(buffer)) {
      if (Date.now() > deadline) {
        throw new Error(`IMAP: no ${pattern.source} in ${buffer}`);
      }
      // oxlint-disable-next-line no-await-in-loop -- waiting for the next chunk, in order.
      await once(socket, "data", { signal: AbortSignal.timeout(500) }).catch(
        () => null
      );
    }
    return buffer;
  };
  try {
    await until(/^\* OK/mu);
    socket.write(`a1 LOGIN ${quoted(user)} ${quoted(DOVECOT.password)}\r\n`);
    await until(/^a1 (?:OK|NO|BAD)/mu);
    if (!/^a1 OK/mu.test(buffer)) {
      throw new Error("IMAP login refused");
    }
    socket.write(`a2 APPEND ${quoted(mailbox)} {${message.length}}\r\n`);
    await until(/^\+/mu);
    socket.write(message);
    socket.write("\r\n");
    await until(/^a2 (?:OK|NO|BAD)/mu);
    if (!/^a2 OK/mu.test(buffer)) {
      throw new Error("IMAP APPEND refused");
    }
    socket.write("a3 LOGOUT\r\n");
  } finally {
    socket.end();
  }
};
