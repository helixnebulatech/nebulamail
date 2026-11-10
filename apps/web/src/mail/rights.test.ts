// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { AccountId, MailboxId } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import type { MailboxRole, MailboxSummary } from "./api";
import type { MailRights } from "./rights";
import { ALL_RIGHTS, mailPermissions, toMailRights } from "./rights";

const NONE: MailRights = {
  mayAddItems: false,
  mayCreateChild: false,
  mayDelete: false,
  mayReadItems: false,
  mayRemoveItems: false,
  mayRename: false,
  maySetKeywords: false,
  maySetSeen: false,
};
const READ: MailRights = { ...NONE, mayReadItems: true };
const WRITE: MailRights = {
  ...READ,
  mayAddItems: true,
  mayRemoveItems: true,
  maySetKeywords: true,
  maySetSeen: true,
};

const shared = AccountId.parse("c");

const box = (
  id: string,
  role: MailboxRole | null,
  rights: MailRights = ALL_RIGHTS,
  accountId: AccountId | null = null
): MailboxSummary => ({
  accountId,
  id: MailboxId.parse(id),
  mayShare: false,
  name: id,
  parentId: null,
  rights,
  role,
  sharedWith: 0,
  total: 0,
  unread: 0,
});

describe(toMailRights, () => {
  it("allows everything when the server leaves the rights out", () => {
    expect(toMailRights(undefined)).toStrictEqual(ALL_RIGHTS);
    expect(toMailRights(null)).toStrictEqual(ALL_RIGHTS);
  });

  it("keeps what the server denies", () => {
    expect(
      toMailRights({ ...ALL_RIGHTS, mayRemoveItems: false, maySetSeen: false })
    ).toStrictEqual({
      ...ALL_RIGHTS,
      mayRemoveItems: false,
      maySetSeen: false,
    });
  });
});

describe(mailPermissions, () => {
  it("allows everything in our own folders", () => {
    const inbox = box("i", "inbox");
    const mailboxes = [inbox, box("d", "drafts"), box("t", "trash")];
    const permissions = mailPermissions(mailboxes, inbox);
    expect(permissions).toMatchObject({
      archive: true,
      move: true,
      setFlagged: true,
      setSeen: true,
      spam: true,
      trash: true,
    });
    // Never into Drafts, never into the same folder.
    expect(permissions.moveTargets.map((target) => target.id)).toStrictEqual([
      "t",
    ]);
  });

  it("offers nothing that changes a read-only shared folder", () => {
    const folder = box("s", null, READ, shared);
    const permissions = mailPermissions(
      [folder, box("u", null, WRITE, shared)],
      folder
    );
    expect(permissions).toStrictEqual({
      archive: false,
      move: false,
      moveTargets: [],
      notSpam: false,
      setFlagged: false,
      setSeen: false,
      spam: false,
      trash: false,
    });
  });

  it("lets mark but not move when only the flags may change", () => {
    const folder = box("s", null, {
      ...READ,
      maySetKeywords: true,
      maySetSeen: true,
    });
    const permissions = mailPermissions([folder, box("o", null)], folder);
    expect(permissions.setSeen).toBeTruthy();
    expect(permissions.setFlagged).toBeTruthy();
    expect(permissions.move).toBeFalsy();
    expect(permissions.trash).toBeFalsy();
  });

  it("only moves into shared folders that accept emails", () => {
    const folder = box("s", null, WRITE, shared);
    const permissions = mailPermissions(
      [folder, box("r", null, READ, shared), box("w", null, WRITE, shared)],
      folder
    );
    expect(permissions.moveTargets.map((target) => target.id)).toStrictEqual([
      "w",
    ]);
  });

  it("never creates Archive or Junk in a shared account", () => {
    const folder = box("s", null, WRITE, shared);
    const permissions = mailPermissions([folder], folder);
    expect(permissions.archive).toBeFalsy();
    expect(permissions.spam).toBeFalsy();
    expect(permissions.trash).toBeFalsy();
  });

  it("creates Archive and Junk in our own account when missing", () => {
    const inbox = box("i", "inbox");
    const permissions = mailPermissions([inbox], inbox);
    expect(permissions.archive).toBeTruthy();
    expect(permissions.spam).toBeTruthy();
    // No Trash folder: nothing to move to.
    expect(permissions.trash).toBeFalsy();
  });

  it("needs the target to accept emails and keywords for spam", () => {
    const folder = box("s", null, { ...WRITE, maySetKeywords: false }, shared);
    const permissions = mailPermissions(
      [
        folder,
        box("j", "junk", WRITE, shared),
        box("t", "trash", READ, shared),
      ],
      folder
    );
    expect(permissions.spam).toBeFalsy();
    expect(permissions.trash).toBeFalsy();
  });
});
