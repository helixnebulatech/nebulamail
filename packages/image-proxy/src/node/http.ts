// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// node:http ↔ Request/Response for the core handler.

import type { IncomingMessage, ServerResponse } from "node:http";

import type { RequestContext } from "../config";

export type WebHandler = (
  request: Request,
  context?: RequestContext
) => Promise<Response>;

/**
 * The Host header is attacker-controlled and the core only routes on the
 * path, so requests get a fixed placeholder origin.
 */
const BASE = "http://image-proxy.invalid";

/**
 * After the response, what is left of a body the handler did not read (it
 * was too large, or not needed) is read and thrown away, so the client gets
 * the response instead of a reset connection. Stalwart's MTA Hook, for
 * example, only reads the answer once it has sent the whole email. Past
 * these limits the socket is destroyed.
 */
export const DRAIN_MAX_BYTES = 32 * 1024 * 1024;
export const DRAIN_MAX_MS = 10_000;

/** Whether the request body has been received in full. */
interface BodyState {
  ended: boolean;
}

/**
 * The request body as a web stream, from `data`/`end` events (not the async
 * iterator: its `return()` destroys the request, and the socket with it,
 * before the response is out). `cancel` stops listening and destroys
 * nothing; the listener drains the rest. (`Readable.toWeb` returns Node's
 * own `ReadableStream` type, which TypeScript does not accept as a DOM
 * `BodyInit`; the bytes are the same.)
 */
const bodyStream = (req: IncomingMessage): ReadableStream<Uint8Array> => {
  const queued: Uint8Array[] = [];
  let done = false;
  let failure: Error | undefined;
  let wake: (() => void) | undefined;
  let listening = false;
  const notify = (): void => {
    const resolve = wake;
    wake = undefined;
    resolve?.();
  };
  const onData = (chunk: unknown): void => {
    if (chunk instanceof Uint8Array) {
      queued.push(chunk);
    } else {
      failure = new TypeError("unexpected request chunk");
    }
    req.pause();
    notify();
  };
  const onEnd = (): void => {
    done = true;
    notify();
  };
  const onError = (): void => {
    failure = new Error("request body failed");
    notify();
  };
  const onClose = (): void => {
    if (!done) {
      failure ??= new Error("request closed");
    }
    notify();
  };
  const nextEvent = async (): Promise<void> => {
    const { promise, resolve } = Promise.withResolvers<undefined>();
    wake = () => {
      resolve(undefined);
    };
    await promise;
  };
  const stop = (): void => {
    req.off("data", onData);
    req.off("end", onEnd);
    req.off("error", onError);
    req.off("close", onClose);
  };
  return new ReadableStream<Uint8Array>({
    cancel: () => {
      stop();
      req.pause();
      notify();
    },
    pull: async (controller) => {
      if (!listening) {
        listening = true;
        req.on("data", onData);
        req.on("end", onEnd);
        req.on("error", onError);
        req.on("close", onClose);
      }
      for (;;) {
        const chunk = queued.shift();
        if (chunk !== undefined) {
          controller.enqueue(chunk);
          return;
        }
        if (failure !== undefined) {
          stop();
          controller.error(failure);
          return;
        }
        if (done) {
          stop();
          controller.close();
          return;
        }
        const next = nextEvent();
        req.resume();
        // One chunk at a time, in order.
        // oxlint-disable-next-line no-await-in-loop
        await next;
      }
    },
  });
};

/**
 * Read and discard the rest of the body (bounded). Without it the socket
 * would close with unread data and the client would see a reset instead of
 * the response.
 */
const drain = (req: IncomingMessage): void => {
  let read = 0;
  const timer = setTimeout(() => {
    req.socket.destroy();
  }, DRAIN_MAX_MS);
  timer.unref();
  const count = (chunk: unknown): void => {
    read += chunk instanceof Uint8Array ? chunk.byteLength : 0;
    if (read > DRAIN_MAX_BYTES) {
      // `close` follows and runs `finish`.
      req.socket.destroy();
    }
  };
  const finish = (): void => {
    clearTimeout(timer);
    req.off("data", count);
  };
  req.on("data", count);
  req.once("end", finish);
  req.once("close", finish);
  req.once("error", finish);
  req.resume();
};

const toRequest = (req: IncomingMessage, signal: AbortSignal): Request => {
  const headers = new Headers();
  for (const [name, value] of Object.entries(req.headers)) {
    if (value !== undefined && !name.startsWith(":")) {
      headers.set(name, Array.isArray(value) ? value.join(", ") : value);
    }
  }
  const url = new URL(req.url ?? "/", BASE);
  const method = req.method ?? "GET";
  const hasBody = method === "POST";
  return new Request(`${BASE}${url.pathname}${url.search}`, {
    headers,
    method,
    signal,
    ...(hasBody
      ? {
          body: bodyStream(req),
          duplex: "half",
        }
      : {}),
  });
};

const INTERNAL_ERROR = JSON.stringify({ error: "internal" });

/** A `node:http` request listener around a core handler. */
export const toNodeListener =
  (
    handler: WebHandler,
    clientAddress: (req: IncomingMessage) => string | undefined
  ) =>
  (req: IncomingMessage, res: ServerResponse): void => {
    const abort = new AbortController();
    const body: BodyState = { ended: false };
    req.once("end", () => {
      body.ended = true;
    });
    res.on("close", () => {
      if (!res.writableFinished) {
        abort.abort();
      }
    });
    const run = async (): Promise<void> => {
      let response: Response;
      try {
        const address = clientAddress(req);
        response = await handler(
          toRequest(req, abort.signal),
          address === undefined ? {} : { clientAddress: address }
        );
      } catch {
        // Never echo the error: it could contain a URL.
        response = new Response(INTERNAL_ERROR, {
          headers: {
            "Cache-Control": "no-store",
            "Content-Type": "application/json; charset=utf-8",
            "X-Content-Type-Options": "nosniff",
          },
          status: 500,
        });
      }
      const bytes = Buffer.from(await response.arrayBuffer());
      const headers: Record<string, string> = {};
      for (const [name, value] of response.headers) {
        headers[name] = value;
      }
      headers["content-length"] = String(bytes.length);
      // `complete`: the whole request arrived (maybe still buffered).
      if (!body.ended && !req.complete) {
        drain(req);
      }
      res.writeHead(response.status, headers);
      res.end(bytes);
    };
    run().catch(() => {
      res.destroy();
    });
  };
