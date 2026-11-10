// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// The pinned undici connector. undici's connector and Node's `lookup` are
// callback APIs we must implement as-is, so the callback-style lint rules are
// disabled for this file only.
/* oxlint-disable promise/prefer-await-to-callbacks, node/callback-return */

import type { LookupFunction, Socket } from "node:net";

import { buildConnector } from "undici";

import { GuardError } from "../errors";
import type { Dial } from "../http/hops";
import type { ValidatedTarget } from "../resolve";

export type { Dial } from "../http/hops";

// Tripwire: the connector always dials an IP, so Node should never resolve.
const refuseLookup: LookupFunction = (_hostname, _options, callback) => {
  callback(new GuardError("dns"), "", 0);
};

/**
 * Connector that only dials the validated addresses of `target`, in order,
 * and refuses any other origin. TLS verifies the certificate against the
 * original hostname (`servername`), not against the IP we dial.
 */
export const pinnedConnector = (
  target: ValidatedTarget,
  connectTimeoutMs: number,
  dial: Dial | undefined
): buildConnector.connector => {
  const base = buildConnector({
    allowH2: false,
    lookup: refuseLookup,
    maxCachedSessions: 0,
    timeout: connectTimeoutMs,
  });
  const servername = target.host.kind === "name" ? target.host.name : undefined;

  return (options, callback) => {
    if (
      options.hostname !== target.hostname ||
      options.httpSocket !== undefined
    ) {
      callback(new GuardError("private-ip"), null);
      return;
    }
    let next = 0;
    // Try each validated address until one connects (all passed the guard).
    const attempt = (lastError: Error): void => {
      const address = target.addresses[next];
      next += 1;
      if (address === undefined) {
        callback(lastError, null);
        return;
      }
      const where = dial?.(address, target.port) ?? {
        host: address,
        port: target.port,
      };
      base(
        {
          ...options,
          hostname: where.host,
          port: String(where.port),
          ...(servername === undefined ? {} : { servername }),
        },
        (...result: [null, Socket] | [Error, null]) => {
          if (result[0] === null) {
            callback(null, result[1]);
            return;
          }
          attempt(result[0]);
        }
      );
    };
    attempt(new GuardError("network"));
  };
};
