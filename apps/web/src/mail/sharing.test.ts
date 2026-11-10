// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { AccountId, PrincipalId } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import type { Person } from "./sharing";
import {
  SHARE_LEVELS,
  isGroup,
  levelOf,
  matchPeople,
  mergeMatches,
  personDetail,
  rightsFor,
  serverSearchFilter,
  sharedMailAccounts,
} from "./sharing";

const MAIL = { "urn:ietf:params:jmap:mail": {} };

describe(sharedMailAccounts, () => {
  it("lists the other accounts with mail, by name", () => {
    expect(
      sharedMailAccounts(
        {
          [AccountId.parse("d")]: { accountCapabilities: MAIL, name: "bob" },
          [AccountId.parse("c")]: { accountCapabilities: MAIL, name: "alice" },
          [AccountId.parse("f")]: {
            accountCapabilities: { "urn:ietf:params:jmap:contacts": {} },
            name: "contacts only",
          },
          [AccountId.parse("e")]: { accountCapabilities: MAIL, name: "ana" },
        },
        AccountId.parse("d")
      )
    ).toStrictEqual([
      { accountId: "c", name: "alice" },
      { accountId: "e", name: "ana" },
    ]);
  });
});

describe(rightsFor, () => {
  it("never lets others send as us or re-share", () => {
    for (const level of SHARE_LEVELS) {
      expect(rightsFor(level)).toMatchObject({
        mayShare: false,
        maySubmit: false,
      });
    }
  });

  it("grows from read to manage", () => {
    expect(rightsFor("read")).toMatchObject({
      mayReadItems: true,
      maySetSeen: false,
    });
    expect(rightsFor("write")).toMatchObject({
      mayDelete: false,
      mayRemoveItems: true,
      maySetSeen: true,
    });
    expect(rightsFor("manage")).toMatchObject({
      mayDelete: true,
      mayRename: true,
    });
  });
});

describe(levelOf, () => {
  it("round-trips the levels and reports anything else as custom", () => {
    for (const level of SHARE_LEVELS) {
      expect(levelOf(rightsFor(level))).toBe(level);
    }
    expect(levelOf({ ...rightsFor("read"), maySubmit: true })).toBe("custom");
    // A server without mayShare in the rights.
    const { mayShare: _ignored, ...withoutShare } = rightsFor("write");
    expect(levelOf(withoutShare)).toBe("write");
  });
});

describe(matchPeople, () => {
  const people: Person[] = [
    {
      email: "bob@nebulamail.test",
      id: PrincipalId.parse("d"),
      name: "bob@nebulamail.test",
      type: "individual",
    },
    {
      email: "jose@x.com",
      id: PrincipalId.parse("e"),
      name: "José Núñez",
      type: "individual",
    },
  ];

  it("matches parts of names and addresses, without accents", () => {
    expect(matchPeople(people, "bo").map((person) => person.id)).toStrictEqual([
      "d",
    ]);
    expect(matchPeople(people, "jose nun").map((p) => p.id)).toStrictEqual([
      "e",
    ]);
    expect(matchPeople(people, "")).toHaveLength(2);
  });

  it("leaves out people already in the list", () => {
    expect(
      matchPeople(people, "", new Set([PrincipalId.parse("d")]))
    ).toStrictEqual([people[1]]);
  });
});

describe(serverSearchFilter, () => {
  it("looks up an address exactly and anything else by name", () => {
    expect(serverSearchFilter(" dan@outside.example ")).toStrictEqual({
      email: "dan@outside.example",
    });
    expect(serverSearchFilter("Staf")).toStrictEqual({ text: "Staf" });
  });

  it("doesn't ask about too little, too much or control characters", () => {
    expect(serverSearchFilter("")).toBeNull();
    expect(serverSearchFilter(" s ")).toBeNull();
    expect(serverSearchFilter("x".repeat(65))).toBeNull();
    expect(serverSearchFilter("St\naff")).toBeNull();
  });
});

const groupPerson = (id: string, name: string): Person => ({
  email: null,
  id: PrincipalId.parse(id),
  name,
  type: "group",
});

describe(mergeMatches, () => {
  it("puts local matches first, adds what only the server found", () => {
    const bob = groupPerson("r1", "Bob");
    const staff = groupPerson("r2", "Staff");
    const shared = groupPerson("r3", "Sales");
    expect(
      mergeMatches([bob], [bob, staff, shared], new Set([shared.id]), 8).map(
        (p) => p.name
      )
    ).toStrictEqual(["Bob", "Staff"]);
    expect(mergeMatches([bob], [staff], new Set(), 1)).toStrictEqual([bob]);
  });
});

describe(personDetail, () => {
  const ventas = {
    email: null,
    id: PrincipalId.parse("pg"),
    name: "Ventas",
    type: "group",
  } satisfies Person;

  it("says a group is one, with or without an address", () => {
    expect(isGroup(ventas)).toBeTruthy();
    expect(personDetail(ventas, "Grupo")).toBe("Grupo");
    expect(
      personDetail({ ...ventas, email: "ventas@example.com" }, "Grupo")
    ).toBe("Grupo · ventas@example.com");
  });

  it("shows a person's address unless it is the name", () => {
    const bob = {
      email: "bob@example.com",
      id: PrincipalId.parse("pb"),
      name: "Bob",
      type: "individual",
    } satisfies Person;
    expect(isGroup(bob)).toBeFalsy();
    expect(personDetail(bob, "Grupo")).toBe("bob@example.com");
    expect(
      personDetail({ ...bob, name: "bob@example.com" }, "Grupo")
    ).toBeNull();
    expect(personDetail({ ...bob, email: null }, "Grupo")).toBeNull();
  });
});
