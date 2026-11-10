// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { AccountId, MailboxId } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import type { MailboxSummary } from "./api";
import { ALL_RIGHTS } from "./rights";
import {
  DEFAULT_SPLITS,
  combine,
  findSplit,
  listFilter,
  mailboxParam,
  resolveView,
} from "./views";

const box = (id: string, role: MailboxSummary["role"]): MailboxSummary => ({
  accountId: null,
  id: MailboxId.parse(id),
  mayShare: true,
  name: id,
  parentId: null,
  role,
  rights: ALL_RIGHTS,
  sharedWith: 0,
  total: 0,
  unread: 0,
});

const inbox = box("a1", "inbox");
const trash = box("a2", "trash");
const work = box("a3", null);
const mailboxes = [inbox, trash, work];

describe(resolveView, () => {
  it("resolves role aliases, ids and starred", () => {
    expect(resolveView("inbox", mailboxes)).toStrictEqual({
      kind: "mailbox",
      mailbox: inbox,
    });
    expect(resolveView("a3", mailboxes)).toStrictEqual({
      kind: "mailbox",
      mailbox: work,
    });
    expect(resolveView("starred", undefined)).toStrictEqual({
      kind: "starred",
    });
  });

  it("reports roles the account lacks and unknown ids", () => {
    expect(resolveView("archive", mailboxes)).toStrictEqual({
      kind: "missing",
      role: "archive",
    });
    expect(resolveView("nope", mailboxes)).toBeNull();
    expect(resolveView("inbox", undefined)).toBeNull();
  });

  it("round-trips with mailboxParam", () => {
    expect(mailboxParam(inbox)).toBe("inbox");
    expect(mailboxParam(work)).toBe("a3");
  });

  it("puts the account of shared folders in front", () => {
    const shared = { ...work, accountId: AccountId.parse("c") };
    const sharedInbox = { ...inbox, accountId: AccountId.parse("c") };
    expect(mailboxParam(shared)).toBe("c.a3");
    expect(mailboxParam(sharedInbox)).toBe("c.inbox");
    expect(resolveView("c.a3", [sharedInbox, shared])).toStrictEqual({
      kind: "mailbox",
      mailbox: shared,
    });
    expect(resolveView("c.starred", undefined)).toStrictEqual({
      kind: "starred",
    });
  });
});

describe(combine, () => {
  it("keeps one condition as is and ANDs several", () => {
    expect(combine([null, { hasAttachment: true }])).toStrictEqual({
      hasAttachment: true,
    });
    expect(
      combine([{ inMailbox: inbox.id }, { notKeyword: "$seen" }])
    ).toStrictEqual({
      conditions: [{ inMailbox: inbox.id }, { notKeyword: "$seen" }],
      operator: "AND",
    });
    expect(combine([])).toStrictEqual({});
  });
});

describe(listFilter, () => {
  const unreadSplit = findSplit(DEFAULT_SPLITS, "unread");

  it("applies splits only to the inbox", () => {
    expect(
      listFilter({ kind: "mailbox", mailbox: inbox }, { split: unreadSplit })
    ).toStrictEqual({
      conditions: [{ inMailbox: inbox.id }, { notKeyword: "$seen" }],
      operator: "AND",
    });
    expect(
      listFilter({ kind: "mailbox", mailbox: work }, { split: unreadSplit })
    ).toStrictEqual({ inMailbox: work.id });
  });

  it("adds the read filter", () => {
    expect(
      listFilter(
        { kind: "mailbox", mailbox: work },
        { readFilter: "attachment" }
      )
    ).toStrictEqual({
      conditions: [{ inMailbox: work.id }, { hasAttachment: true }],
      operator: "AND",
    });
  });

  it("starred leaves out trash and spam", () => {
    expect(
      listFilter({ kind: "starred" }, { excluded: [trash.id] })
    ).toStrictEqual({
      hasKeyword: "$flagged",
      inMailboxOtherThan: [trash.id],
    });
  });

  it("falls back to the first split for unknown ids", () => {
    expect(findSplit(DEFAULT_SPLITS, "nope").id).toBe("important");
  });
});
