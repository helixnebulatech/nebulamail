// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// @vitest-environment jsdom

import { BlobId, EmailId, MailboxId, ThreadId } from "@nebulamail/jmap-client";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  NOT_SPAM_KEYWORDS,
  SPAM_KEYWORDS,
  htmlBodyContent,
  movePatch,
  sortMailboxes,
  splitAttachments,
  toSummary,
  unsubscribeOneClick,
} from "./api";

const mailbox = (name: string, role: string | null, sortOrder = 0) => ({
  id: MailboxId.parse(name.toLowerCase()),
  name,
  parentId: null,
  role,
  sortOrder,
  totalEmails: 3,
  unreadEmails: 1,
});

describe(sortMailboxes, () => {
  it("puts special folders first in a fixed order, then the rest", () => {
    const sorted = sortMailboxes([
      mailbox("Zeta", null),
      mailbox("Trash", "trash"),
      mailbox("Alpha", null),
      mailbox("Sent", "sent"),
      mailbox("Inbox", "inbox"),
      mailbox("Custom", null, -1),
      mailbox("Junk", "junk"),
      mailbox("Important", "important"),
    ]);

    expect(sorted.map((box) => box.name)).toStrictEqual([
      "Inbox",
      "Sent",
      "Junk",
      "Trash",
      "Custom",
      "Alpha",
      "Important",
      "Zeta",
    ]);
  });

  it("drops roles the UI doesn't treat specially", () => {
    const [box] = sortMailboxes([mailbox("Important", "important")]);

    expect(box?.role).toBeNull();
  });
});

describe(toSummary, () => {
  const base = {
    from: [{ email: "bob@example.com", name: "Bob" }],
    hasAttachment: false,
    id: EmailId.parse("e1"),
    preview: "Hola",
    receivedAt: "2026-10-23T10:00:00Z",
    subject: null,
    threadId: ThreadId.parse("t1"),
  };

  it("maps keywords to unread and flagged", () => {
    expect(toSummary({ ...base, keywords: {} })).toMatchObject({
      flagged: false,
      subject: "",
      unread: true,
    });
    expect(
      toSummary({ ...base, keywords: { $flagged: true, $seen: true } })
    ).toMatchObject({ flagged: true, unread: false });
  });
});

const bodyPart = (cid: string, type: string, disposition = "inline") => ({
  blobId: BlobId.parse("b1"),
  cid: `<${cid}>`,
  disposition,
  name: cid,
  size: 1,
  type,
});

const split = (html: string, ...parts: ReturnType<typeof bodyPart>[]) => {
  const result = splitAttachments(parts, null, html);
  return {
    attachments: result.attachments.map((item) => item.name),
    inline: result.inlineParts.map((item) => item.cid),
  };
};

describe(splitAttachments, () => {
  it("keeps cid images apart from the attachments to list", () => {
    const blob = BlobId.parse("b1");
    const result = splitAttachments(
      [
        {
          blobId: blob,
          cid: "<logo@x>",
          disposition: "inline",
          name: "logo.png",
          size: 10,
          type: "image/png",
        },
        {
          blobId: blob,
          disposition: "attachment",
          name: "factura.pdf",
          size: 20,
          type: "application/pdf",
        },
        {
          blobId: blob,
          cid: "<forced@x>",
          disposition: "attachment",
          name: "a.png",
          size: 5,
          type: "image/png",
        },
        { blobId: null, name: "broken" },
      ],
      null,
      '<img src="cid:logo@x"><img src="cid:forced@x">'
    );

    expect(result.inlineParts.map((part) => part.cid)).toStrictEqual([
      "logo@x",
    ]);
    expect(result.attachments.map((part) => part.name)).toStrictEqual([
      "factura.pdf",
      "a.png",
    ]);
  });

  it("lists what the HTML doesn't show as an image", () => {
    expect(
      split(
        '<img src="cid:pic"><a href="cid:doc">doc</a>',
        bodyPart("doc", "application/pdf"),
        bodyPart("pic", "image/png"),
        bodyPart("unused", "image/png"),
        bodyPart("odd cid", "image/png")
      )
    ).toStrictEqual({
      attachments: ["doc", "unused", "odd cid"],
      inline: ["pic"],
    });
  });

  it("finds references however the HTML writes them", () => {
    expect(
      split(
        '<img src="cid&#58;a"><img src="CID:B"><p style="background:url(cid:c)"></p><table background="cid:d"></table>',
        bodyPart("a", "image/png"),
        bodyPart("b", "image/gif"),
        bodyPart("c", "image/jpeg"),
        bodyPart("d", "image/webp")
      )
    ).toStrictEqual({ attachments: [], inline: ["a", "b", "c", "d"] });
  });

  it("never takes an SVG as inline", () => {
    expect(
      split('<img src="cid:s">', bodyPart("s", "image/svg+xml"))
    ).toStrictEqual({ attachments: ["s"], inline: [] });
  });
});

describe(movePatch, () => {
  const junk = MailboxId.parse("junk");
  const inbox = MailboxId.parse("inbox");

  it("moves to exactly one mailbox", () => {
    expect(movePatch(junk)).toStrictEqual({ mailboxIds: { junk: true } });
  });

  it("reports spam with $junk and without $notjunk", () => {
    expect(movePatch(junk, SPAM_KEYWORDS)).toStrictEqual({
      "keywords/$junk": true,
      "keywords/$notjunk": null,
      mailboxIds: { junk: true },
    });
  });

  it("marks not spam with $notjunk and without $junk", () => {
    expect(movePatch(inbox, NOT_SPAM_KEYWORDS)).toStrictEqual({
      "keywords/$junk": null,
      "keywords/$notjunk": true,
      mailboxIds: { inbox: true },
    });
  });
});

const answer = (status: number, body: unknown) => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => await Promise.resolve(Response.json(body, { status })))
  );
};

describe(unsubscribeOneClick, () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it.each([
    [200, { ok: true }, "done"],
    [422, { error: "not-allowed", ok: false }, "not-allowed"],
    // RFC 8058 §3.1: a redirect is not a confirmation.
    [502, { error: "redirected", ok: false }, "redirected"],
    [502, { error: "rejected", ok: false }, "failed"],
    [500, "oops", "failed"],
  ])("maps %i %o to %s", async (status, body, result) => {
    answer(status, body);
    await expect(unsubscribeOneClick(EmailId.parse("e1"))).resolves.toBe(
      result
    );
  });
});

const part = (partId: string, type: string) => ({ partId, type });

describe(htmlBodyContent, () => {
  const values = {
    "1": { isTruncated: false, value: "<p>Hello</p>" },
    "2": { isTruncated: false, value: "Footer <b>not bold</b>\nline & 2" },
    "3": { isTruncated: true, value: "<p>Bye</p>" },
    "4": { isTruncated: false, value: "only text" },
  };

  it("shows text/plain parts in order, escaped (RFC 8621 §4.1.4)", () => {
    expect(
      htmlBodyContent(
        [
          part("1", "text/html"),
          part("2", "text/plain"),
          part("3", "TEXT/HTML"),
        ],
        values
      )
    ).toStrictEqual({
      content:
        "<p>Hello</p><div>Footer &lt;b&gt;not bold&lt;/b&gt;<br>line &amp; 2</div><p>Bye</p>",
      truncated: true,
    });
  });

  it("has no HTML view without a text/html part", () => {
    expect(htmlBodyContent([part("4", "text/plain")], values)).toStrictEqual({
      content: null,
      truncated: false,
    });
  });

  it("skips images and parts without a value", () => {
    expect(
      htmlBodyContent(
        [
          part("1", "text/html"),
          part("9", "text/plain"),
          part("5", "image/png"),
        ],
        values
      )
    ).toStrictEqual({ content: "<p>Hello</p>", truncated: false });
  });
});
