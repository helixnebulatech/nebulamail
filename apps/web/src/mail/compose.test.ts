// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import {
  BlobId,
  IdentityId,
  JmapError,
  MailboxId,
} from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import type { ReadyDraft } from "./compose";
import { checkDraft, draftEmail, draftText, isValidEmail } from "./compose";
import { DraftRefusedError, serverRefusalOf } from "./refusal";
import { SendFailedError } from "./send-failed";

const draft: ReadyDraft = {
  bcc: [{ email: "secret@example.com" }],
  cc: [],
  identity: {
    email: "alice@example.com",
    id: IdentityId.parse("i1"),
    name: "Alice",
    signature: null,
  },
  subject: "Hola",
  attachments: [],
  html: "<p>Texto</p>",
  to: [{ email: " bob@example.com ", name: " Bob " }],
};

describe(isValidEmail, () => {
  it.each([
    ["bob@example.com", true],
    [" bob@example.com ", true],
    ["bob@localhost", false],
    ["bob example@x.com", false],
    ["<bob@x.com>", false],
    ["a@b@c.com", false],
    ["", false],
    ["@example.com", false],
    ["bob@.com", false],
    ["bob@example.", false],
    ["bob@a.b", true],
    ["bob@a..b", true],
    ["bob@ex@ample.com", false],
    [`${"a".repeat(64)}@${"b".repeat(185)}.com`, true],
    [`${"a".repeat(64)}@${"b".repeat(186)}.com`, false],
  ])("%j → %s", (email, expected) => {
    expect(isValidEmail(email)).toBe(expected);
  });

  it("is linear on hostile input (a mailto: with many dots)", () => {
    const start = performance.now();
    for (const tail of ['"', "", "@", " "]) {
      expect(isValidEmail(`a@${".".repeat(100_000)}${tail}`)).toBeFalsy();
      expect(isValidEmail(`${"a".repeat(100_000)}@b.c${tail}`)).toBeFalsy();
    }
    expect(performance.now() - start).toBeLessThan(200);
  });
});

describe(checkDraft, () => {
  it("needs at least one valid recipient", () => {
    expect([
      checkDraft(draft),
      checkDraft({ ...draft, bcc: [], to: [] }),
      checkDraft({ ...draft, cc: [{ email: "nope" }] }),
    ]).toStrictEqual([null, "no-recipients", "invalid-recipient"]);
  });

  it("can't send before the sender is known (opened offline)", () => {
    expect(checkDraft({ ...draft, identity: null })).toBe("no-identity");
  });
});

describe(draftEmail, () => {
  it("builds a $draft in Drafts with trimmed addresses, HTML and its text", () => {
    const drafts = MailboxId.parse("drafts");

    const email = draftEmail(draft, drafts, "Texto");

    expect(email).toMatchObject({
      bcc: [{ email: "secret@example.com", name: null }],
      bodyValues: {
        html: { value: "<p>Texto</p>" },
        text: { value: "Texto" },
      },
      htmlBody: [{ partId: "html", type: "text/html" }],
      from: [{ email: "alice@example.com", name: "Alice" }],
      keywords: { $draft: true, $seen: true },
      mailboxIds: { drafts: true },
      textBody: [{ partId: "text", type: "text/plain" }],
      to: [{ email: "bob@example.com", name: "Bob" }],
    });
    // Stalwart rejects null header fields on create: empty ones are omitted.
    expect(email).not.toHaveProperty("cc");
    expect(email).not.toHaveProperty("inReplyTo");
    expect(email).not.toHaveProperty("attachments");
    expect(email).not.toHaveProperty(
      "header:Disposition-Notification-To:asAddresses"
    );
  });

  // RFC 8621 §6: the identity's replyTo is what the client SHOULD set.
  it("sets the identity's Reply-To, and none when it has none", () => {
    const help = ["help", "example.com"].join("@");
    const withReplyTo = draftEmail(
      {
        ...draft,
        identity: {
          ...draft.identity,
          replyTo: [{ email: help, name: "Help" }],
        },
      },
      MailboxId.parse("drafts"),
      "Texto"
    );
    const without = draftEmail(
      { ...draft, identity: { ...draft.identity, replyTo: [] } },
      MailboxId.parse("drafts"),
      "Texto"
    );

    expect(withReplyTo.replyTo).toStrictEqual([{ email: help, name: "Help" }]);
    expect(without).not.toHaveProperty("replyTo");
    expect(draftEmail(draft, MailboxId.parse("drafts"), "")).not.toHaveProperty(
      "replyTo"
    );
  });

  it("asks for a read receipt to the sender only when chosen", () => {
    const email = draftEmail(
      { ...draft, readReceipt: true },
      MailboxId.parse("drafts"),
      "Texto"
    );
    expect(
      email["header:Disposition-Notification-To:asAddresses"]
    ).toStrictEqual([{ email: "alice@example.com", name: "Alice" }]);
  });

  it("refers to uploaded attachments by blobId", () => {
    const email = draftEmail(
      {
        ...draft,
        attachments: [
          {
            blobId: BlobId.parse("B1"),
            name: "plano.pdf",
            size: 10,
            type: "application/pdf",
          },
        ],
      },
      MailboxId.parse("drafts"),
      ""
    );
    expect(email.attachments).toStrictEqual([
      {
        blobId: "B1",
        disposition: "attachment",
        name: "plano.pdf",
        type: "application/pdf",
      },
    ]);
  });

  it("puts quoted inline images in multipart/related, by Content-ID", () => {
    const email = draftEmail(
      {
        ...draft,
        attachments: [
          {
            blobId: BlobId.parse("B1"),
            name: "plano.pdf",
            size: 10,
            type: "application/pdf",
          },
          {
            blobId: BlobId.parse("B2"),
            cid: "logo@x",
            name: "logo.png",
            size: 10,
            type: "image/png",
          },
        ],
        html: `<p><img alt="" src="cid:logo@x"></p>`,
      },
      MailboxId.parse("drafts"),
      ""
    );
    // bodyStructure replaces htmlBody, textBody and attachments (RFC 8621).
    expect(email).not.toHaveProperty("htmlBody");
    expect(email).not.toHaveProperty("attachments");
    expect(email.bodyStructure).toStrictEqual({
      subParts: [
        {
          subParts: [
            {
              subParts: [
                { partId: "text", type: "text/plain" },
                { partId: "html", type: "text/html" },
              ],
              type: "multipart/alternative",
            },
            {
              blobId: "B2",
              cid: "logo@x",
              disposition: "inline",
              name: "logo.png",
              type: "image/png",
            },
          ],
          type: "multipart/related",
        },
        {
          blobId: "B1",
          disposition: "attachment",
          name: "plano.pdf",
          type: "application/pdf",
        },
      ],
      type: "multipart/mixed",
    });
  });
});

describe("plain-text mode and the default font", () => {
  it("sends only text/plain: no HTML part, no inline images", () => {
    const plain: ReadyDraft = {
      ...draft,
      attachments: [
        {
          blobId: BlobId.parse("B1"),
          name: "plano.pdf",
          size: 10,
          type: "application/pdf",
        },
        {
          blobId: BlobId.parse("B2"),
          cid: "logo@x",
          name: "logo.png",
          size: 10,
          type: "image/png",
        },
      ],
      bodyStyle: { font: "georgia", size: "18px" },
      html: `<p><img alt="" src="cid:logo@x"></p>`,
      plainText: "Hola\r\n\n-- \nAna\n\n",
    };
    const email = draftEmail(
      plain,
      MailboxId.parse("drafts"),
      draftText(plain)
    );
    expect(email.bodyValues).toStrictEqual({
      text: { value: "Hola\n\n-- \nAna\n" },
    });
    expect(email.textBody).toStrictEqual([
      { partId: "text", type: "text/plain" },
    ]);
    expect(email).not.toHaveProperty("htmlBody");
    expect(email).not.toHaveProperty("bodyStructure");
    expect(email.attachments).toStrictEqual([
      {
        blobId: "B1",
        disposition: "attachment",
        name: "plano.pdf",
        type: "application/pdf",
      },
    ]);
  });

  it("keeps an empty plain body empty", () => {
    expect(draftText({ ...draft, plainText: " \n" })).toBe("");
  });

  it("wraps the HTML in the default font and size; the text is unchanged", () => {
    const styled: ReadyDraft = {
      ...draft,
      bodyStyle: { font: "times", size: "16px" },
    };
    const email = draftEmail(styled, MailboxId.parse("drafts"), "Texto\n");
    expect(email.bodyValues).toStrictEqual({
      html: {
        value: `<div style="font-family: Times New Roman, serif; font-size: 16px"><p>Texto</p></div>`,
      },
      text: { value: "Texto\n" },
    });
  });
});

const methodError = (type: string) =>
  new JmapError({
    arguments: {},
    callId: "c0",
    kind: "method",
    method: "Email/set",
    type,
  });

const refusedOver = (limit: string) =>
  new JmapError({
    kind: "request",
    limit,
    status: 400,
    type: "urn:ietf:params:jmap:error:limit",
  });

describe(serverRefusalOf, () => {
  it("looks through a failed send at its cause", () => {
    expect([
      serverRefusalOf(
        new SendFailedError(new DraftRefusedError("tooLarge", null), null)
      ),
      serverRefusalOf(
        new SendFailedError(methodError("tooManyKeywords"), null)
      ),
      serverRefusalOf(new SendFailedError(new Error("x"), null)),
    ]).toStrictEqual(["tooLarge", "tooManyKeywords", null]);
  });

  it("names the refusals the user can fix", () => {
    expect(serverRefusalOf(new DraftRefusedError("tooLarge", null))).toBe(
      "tooLarge"
    );
    expect(
      serverRefusalOf(new DraftRefusedError("tooManyMailboxes", "max 100"))
    ).toBe("tooManyMailboxes");
    expect(
      serverRefusalOf(new DraftRefusedError("tooManyKeywords", null))
    ).toBe("tooManyKeywords");
    expect(serverRefusalOf(methodError("requestTooLarge"))).toBe("tooLarge");
    expect(
      serverRefusalOf(
        new JmapError({
          actual: 2,
          kind: "limit",
          limit: "maxSizeRequest",
          max: 1,
        })
      )
    ).toBe("tooLarge");
  });

  it("doesn't call a busy server a size problem", () => {
    expect(serverRefusalOf(refusedOver("maxSizeRequest"))).toBe("tooLarge");
    expect(serverRefusalOf(refusedOver("maxConcurrentRequests"))).toBeNull();
  });

  it("leaves everything else to the generic message", () => {
    expect(
      serverRefusalOf(new DraftRefusedError("overQuota", null))
    ).toBeNull();
    expect(serverRefusalOf(methodError("serverFail"))).toBeNull();
    expect(serverRefusalOf(new JmapError({ kind: "network" }))).toBeNull();
    expect(serverRefusalOf(new Error("x"))).toBeNull();
  });
});
