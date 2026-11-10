// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { EmailId, MailboxId, ThreadId } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import type { ConversationRow, ThreadMember } from "./api";
import type { ThreadContext } from "./threads";
import {
  actionTargets,
  conversationView,
  initialExpansion,
  isExpanded,
  isShown,
  latestEmail,
  participantNames,
  rowTargets,
  shownEmails,
} from "./threads";

const box = {
  archive: MailboxId.parse("archive"),
  drafts: MailboxId.parse("drafts"),
  inbox: MailboxId.parse("inbox"),
  junk: MailboxId.parse("junk"),
  sent: MailboxId.parse("sent"),
  trash: MailboxId.parse("trash"),
};

const contextFor = (mailboxId: MailboxId | null): ThreadContext => ({
  draftsId: box.drafts,
  junkId: box.junk,
  mailboxId,
  sentId: box.sent,
  trashId: box.trash,
});
const inInbox = contextFor(box.inbox);
const inSearch = contextFor(null);

const member = (
  id: string,
  mailbox: MailboxId,
  extra: Partial<ThreadMember> = {}
): ThreadMember => ({
  draft: false,
  flagged: false,
  from: [{ email: `${id}@x.com`, name: id.toUpperCase() }],
  hasAttachment: false,
  id: EmailId.parse(id),
  mailboxIds: [mailbox],
  receivedAt: "2026-10-24T10:00:00Z",
  unread: false,
  ...extra,
});

// Bob writes, I reply (Sent), Bob answers (unread), an old one is trashed,
// and I have a draft reply.
const ana = member("ana", box.trash);
const bob1 = member("bob1", box.inbox, {
  from: [{ email: "bob@x.com", name: "Bob Smith" }],
});
const mine = member("mine", box.sent, {
  from: [{ email: "me@home.com", name: "Me" }],
});
const bob2 = member("bob2", box.inbox, {
  from: [{ email: "BOB@x.com", name: "Bob" }],
  hasAttachment: true,
  unread: true,
});
const draft = member("draft", box.drafts, { draft: true });
const thread = [ana, bob1, mine, bob2, draft];

const row = (members: readonly ThreadMember[], own: ThreadMember) =>
  ({
    ...own,
    members,
    preview: "",
    subject: "Hola",
    threadId: ThreadId.parse("t1"),
    to: [],
  }) satisfies ConversationRow;

const id = (value: string) => EmailId.parse(value);

describe(isShown, () => {
  it("shows your replies in Sent but hides Trash and Spam in a folder", () => {
    expect(shownEmails(thread, inInbox).map((email) => email.id)).toStrictEqual(
      ["bob1", "mine", "bob2", "draft"]
    );
    expect(isShown(member("spam", box.junk), inSearch)).toBeFalsy();
  });

  it("shows only what is in Trash when viewing Trash (or Spam in Spam)", () => {
    expect(
      shownEmails(thread, contextFor(box.trash)).map((email) => email.id)
    ).toStrictEqual(["ana"]);
    expect(isShown(bob1, contextFor(box.junk))).toBeFalsy();
  });

  it("always keeps the opened email, even if trashed", () => {
    expect(
      shownEmails(thread, inSearch, id("ana")).map((email) => email.id)
    ).toContain("ana");
  });
});

describe(latestEmail, () => {
  it("skips drafts", () => {
    expect(latestEmail(thread)?.id).toBe("bob2");
    expect(latestEmail([draft])?.id).toBe("draft");
  });
});

describe(conversationView, () => {
  it("counts shown emails without drafts and merges senders", () => {
    const view = conversationView(row(thread, bob2), inInbox);

    expect(view.count).toBe(3);
    expect(view.unread).toBeTruthy();
    expect(view.flagged).toBeFalsy();
    expect(view.hasAttachment).toBeTruthy();
    expect(view.participants.map((person) => person.email)).toStrictEqual([
      "bob@x.com",
      "me@home.com",
    ]);
  });

  it("falls back to the row itself when its thread isn't known", () => {
    expect(conversationView(row([], bob1), inInbox).count).toBe(1);
  });
});

describe(participantNames, () => {
  it("lists up to three, then the first and the last two", () => {
    expect(participantNames(["Ana", "Bob", "me"])).toBe("Ana, Bob, me");
    expect(participantNames(["Ana", "Bob", "Carla", "me"])).toBe(
      "Ana … Carla, me"
    );
  });
});

describe(actionTargets, () => {
  it("moves only what is in the open folder: your replies stay in Sent", () => {
    expect(actionTargets(thread, inInbox, "move", id("bob2"))).toStrictEqual([
      "bob1",
      "bob2",
    ]);
  });

  it("outside a folder, moves what isn't in Sent or Drafts", () => {
    expect(actionTargets(thread, inSearch, "move", id("bob2"))).toStrictEqual([
      "bob1",
      "bob2",
    ]);
  });

  it("falls back to the opened email when nothing else qualifies", () => {
    expect(actionTargets([mine], inSearch, "move", id("mine"))).toStrictEqual([
      "mine",
    ]);
  });

  it("reads all unread, marks only the newest unread, like Gmail", () => {
    expect(actionTargets(thread, inInbox, "read", id("bob1"))).toStrictEqual([
      "bob2",
    ]);
    expect(actionTargets(thread, inInbox, "unread", id("bob1"))).toStrictEqual([
      "bob2",
    ]);
  });

  it("stars the newest and unstars every starred one", () => {
    const starred = [
      member("a", box.inbox, { flagged: true }),
      member("b", box.sent, { flagged: true }),
      member("c", box.inbox),
    ];

    expect(actionTargets(starred, inInbox, "flag", id("a"))).toStrictEqual([
      "c",
    ]);
    expect(actionTargets(starred, inInbox, "unflag", id("c"))).toStrictEqual([
      "a",
      "b",
    ]);
  });
});

describe(rowTargets, () => {
  it("joins the targets of every selected row, once each", () => {
    const other = member("solo", box.inbox);

    expect(
      rowTargets([row(thread, bob2), row([other], other)], inInbox, "move")
    ).toStrictEqual(["bob1", "bob2", "solo"]);
  });
});

describe(initialExpansion, () => {
  it("opens the unread ones, the newest and the opened one; not drafts", () => {
    const shown = shownEmails(thread, inInbox);
    const expansion = initialExpansion(shown, id("bob1"));

    expect([...expansion.open]).toStrictEqual(["bob1", "bob2"]);
  });
});

describe(isExpanded, () => {
  const shown = shownEmails(thread, inInbox);
  const expansion = initialExpansion(shown, id("bob2"));
  const none = new Map<EmailId, boolean>();

  it("keeps the first choice after the emails are marked read", () => {
    const read = { ...bob2, unread: false };

    expect(isExpanded(read, id("bob2"), expansion, none)).toBeTruthy();
    expect(isExpanded(mine, id("bob2"), expansion, none)).toBeFalsy();
  });

  it("follows the user's clicks", () => {
    expect(
      isExpanded(mine, id("bob2"), expansion, new Map([[id("mine"), true]]))
    ).toBeTruthy();
    expect(
      isExpanded(bob2, id("bob2"), expansion, new Map([[id("bob2"), false]]))
    ).toBeFalsy();
  });

  it("opens a reply that arrives while open (push) if unread or newest", () => {
    const arrived = member("new", box.inbox, { unread: true });
    const readNew = member("new2", box.inbox);

    expect(isExpanded(arrived, id("new"), expansion, none)).toBeTruthy();
    expect(isExpanded(readNew, id("new2"), expansion, none)).toBeTruthy();
    expect(isExpanded(readNew, id("other"), expansion, none)).toBeFalsy();
  });

  it("never opens a draft (it opens in the composer)", () => {
    expect(
      isExpanded(draft, id("draft"), expansion, new Map([[id("draft"), true]]))
    ).toBeFalsy();
  });
});
