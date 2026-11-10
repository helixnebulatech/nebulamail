// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import "fake-indexeddb/auto";
import { BlobId, EmailId, IdentityId } from "@nebulamail/jmap-client";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Draft } from "./compose";
import {
  deleteBackup,
  forgetBackups,
  listBackups,
  newBackupId,
  parseBackup,
  saveBackup,
} from "./draft-backup";

// null: no account known (opened offline with nothing saved).
const account = vi.hoisted(() => ({
  current: "https://mail.test alice" as string | null,
}));

vi.mock(import("./account-key"), () => ({
  accountKey: async () => {
    await Promise.resolve();
    if (account.current === null) {
      throw new TypeError("Failed to fetch");
    }
    return account.current;
  },
  connectedAccountKey: async () => await Promise.reject(new Error("unused")),
}));

const draft: Draft = {
  attachments: [
    {
      blobId: BlobId.parse("B1"),
      name: "a.pdf",
      size: 3,
      type: "application/pdf",
    },
  ],
  bcc: [],
  cc: [{ email: "c@x.com" }],
  html: "<p>Hola</p>",
  identity: {
    email: "alice@x.com",
    id: IdentityId.parse("i1"),
    name: "Alice",
    signature: null,
  },
  inReplyTo: ["m1@x.com"],
  subject: "Asunto",
  to: [{ email: "bob@x.com", name: "Bob" }],
};

describe("draft backups", () => {
  beforeEach(async () => {
    account.current = "https://mail.test alice";
    await forgetBackups();
  });

  it("keeps a draft until it is deleted, per account", async () => {
    await saveBackup({ draft, draftId: EmailId.parse("e1"), id: "w1" });
    await saveBackup({ draft: { ...draft, subject: "Otro" }, id: "w2" });
    account.current = "https://mail.test bob";
    await saveBackup({ draft, id: "w3" });
    await expect(listBackups()).resolves.toHaveLength(1);

    account.current = "https://mail.test alice";
    const backups = await listBackups();
    expect(backups.map((backup) => backup.id).toSorted()).toStrictEqual([
      "w1",
      "w2",
    ]);
    expect(backups.find((backup) => backup.id === "w1")).toMatchObject({
      draft,
      draftId: "e1",
    });

    await deleteBackup("w1");
    const left = await listBackups();
    expect(left.map((backup) => backup.id)).toStrictEqual(["w2"]);
  });

  it("replaces the copy of the same window", async () => {
    await saveBackup({ draft, id: "w1" });
    await saveBackup({ draft: { ...draft, html: "<p>Más</p>" }, id: "w1" });
    const backups = await listBackups();
    expect(backups).toHaveLength(1);
    expect(backups[0]?.draft.html).toBe("<p>Más</p>");
  });

  it("keeps a draft written offline before the sender or account was known", async () => {
    account.current = null;
    const offline = { ...draft, identity: null };
    await saveBackup({ draft: offline, id: "w1" });
    // Offered to whoever connects next in this browser.
    account.current = "https://mail.test alice";
    const backups = await listBackups();
    expect(backups).toHaveLength(1);
    expect(backups[0]?.draft).toStrictEqual(offline);
  });

  it("forgets everything on logout", async () => {
    await saveBackup({ draft, id: "w1" });
    await forgetBackups();
    await expect(listBackups()).resolves.toStrictEqual([]);
  });
});

describe(newBackupId, () => {
  it("is 32 random hex characters", () => {
    const id = newBackupId();
    expect(id).toMatch(/^[\da-f]{32}$/u);
    expect(newBackupId()).not.toBe(id);
  });
});

describe(parseBackup, () => {
  it("ignores records it can't use", () => {
    expect(parseBackup({ id: "x" })).toBeNull();
    expect(parseBackup("nope")).toBeNull();
    expect(
      parseBackup({ account: "a", draft, id: "w", savedAt: 1 })
    ).toMatchObject({ id: "w" });
  });
});
