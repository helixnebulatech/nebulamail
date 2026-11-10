// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// sendItip never sends from an identity other than the address the message
// speaks for (RFC 6047 §3, RFC 5546 §6.1): the other side would ignore it.

import { describe, expect, it, vi } from "vitest";

import { scriptedJmap } from "../test/scripted-jmap";

const jmap = vi.hoisted(() => ({
  connect: null as null | (() => Promise<unknown>),
}));

vi.mock(import("../mail/client"), async () => {
  const { JmapClient } = await import("@nebulamail/jmap-client");
  return {
    getClient: async () => {
      const client = await jmap.connect?.();
      if (!(client instanceof JmapClient)) {
        throw new TypeError("no scripted client");
      }
      return client;
    },
    resetClient: vi.fn<() => void>(),
  };
});

const { NoSendingIdentityError, sendItip } = await import("./invitations");

const message = (email: string) => ({
  from: { email, name: null },
  ics: "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n",
  method: "REPLY" as const,
  texts: { subject: "Accepted: x", text: "x" },
  to: { email: "org@example.org", name: null },
});

describe("sendItip", () => {
  it("refuses to send from another identity", async () => {
    const scripted = scriptedJmap({
      handlers: {
        "Identity/get": (_args, callId) => [
          [
            "Identity/get",
            {
              accountId: "a",
              list: [
                { email: "Ana@Example.org", id: "i1", name: "Ana" },
                { email: "boss@example.org", id: "i2", name: "Boss" },
              ],
              notFound: [],
              state: "1",
            },
            callId,
          ],
        ],
        "Mailbox/get": (_args, callId) => [
          [
            "Mailbox/get",
            {
              accountId: "a",
              list: [{ id: "m1", role: "sent" }],
              notFound: [],
              state: "1",
            },
            callId,
          ],
        ],
      },
    });
    const client = await scripted.connect();
    jmap.connect = async () => await Promise.resolve(client);
    const error: unknown = await sendItip(message("alias@example.org")).catch(
      (error_: unknown) => error_
    );
    expect(error).toBeInstanceOf(NoSendingIdentityError);
    expect(error).toMatchObject({ address: "alias@example.org" });
    // Nothing was written or sent: no Email/set, no submission.
    const names = scripted.calls.map(([name]) => name);
    expect(names).toContain("Identity/get");
    expect(names).not.toContain("Email/set");
    expect(names).not.toContain("EmailSubmission/set");
  });
});
