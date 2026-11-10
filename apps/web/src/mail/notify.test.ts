// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { EmailId, MailboxId } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import type { ArrivedEmail } from "./notify";
import { notificationContent, worthNotifying } from "./notify";

const NOW = Date.UTC(2026, 9, 24, 12);
const INBOX = MailboxId.parse("inbox");

const email = (
  id: string,
  change: Partial<ArrivedEmail> = {}
): ArrivedEmail => ({
  from: [{ email: "ana@x.com", name: "Ana" }],
  id: EmailId.parse(id),
  keywords: {},
  mailboxIds: { [INBOX]: true },
  receivedAt: new Date(NOW - 60_000).toISOString(),
  subject: "Hola",
  ...change,
});

const labels = {
  many: (count: number) => `${count} new`,
  noSubject: "(no subject)",
};

describe(worthNotifying, () => {
  it("keeps new unread inbox mail only", () => {
    const kept = worthNotifying(
      [
        email("a"),
        email("read", { keywords: { $seen: true } }),
        email("junk", { keywords: { $junk: true } }),
        email("sent", { mailboxIds: { [MailboxId.parse("sent")]: true } }),
        email("old", {
          receivedAt: new Date(NOW - 60 * 60_000).toISOString(),
        }),
      ],
      INBOX,
      NOW
    );
    expect(kept.map((item) => item.id)).toStrictEqual(["a"]);
  });
});

describe(notificationContent, () => {
  it("shows sender and subject only, cleaned", () => {
    expect(
      notificationContent(
        [
          email("a", {
            from: [{ email: "ev@x.com", name: "Pay\u202EPal" }],
            subject: "Factura\u200B",
          }),
          email("b", { from: [{ email: "bob@y.com" }], subject: null }),
        ],
        labels
      )
    ).toStrictEqual([
      { body: "Factura", emailId: "a", tag: "wm-a", title: "PayPal" },
      { body: "(no subject)", emailId: "b", tag: "wm-b", title: "bob@y.com" },
    ]);
  });

  it("sums up more than three", () => {
    const many = ["a", "b", "c", "d"].map((id) => email(id));
    expect(notificationContent(many, labels)).toStrictEqual([
      { body: "", emailId: null, tag: "wm-new-mail", title: "4 new" },
    ]);
  });
});
