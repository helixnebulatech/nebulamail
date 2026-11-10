// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { EmailId } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import {
  overallState,
  recipientState,
  recipientStatuses,
  statusesByEmail,
} from "./delivery";

const status = (delivered: string, displayed = "unknown", smtpReply = "") => ({
  delivered,
  displayed,
  smtpReply,
});

describe(recipientState, () => {
  it("maps RFC 8621 §7 values", () => {
    expect(recipientState(status("yes"))).toBe("delivered");
    expect(recipientState(status("no"))).toBe("failed");
    expect(recipientState(status("queued"))).toBe("queued");
    expect(recipientState(status("unknown"))).toBe("unknown");
  });

  it("reads receipts, but a bounce wins", () => {
    expect(recipientState(status("yes", "yes"))).toBe("read");
    // A receipt without a DSN still means it arrived.
    expect(recipientState(status("unknown", "yes"))).toBe("read");
    // A bounce wins over a (strange) receipt.
    expect(recipientState(status("no", "yes"))).toBe("failed");
  });
});

describe(recipientStatuses, () => {
  it("keeps the reply of bounces and delays only, cleaned", () => {
    expect(
      recipientStatuses({
        "a@x.com": status("yes", "unknown", "250 2.0.0 OK"),
        "b@x.com": status("no", "unknown", "550 5.1.1\r\n user\tunknown"),
        "c@x.com": status("queued", "unknown", "451 try later"),
      })
    ).toStrictEqual([
      { email: "a@x.com", reply: null, state: "delivered" },
      { email: "b@x.com", reply: "550 5.1.1 user unknown", state: "failed" },
      { email: "c@x.com", reply: "451 try later", state: "queued" },
    ]);
    expect(recipientStatuses(null)).toStrictEqual([]);
  });

  it("drops 2xx replies: they say it was taken, not why it bounced", () => {
    expect(
      recipientStatuses({
        "a@x.com": status("no", "unknown", "250 2.0.0 Ok: queued as 1539"),
        "b@x.com": status("queued", "unknown", "250-2.1.5 Queued"),
        "c@x.com": status("no", "unknown", "2500 is not a code"),
      }).map((entry) => entry.reply)
    ).toStrictEqual([null, null, "2500 is not a code"]);
  });

  it("reads Stalwart 1.0's spelled-out replies", () => {
    expect(
      recipientStatuses({
        "a@x.com": status(
          "queued",
          "unknown",
          "Code: 250, Enhanced code: 2.1.5, Message: OK"
        ),
        "b@x.com": status(
          "queued",
          "unknown",
          "Code: 451, Enhanced code: 4.3.0, Message: Try again later"
        ),
        "c@x.com": status(
          "no",
          "unknown",
          "Domain does not accept messages (null MX)"
        ),
      }).map((entry) => entry.reply)
    ).toStrictEqual([
      null,
      "451 4.3.0 Try again later",
      "Domain does not accept messages (null MX)",
    ]);
  });

  it("cuts very long replies", () => {
    const [entry] = recipientStatuses({
      "b@x.com": status("no", "unknown", `550 ${"x".repeat(1000)}`),
    });
    expect(entry?.reply?.length).toBe(301);
  });
});

const of = (...states: string[]) =>
  states.map((state, index) => ({
    email: `${String(index)}@x.com`,
    reply: null,
    state: recipientState(
      state === "read" ? status("yes", "yes") : status(state)
    ),
  }));

describe(overallState, () => {
  it("puts bounces first, then delays", () => {
    expect(overallState(of("yes", "no", "queued"))).toBe("failed");
    expect(overallState(of("yes", "queued"))).toBe("queued");
  });

  it("is read only when everyone read it", () => {
    expect(overallState(of("read", "read"))).toBe("read");
    expect(overallState(of("read", "yes"))).toBe("delivered");
  });

  it("shows nothing while anything is unknown", () => {
    expect(overallState(of("yes", "unknown"))).toBe("unknown");
    expect(overallState([])).toBe("unknown");
  });
});

describe(statusesByEmail, () => {
  const e1 = EmailId.parse("e1");
  const e2 = EmailId.parse("e2");

  it("uses the newest submission of each wanted email, never cancelled ones", () => {
    const result = statusesByEmail(
      [
        {
          deliveryStatus: { "a@x.com": status("no") },
          emailId: e1,
          sendAt: "2026-10-27T09:00:00Z",
          undoStatus: "final",
        },
        {
          deliveryStatus: { "a@x.com": status("yes") },
          emailId: e1,
          sendAt: "2026-10-27T10:00:00Z",
          undoStatus: "final",
        },
        {
          deliveryStatus: { "b@x.com": status("yes") },
          emailId: e2,
          sendAt: "2026-10-27T11:00:00Z",
          undoStatus: "canceled",
        },
        {
          deliveryStatus: { "c@x.com": status("yes") },
          emailId: EmailId.parse("other"),
          sendAt: "2026-10-27T11:00:00Z",
          undoStatus: "final",
        },
      ],
      new Set([e1, e2])
    );
    expect([...result.keys()]).toStrictEqual([e1]);
    expect(result.get(e1)?.[0]?.state).toBe("delivered");
  });

  it("leaves out submissions without status", () => {
    const result = statusesByEmail(
      [
        {
          deliveryStatus: null,
          emailId: e1,
          sendAt: "2026-10-27T09:00:00Z",
          undoStatus: "final",
        },
      ],
      new Set([e1])
    );
    expect(result.size).toBe(0);
  });
});
