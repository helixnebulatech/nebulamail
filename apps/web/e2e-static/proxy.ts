// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The browser's only way out in `test:static` (playwright.static.config.ts):
// an HTTP proxy that forwards to the throwaway Stalwart and nothing else,
// and keeps a list of every request. Playwright doesn't see a
// SharedWorker's requests (the session worker's token exchange); the proxy
// sees all of them, from pages and every kind of worker. Anything for
// another origin is refused (403, or a closed socket for https) and listed.
//
// The test reads the list with a plain GET of `/requests` on the proxy
// itself (not a proxied URL), and empties it with `DELETE /requests`.

import { once } from "node:events";
import { createServer, request as forward } from "node:http";
import type { IncomingMessage, Server, ServerResponse } from "node:http";

import { z } from "zod";

import { STALWART_URL } from "./env";

export const PROXY_PORT = Number(process.env.E2E_STATIC_PROXY_PORT ?? "8096");
export const PROXY_URL = `http://127.0.0.1:${PROXY_PORT}`;

export const SeenSchema = z.object({
  /**
   * Only for POSTs to Stalwart's token and revocation endpoints (the PKCE
   * exchange, refreshes, the revocation at sign-out).
   */
  body: z.string().optional(),
  method: z.string(),
  /** Absolute URL; `https://host:port` for a CONNECT. */
  url: z.string(),
});
export type Seen = z.infer<typeof SeenSchema>;

const target = new URL(STALWART_URL);
const BODY_PATHS = new Set(["/auth/token", "/auth/revoke"]);

const readBody = async (incoming: IncomingMessage): Promise<Buffer> => {
  const chunks: Buffer[] = [];
  for await (const chunk of incoming) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(String(chunk)));
  }
  return Buffer.concat(chunks);
};

const control = (
  seen: Seen[],
  incoming: IncomingMessage,
  outgoing: ServerResponse
): void => {
  if (incoming.method === "DELETE") {
    seen.length = 0;
    outgoing.writeHead(204).end();
    return;
  }
  outgoing
    .writeHead(200, { "content-type": "application/json" })
    .end(JSON.stringify(seen));
};

const proxy = async (
  seen: Seen[],
  incoming: IncomingMessage,
  outgoing: ServerResponse
): Promise<void> => {
  const url = new URL(incoming.url ?? "", target);
  const method = incoming.method ?? "GET";
  const body = await readBody(incoming);
  seen.push({
    method,
    url: url.href,
    ...(BODY_PATHS.has(url.pathname) && method === "POST"
      ? { body: body.toString("utf-8") }
      : {}),
  });
  if (url.origin !== target.origin) {
    outgoing.writeHead(403).end();
    return;
  }
  const upstream = forward(
    {
      headers: incoming.headers,
      host: "127.0.0.1",
      method,
      path: `${url.pathname}${url.search}`,
      port: target.port,
    },
    (response) => {
      outgoing.writeHead(response.statusCode ?? 502, response.headers);
      response.pipe(outgoing);
    }
  );
  upstream.on("error", () => {
    outgoing.destroy();
  });
  // The event source never ends: stop forwarding when the browser leaves.
  outgoing.on("close", () => {
    upstream.destroy();
  });
  upstream.end(body);
};

/** Starts the proxy; resolves once it listens. */
export const startProxy = async (): Promise<Server> => {
  const seen: Seen[] = [];
  const server = createServer((incoming, outgoing) => {
    // A request for the proxy itself has a path, not an absolute URL
    // (browsers always send one to a proxy). Not listed.
    if (!(incoming.url ?? "").startsWith("http://")) {
      if (incoming.url === "/requests") {
        control(seen, incoming, outgoing);
      } else {
        outgoing.writeHead(404).end();
      }
      return;
    }
    proxy(seen, incoming, outgoing).catch(() => {
      outgoing.destroy();
    });
  });
  server.on("connect", (incoming: IncomingMessage, socket) => {
    seen.push({ method: "CONNECT", url: `https://${incoming.url ?? ""}` });
    socket.destroy();
  });
  server.listen(PROXY_PORT, "127.0.0.1");
  await once(server, "listening");
  return server;
};
