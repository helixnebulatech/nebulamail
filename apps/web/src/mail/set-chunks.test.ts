// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// @vitest-environment jsdom

// RFC 8620 §5.3: a /set call carries at most `maxObjectsInSet` objects.
// Bulk changes (mark, flag, move, delete contacts, import contacts, the
// calendar trash) go in several calls, and a partial failure is reported.

import {
  AddressBookId,
  ContactCardId,
  EmailId,
  MailboxId,
} from "@nebulamail/jmap-client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { changeTrash } from "../calendar/nextcloud";
import { deleteContacts, importContacts } from "../contacts/api";
import { methodError, scriptedJmap } from "../test/scripted-jmap";
import type { Handler, Scripted } from "../test/scripted-jmap";
import { EmailUpdateError, moveTo, setSeen } from "./api";

const jmap = vi.hoisted(() => ({
  connect: null as null | (() => Promise<unknown>),
}));

vi.mock(import("./client"), async () => {
  const { JmapClient } = await import("@nebulamail/jmap-client");
  return {
    getClient: async () => {
      const client = await jmap.connect?.();
      if (!(client instanceof JmapClient)) {
        throw new TypeError("no scripted client");
      }
      return client;
    },
    resetClient: vi.fn<() => void>(),
  };
});

const setBase = { newState: "2", oldState: "1" };

const record = (value: unknown): Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value)
    ? Object.fromEntries(Object.entries(value))
    : {};

const strings = (value: unknown): string[] =>
  Array.isArray(value)
    ? value.filter((item): item is string => typeof item === "string")
    : [];

const failedIds = (error: unknown): readonly EmailId[] | null =>
  error instanceof EmailUpdateError ? error.failed : null;

/** A /set handler: `refuse` ids end in notUpdated/notDestroyed. */
const setHandler =
  (name: string, refuse: ReadonlySet<string> = new Set()): Handler =>
  (args, callId) => {
    const update = record(args.update);
    const destroy = strings(args.destroy);
    const create = record(args.create);
    const restore = strings(args.restore);
    const error = { type: "forbidden" };
    const split = (ids: readonly string[]) => ({
      bad: Object.fromEntries(
        ids.filter((id) => refuse.has(id)).map((id) => [id, error])
      ),
      good: ids.filter((id) => !refuse.has(id)),
    });
    const updated = split(Object.keys(update));
    const destroyed = split(destroy);
    const restored = split(restore);
    return [
      [
        name,
        {
          ...setBase,
          accountId: args.accountId,
          created: Object.fromEntries(
            Object.keys(create).map((key) => [key, { id: `new-${key}` }])
          ),
          destroyed: destroyed.good,
          notDestroyed: destroyed.bad,
          notRestored: restored.bad,
          notUpdated: updated.bad,
          restored: restored.good,
          updated: Object.fromEntries(updated.good.map((id) => [id, null])),
        },
        callId,
      ],
    ];
  };

const setup = (
  handlers: Record<string, Handler>,
  maxObjectsInSet = 2
): Scripted => {
  const scripted = scriptedJmap({ handlers }, { maxObjectsInSet });
  jmap.connect = scripted.connect;
  return scripted;
};

const sizes = (scripted: Scripted, name: string, key: string): number[] =>
  scripted.calls
    .filter(([method]) => method === name)
    .map(([, args]) => {
      const value = args[key];
      return Array.isArray(value)
        ? value.length
        : Object.keys(record(value)).length;
    });

const emailIds = (count: number): EmailId[] =>
  Array.from({ length: count }, (_, index) => EmailId.parse(`e${index}`));

describe("Email/set in chunks of maxObjectsInSet", () => {
  afterEach(() => {
    jmap.connect = null;
  });

  it("marks 5 emails in calls of 2, 2 and 1", async () => {
    const scripted = setup({ "Email/set": setHandler("Email/set") });
    await setSeen(emailIds(5), true);
    expect(sizes(scripted, "Email/set", "update")).toStrictEqual([2, 2, 1]);
    // Every id once, with the same patch.
    const all = scripted.calls.flatMap(([, args]) =>
      Object.entries(record(args.update))
    );
    expect(all.map(([id]) => id)).toStrictEqual(emailIds(5).map(String));
    expect(new Set(all.map(([, patch]) => JSON.stringify(patch))).size).toBe(1);
  });

  it("sends nothing for no emails", async () => {
    const scripted = setup({ "Email/set": setHandler("Email/set") });
    await setSeen([], true);
    expect(scripted.calls).toHaveLength(0);
  });

  it("reports the emails the server didn't update, after trying them all", async () => {
    const scripted = setup({
      "Email/set": setHandler("Email/set", new Set(["e1", "e4"])),
    });
    const error = await moveTo(emailIds(5), MailboxId.parse("trash")).catch(
      (error_: unknown) => error_
    );
    expect(failedIds(error)).toStrictEqual(["e1", "e4"]);
    expect(sizes(scripted, "Email/set", "update")).toStrictEqual([2, 2, 1]);
  });

  it("goes on after a chunk fails as a whole", async () => {
    let call = 0;
    const scripted = setup({
      "Email/set": (args, callId) => {
        call += 1;
        return call === 2
          ? [methodError("serverFail", callId)]
          : setHandler("Email/set")(args, callId);
      },
    });
    const error = await setSeen(emailIds(5), false).catch(
      (error_: unknown) => error_
    );
    expect(failedIds(error)).toStrictEqual(["e2", "e3"]);
    expect(sizes(scripted, "Email/set", "update")).toStrictEqual([2, 2, 1]);
  });

  it("rethrows the server's error when nothing changed", async () => {
    setup({
      "Email/set": (_args, callId) => [methodError("serverFail", callId)],
    });
    await expect(setSeen(emailIds(3), true)).rejects.not.toBeInstanceOf(
      EmailUpdateError
    );
  });
});

describe("other bulk /set calls", () => {
  afterEach(() => {
    jmap.connect = null;
  });

  it("deletes contacts in chunks and fails if any wasn't deleted", async () => {
    const ids = ["c0", "c1", "c2"].map((id) => ContactCardId.parse(id));
    const refs = ids.map((id) => ({ accountId: null, id }));
    const ok = setup({ "ContactCard/set": setHandler("ContactCard/set") });
    await deleteContacts(refs);
    expect(sizes(ok, "ContactCard/set", "destroy")).toStrictEqual([2, 1]);

    const bad = setup({
      "ContactCard/set": setHandler("ContactCard/set", new Set(["c0"])),
    });
    await expect(deleteContacts(refs)).rejects.toThrow(Error);
    // The other chunks still went.
    expect(sizes(bad, "ContactCard/set", "destroy")).toStrictEqual([2, 1]);
  });

  it("imports contacts in chunks and counts what was created", async () => {
    const scripted = setup({
      "ContactCard/set": setHandler("ContactCard/set"),
    });
    const cards = Array.from({ length: 5 }, (_, index) => ({
      name: { full: `Person ${index}` },
    }));
    await expect(
      importContacts(cards, {
        accountId: null,
        id: AddressBookId.parse("book"),
      })
    ).resolves.toStrictEqual({ created: 5, failed: 0 });
    expect(sizes(scripted, "ContactCard/set", "create")).toStrictEqual([
      2, 2, 1,
    ]);
  });

  it("empties the calendar trash in chunks", async () => {
    const scripted = setup({
      "CalendarTrash/set": setHandler("CalendarTrash/set"),
    });
    await changeTrash("destroy", ["t0", "t1", "t2", "t3", "t4"]);
    await changeTrash("restore", ["t0", "t1", "t2"]);
    expect(sizes(scripted, "CalendarTrash/set", "destroy")).toStrictEqual([
      2, 2, 1, 0, 0,
    ]);
    expect(sizes(scripted, "CalendarTrash/set", "restore")).toStrictEqual([
      0, 0, 0, 2, 1,
    ]);
  });
});
