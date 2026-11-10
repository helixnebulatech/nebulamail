// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { BlobId, EmailId, ThreadId } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import type { EmailDetail } from "./api";
import {
  buildReply,
  reopenDraft,
  replyReferences,
  replySubject,
} from "./reply";

const labels = {
  formatDate: (iso: string) => iso.slice(0, 10),
  forwarded: ({
    from,
    subject,
  }: {
    readonly from: string;
    readonly subject: string;
  }) => `--- Forwarded: ${from} / ${subject}`,
  forwardedAttached: "Attached.",
  quoteTruncated: "[Cut.]",
  wrote: (date: string, sender: string) => `On ${date}, ${sender} wrote:`,
};

const email: EmailDetail = {
  accountId: null,
  attachments: [],
  bcc: [],
  blobId: BlobId.parse("b1"),
  preloadToken: null,
  readReceipt: null,
  senderVerdict: null,
  cc: [
    { email: "carla@x.com", name: null },
    { email: "me@home.com", name: "Me" },
  ],
  draft: false,
  flagged: false,
  from: [{ email: "ana@x.com", name: "Ana" }],
  hasAttachment: false,
  html: null,
  id: EmailId.parse("e1"),
  inReplyTo: ["m1@x.com"],
  inlineParts: [],
  mailboxIds: [],
  messageId: ["m2@x.com"],
  preview: "Hola",
  receivedAt: "2026-10-23T09:41:00Z",
  references: ["m1@x.com"],
  replyTo: [],
  sentAt: "2026-10-23T09:40:00Z",
  subject: "Presupuesto",
  text: "Hola\n> antes\nAdiós\n",
  threadId: ThreadId.parse("t1"),
  to: [
    { email: "ME@home.com", name: null },
    { email: "bob@y.com", name: "Bob" },
  ],
  truncated: false,
  unread: false,
  size: 1000,
  unsubscribe: { mailto: null, oneClick: null, web: null },
};

describe(replySubject, () => {
  it("adds Re:/Fwd: once", () => {
    expect(replySubject("Hola", "reply")).toBe("Re: Hola");
    expect(replySubject("RE: Hola", "replyAll")).toBe("RE: Hola");
    expect(replySubject("Hola", "forward")).toBe("Fwd: Hola");
    expect(replySubject("Fw: Hola", "forward")).toBe("Fw: Hola");
  });
});

describe(buildReply, () => {
  const own = ["me@home.com"];

  it("replies to the sender, threaded, from the receiving identity", () => {
    expect(buildReply(email, "reply", own, labels)).toStrictEqual({
      cc: [],
      identityEmail: "me@home.com",
      inReplyTo: ["m2@x.com"],
      // The original has no HTML: the reply starts in plain text.
      plainText: true,
      quoteHtml:
        "<div data-quote><p>On 2026-10-23, Ana &lt;ana@x.com&gt; wrote:</p><blockquote><p>Hola</p><p>&gt; antes</p><p>Adiós</p><p></p></blockquote></div>",
      references: ["m1@x.com", "m2@x.com"],
      subject: "Re: Presupuesto",
      to: [{ email: "ana@x.com", name: "Ana" }],
    });
  });

  it("quotes the HTML part through the sanitizer it is given", () => {
    const seen: (string | null)[] = [];
    const reply = buildReply(
      { ...email, html: "<p>raw</p>" },
      "reply",
      own,
      labels,
      (html) => {
        seen.push(html);
        return "<p>clean</p>";
      }
    );
    expect(seen).toStrictEqual(["<p>raw</p>"]);
    expect(reply.quoteHtml).toContain("<blockquote><p>clean</p></blockquote>");
    // An HTML original keeps the default format.
    expect(reply).not.toHaveProperty("plainText");
  });

  it("prefers Reply-To", () => {
    const reply = buildReply(
      { ...email, replyTo: [{ email: "list@x.com", name: null }] },
      "reply",
      own,
      labels
    );
    expect(reply.to).toStrictEqual([{ email: "list@x.com" }]);
  });

  it("reply all adds the other recipients, never me", () => {
    const reply = buildReply(email, "replyAll", own, labels);
    expect(reply.to).toStrictEqual([
      { email: "ana@x.com", name: "Ana" },
      { email: "bob@y.com", name: "Bob" },
    ]);
    expect(reply.cc).toStrictEqual([{ email: "carla@x.com" }]);
  });

  it("replying to my own message goes to its recipients", () => {
    const reply = buildReply(
      { ...email, from: [{ email: "me@home.com", name: "Me" }] },
      "reply",
      own,
      labels
    );
    expect(reply.to).toStrictEqual([
      { email: "ME@home.com" },
      { email: "bob@y.com", name: "Bob" },
    ]);
  });

  it("forwards the original's attachments, replies don't", () => {
    const file = {
      blobId: BlobId.parse("B1"),
      name: "factura.pdf",
      size: 10,
      type: "application/pdf",
    };
    const withFile = { ...email, attachments: [file] };
    expect(
      buildReply(withFile, "forward", own, labels).attachments
    ).toStrictEqual([file]);
    expect(
      buildReply(withFile, "reply", own, labels).attachments
    ).toBeUndefined();
  });

  it("keeps the original's inline images (by blobId) in forwards and replies", () => {
    const logo = {
      blobId: BlobId.parse("B2"),
      cid: "logo@x",
      name: "logo.png",
      size: 20,
      type: "image/png",
    };
    const file = {
      blobId: BlobId.parse("B1"),
      name: "factura.pdf",
      size: 10,
      type: "application/pdf",
    };
    const withImage = {
      ...email,
      attachments: [file],
      html: `<p><img src="cid:logo@x"></p>`,
      inlineParts: [logo],
    };
    const cids: (readonly string[])[] = [];
    const toEditable = (
      _html: string | null,
      _text: string | null,
      inlineCids: readonly string[]
    ): string => {
      cids.push(inlineCids);
      return `<p><img src="cid:logo@x" alt=""></p>`;
    };
    expect(
      buildReply(withImage, "forward", own, labels, toEditable).attachments
    ).toStrictEqual([file, logo]);
    const reply = buildReply(withImage, "reply", own, labels, toEditable);
    expect(reply.attachments).toStrictEqual([logo]);
    expect(reply.quoteHtml).toContain(`<img src="cid:logo@x" alt="">`);
    expect(cids).toStrictEqual([["logo@x"], ["logo@x"]]);
  });

  it("forwards with a header and no recipients or threading", () => {
    const forward = buildReply(email, "forward", own, labels);
    expect(forward).toStrictEqual({
      identityEmail: "me@home.com",
      plainText: true,
      quoteHtml:
        "<div data-quote><p>--- Forwarded: Ana &lt;ana@x.com&gt; / Presupuesto</p><p></p><p>Hola</p><p>&gt; antes</p><p>Adiós</p><p></p></div>",
      subject: "Fwd: Presupuesto",
    });
  });
});

const reopened = (...args: Parameters<typeof reopenDraft>) => {
  const result = reopenDraft(...args);
  if (!result.ok) {
    throw new Error("not reopened");
  }
  return result.init;
};

describe(reopenDraft, () => {
  const draft: EmailDetail = {
    ...email,
    bcc: [{ email: "hidden@x.com", name: "Hidden" }],
    cc: [{ email: "carla@x.com", name: null }],
    draft: true,
    from: [{ email: "me@home.com", name: "Me" }],
    inReplyTo: ["m2@x.com"],
    references: ["m1@x.com", "m2@x.com"],
    html: "<p>saved</p>",
    subject: "Re: Presupuesto",
    to: [{ email: "ana@x.com", name: "Ana" }],
  };

  it("reopens a draft saved without HTML in plain-text mode", () => {
    const init = reopened({ ...draft, html: null, text: "Hola\n-- \nMe\n" });
    expect(init).toMatchObject({
      plainText: true,
      restored: true,
      text: "Hola\n-- \nMe\n",
    });
    expect(init).not.toHaveProperty("html");
  });

  it("keeps Bcc and the threading headers of a saved draft", () => {
    expect(reopened(draft, () => "<p>body</p>")).toStrictEqual({
      attachments: [],
      bcc: [{ email: "hidden@x.com", name: "Hidden" }],
      cc: [{ email: "carla@x.com" }],
      html: "<p>body</p>",
      identityEmail: "me@home.com",
      inReplyTo: ["m2@x.com"],
      references: ["m1@x.com", "m2@x.com"],
      restored: true,
      subject: "Re: Presupuesto",
      to: [{ email: "ana@x.com", name: "Ana" }],
    });
  });

  it("leaves out threading headers a new message doesn't have", () => {
    const init = reopened(
      { ...draft, inReplyTo: [], references: [] },
      () => ""
    );

    expect(init).not.toHaveProperty("inReplyTo");
    expect(init).not.toHaveProperty("references");
  });

  it("never reopens a draft too long to edit", () => {
    expect(reopenDraft({ ...draft, truncated: true })).toStrictEqual({
      ok: false,
      reason: "too-large",
    });
    expect(
      reopenDraft({ ...draft, html: "x".repeat(2_000_001) })
    ).toStrictEqual({ ok: false, reason: "too-large" });
  });
});

describe("forwarding a long email", () => {
  const own: readonly string[] = [];
  const long: EmailDetail = {
    ...email,
    attachments: [
      {
        blobId: BlobId.parse("pdf"),
        name: "a.pdf",
        size: 5,
        type: "application/pdf",
      },
    ],
    html: "<p>secret body</p>",
    inlineParts: [
      {
        blobId: BlobId.parse("img"),
        cid: "i@x",
        name: "i.png",
        size: 1,
        type: "image/png",
      },
    ],
    truncated: true,
  };

  it("attaches the whole original, and nothing else", () => {
    const forward = buildReply(long, "forward", own, labels);
    expect(forward.attachments).toStrictEqual([
      {
        blobId: long.blobId,
        name: "Presupuesto.eml",
        size: long.size,
        type: "message/rfc822",
      },
    ]);
    expect(forward.quoteHtml).not.toContain("secret body");
    expect(forward.quoteHtml).toContain("Attached.");
  });

  it("quotes part of it in a reply, saying so", () => {
    const reply = buildReply(long, "reply", own, labels, () => "<p>part</p>");
    expect(reply.quoteHtml).toContain("<p>part</p>");
    expect(reply.quoteHtml).toContain("[Cut.]");
  });
});

describe(replyReferences, () => {
  it.each([
    [
      "References, then Message-ID",
      {
        inReplyTo: ["[EMAIL]"],
        messageId: ["[EMAIL]"],
        references: ["[EMAIL]", "[EMAIL]"],
      },
      ["[EMAIL]", "[EMAIL]", "[EMAIL]"],
    ],
    [
      "no References: the single In-Reply-To, then Message-ID",
      { inReplyTo: ["[EMAIL]"], messageId: ["[EMAIL]"], references: [] },
      ["[EMAIL]", "[EMAIL]"],
    ],
    [
      "no References and several In-Reply-To: only Message-ID",
      {
        inReplyTo: ["[EMAIL]", "[EMAIL]"],
        messageId: ["[EMAIL]"],
        references: [],
      },
      ["[EMAIL]"],
    ],
    [
      "nothing at all: no References",
      { inReplyTo: [], messageId: [], references: [] },
      [],
    ],
  ])("RFC 5322 §3.6.4: %s", (_name, parent, expected) => {
    expect(replyReferences(parent)).toStrictEqual(expected);
  });
});
