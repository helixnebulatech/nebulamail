// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// A fake Web Push service for the bridge tests: the
// bridge POSTs to it (the endpoint is on loopback, allowed by
// `[push] loopback = true` in start-bridge.ts), it checks the VAPID header
// and decrypts the message (RFC 8291, aes128gcm) with the browser's
// private key, as the browser would. The test then hands the plaintext to
// the service worker with Chromium's DevTools (what a real push service
// and browser do together).
//
// The browser keys are RFC 8291 §5's example keys: test-only.

import {
  createDecipheriv,
  createECDH,
  hkdfSync,
  createPublicKey,
  verify,
} from "node:crypto";
import { once } from "node:events";
import type { Server } from "node:http";
import { createServer } from "node:http";

const fromB64 = (text: string): Buffer => Buffer.from(text, "base64url");

export const BROWSER_KEYS = {
  auth: "BTBZMqHH6r4Tts7J_aSIgg",
  p256dh:
    "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
  private: "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94",
} as const;

const hkdf = (
  ikm: Buffer,
  salt: Buffer,
  info: string | Buffer,
  length: number
) => Buffer.from(hkdfSync("sha256", ikm, salt, info, length));

/** RFC 8291 decryption of a one-record aes128gcm body. */
export const decrypt = (body: Buffer): unknown => {
  const salt = body.subarray(0, 16);
  const idLength = body.readUInt8(20);
  const asPublic = body.subarray(21, 21 + idLength);
  const record = body.subarray(21 + idLength);
  const ecdh = createECDH("prime256v1");
  ecdh.setPrivateKey(fromB64(BROWSER_KEYS.private));
  const secret = ecdh.computeSecret(asPublic);
  const keyInfo = Buffer.concat([
    Buffer.from("WebPush: info\0"),
    fromB64(BROWSER_KEYS.p256dh),
    asPublic,
  ]);
  const ikm = hkdf(secret, fromB64(BROWSER_KEYS.auth), keyInfo, 32);
  const cek = hkdf(ikm, salt, "Content-Encoding: aes128gcm\0", 16);
  const nonce = hkdf(ikm, salt, "Content-Encoding: nonce\0", 12);
  const decipher = createDecipheriv("aes-128-gcm", cek, nonce);
  decipher.setAuthTag(record.subarray(-16));
  const plain = Buffer.concat([
    decipher.update(record.subarray(0, -16)),
    decipher.final(),
  ]);
  let end = plain.length - 1;
  while (end >= 0 && plain[end] === 0) {
    end -= 1;
  }
  if (plain[end] !== 2) {
    throw new Error("bad padding");
  }
  return JSON.parse(plain.subarray(0, end).toString("utf-8"));
};

/** Checks the `vapid t=…, k=…` header: ES256 by `k`, for this origin. */
const checkVapid = (header: string, origin: string): void => {
  const match = /^vapid t=(?<t>[\w-]+\.[\w-]+\.[\w-]+), k=(?<k>[\w-]+)$/u.exec(
    header
  );
  const token = match?.groups?.t;
  const key = match?.groups?.k;
  if (token === undefined || key === undefined) {
    throw new Error("no VAPID header");
  }
  const [head = "", claims = "", signature = ""] = token.split(".");
  const point = fromB64(key);
  const publicKey = createPublicKey({
    format: "jwk",
    key: {
      crv: "P-256",
      kty: "EC",
      x: point.subarray(1, 33).toString("base64url"),
      y: point.subarray(33, 65).toString("base64url"),
    },
  });
  const valid = verify(
    "sha256",
    Buffer.from(`${head}.${claims}`),
    { dsaEncoding: "ieee-p1363", key: publicKey },
    fromB64(signature)
  );
  const body: unknown = JSON.parse(fromB64(claims).toString("utf-8"));
  if (
    !valid ||
    typeof body !== "object" ||
    body === null ||
    Reflect.get(body, "aud") !== origin
  ) {
    throw new Error("bad VAPID token");
  }
};

export interface PushService {
  /** The endpoint to give the bridge. */
  readonly endpoint: string;
  /** Decrypted push objects, in order. */
  readonly received: unknown[];
  readonly close: () => Promise<void>;
}

const portOf = (server: Server): number => {
  const address = server.address();
  if (address === null || typeof address === "string") {
    throw new Error("not listening");
  }
  return address.port;
};

export const startPushService = async (): Promise<PushService> => {
  const received: unknown[] = [];
  const server = createServer((request, response) => {
    // The bridge only POSTs to the endpoint at 127.0.0.1. Anything else
    // (an IDE probing every new port with `GET /` on localhost) is not
    // a push and is not recorded.
    const host = request.headers.host?.split(":")[0];
    if (
      request.method !== "POST" ||
      request.url !== "/push/e2e" ||
      host !== "127.0.0.1"
    ) {
      request.resume();
      response.writeHead(404).end();
      return;
    }
    const chunks: Buffer[] = [];
    request.on("data", (chunk: Buffer) => {
      chunks.push(chunk);
    });
    request.on("end", () => {
      try {
        checkVapid(
          request.headers.authorization ?? "",
          `http://127.0.0.1:${portOf(server)}`
        );
        if (request.headers["content-encoding"] !== "aes128gcm") {
          throw new Error("not encrypted");
        }
        received.push(decrypt(Buffer.concat(chunks)));
        response.writeHead(201).end();
      } catch (error) {
        received.push({ error: String(error) });
        response.writeHead(400).end();
      }
    });
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  return {
    close: async () => {
      server.close();
      await once(server, "close");
    },
    endpoint: `http://127.0.0.1:${portOf(server)}/push/e2e`,
    received,
  };
};
