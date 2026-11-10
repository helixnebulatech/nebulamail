// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// @vitest-environment jsdom

import { BlobId, EmailId, ThreadId } from "@nebulamail/jmap-client";
import { renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { EmailDetail } from "./api";

const images = vi.hoisted(() => ({
  loadInlineImages:
    vi.fn<
      (
        parts: readonly unknown[],
        needed: readonly string[]
      ) => Promise<ReadonlyMap<string, string>>
    >(),
}));

vi.mock(import("./images"), async (importOriginal) => ({
  ...(await importOriginal()),
  loadInlineImages: images.loadInlineImages,
}));

const { useRenderedEmail } = await import("./rendering");

const PIXEL =
  "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

const cids = ["a", "b", "c", "d", "e", "f"];

const email = (id: string): EmailDetail => ({
  accountId: null,
  attachments: [],
  bcc: [],
  blobId: BlobId.parse("raw"),
  cc: [],
  draft: false,
  flagged: false,
  from: [{ email: "ana@example.com", name: null }],
  hasAttachment: false,
  html: cids.map((cid) => `<img src="cid:${cid}" alt="">`).join(""),
  id: EmailId.parse(id),
  inReplyTo: [],
  inlineParts: cids.map((cid) => ({
    blobId: BlobId.parse(`b-${cid}`),
    cid,
    name: `${cid}.png`,
    size: 4 * 1024 * 1024,
    type: "image/png",
  })),
  mailboxIds: [],
  messageId: [],
  preloadToken: null,
  preview: "",
  readReceipt: null,
  receivedAt: "2026-10-23T09:41:00Z",
  references: [],
  replyTo: [],
  senderVerdict: null,
  sentAt: null,
  size: 1000,
  subject: "Images",
  text: null,
  threadId: ThreadId.parse("t"),
  to: [],
  truncated: false,
  unread: false,
  unsubscribe: { mailto: null, oneClick: null, web: null },
});

describe(useRenderedEmail, () => {
  it("asks for each inline image once per email, whatever loads", async () => {
    // The budget lets two of the six through.
    images.loadInlineImages.mockResolvedValue(
      new Map([
        ["a", PIXEL],
        ["b", PIXEL],
      ])
    );
    const first = email("e1");
    const { rerender, result } = renderHook(
      ({ shown }: { shown: EmailDetail }) =>
        useRenderedEmail(shown, { remote: null, view: "html" }),
      { initialProps: { shown: first } }
    );
    await waitFor(() => {
      expect(result.current?.srcdoc).toContain(PIXEL);
    });
    rerender({ shown: first });
    rerender({ shown: { ...first } });
    expect(images.loadInlineImages).toHaveBeenCalledOnce();
    expect(images.loadInlineImages.mock.calls[0]?.[1]).toHaveLength(6);
    // Another email has its own budget.
    rerender({ shown: email("e2") });
    await waitFor(() => {
      expect(images.loadInlineImages).toHaveBeenCalledTimes(2);
    });
  });
});
