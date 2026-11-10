// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import type { IncomingMessage } from "node:http";
import { Socket } from "node:net";

import { describe, expect, it } from "vitest";

import { clientAddressFrom } from "./client-ip";

const SOCKET = "192.0.2.10";

const request = (
  headers: Record<string, string | string[]>
): IncomingMessage => {
  const socket = new Socket();
  Object.defineProperty(socket, "remoteAddress", { value: SOCKET });
  // Only `headers` and `socket` are read.
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion
  return { headers, socket } as unknown as IncomingMessage;
};

describe(clientAddressFrom, () => {
  it("socket: ignores the headers", () => {
    expect(
      clientAddressFrom("socket")(
        request({
          "x-forwarded-for": "203.0.113.1",
          "x-real-ip": "203.0.113.2",
        })
      )
    ).toBe(SOCKET);
  });

  it("x-forwarded-for: the last entry, the one the proxy appended", () => {
    const from = clientAddressFrom("x-forwarded-for");
    expect(
      from(request({ "x-forwarded-for": "198.51.100.7, 203.0.113.1" }))
    ).toBe("203.0.113.1");
    expect(
      from(
        request({
          "x-forwarded-for": ["198.51.100.7", "198.51.100.8,  2001:db8::1 "],
        })
      )
    ).toBe("2001:db8::1");
    expect(
      from(request({ "x-forwarded-for": "198.51.100.7, [2001:db8::2]" }))
    ).toBe("2001:db8::2");
  });

  it("x-forwarded-for: missing or garbage falls back to the socket", () => {
    const from = clientAddressFrom("x-forwarded-for");
    expect(from(request({}))).toBe(SOCKET);
    expect(from(request({ "x-forwarded-for": "203.0.113.1, evil" }))).toBe(
      SOCKET
    );
    expect(from(request({ "x-forwarded-for": "203.0.113.1," }))).toBe(SOCKET);
  });

  it("x-real-ip: one IP, otherwise the socket", () => {
    const from = clientAddressFrom("x-real-ip");
    expect(from(request({ "x-real-ip": "203.0.113.2" }))).toBe("203.0.113.2");
    expect(from(request({ "x-real-ip": "203.0.113.2, 203.0.113.3" }))).toBe(
      SOCKET
    );
    expect(from(request({ "x-real-ip": "localhost" }))).toBe(SOCKET);
    expect(from(request({}))).toBe(SOCKET);
  });
});
