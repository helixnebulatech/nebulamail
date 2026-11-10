// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { ContactCardId } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import type { ContactSummary } from "./api";
import { initialOf, searchContacts, sortContacts } from "./search";

const contact = (
  id: string,
  name: string,
  extra: Partial<ContactSummary> = {}
): ContactSummary => ({
  accountId: null,
  addressBookIds: [],
  emails: [],
  group: false,
  id: ContactCardId.parse(id),
  key: id,
  mayRemove: true,
  mayWrite: true,
  members: [],
  name,
  organization: "",
  owner: null,
  phones: [],
  uid: null,
  ...extra,
});

const carla = contact("a", "Carla Gómez", { emails: ["carla@example.com"] });
const bob = contact("b", "Bob Pérez", {
  emails: ["bob@nebulamail.test"],
  organization: "ACME Corp",
  phones: ["+34 600 11 22 33"],
});
const nameless = contact("c", "", { phones: ["555"] });

describe(searchContacts, () => {
  it("matches the start of words, without accents (unlike Stalwart)", () => {
    const all = [carla, bob, nameless];
    expect(searchContacts(all, "car").map((c) => c.id)).toStrictEqual(["a"]);
    expect(searchContacts(all, "gomez").map((c) => c.id)).toStrictEqual(["a"]);
    expect(searchContacts(all, "acme").map((c) => c.id)).toStrictEqual(["b"]);
    expect(searchContacts(all, "nebulamail").map((c) => c.id)).toStrictEqual([
      "b",
    ]);
  });

  it("finds phone numbers by their digits, and nothing for nonsense", () => {
    const all = [carla, bob, nameless];
    expect(searchContacts(all, "600 11").map((c) => c.id)).toStrictEqual(["b"]);
    expect(searchContacts(all, "zzz")).toStrictEqual([]);
    expect(searchContacts(all, "  ")).toHaveLength(3);
  });

  it("puts names that start with the query first", () => {
    const ana = contact("d", "Ana Bobadilla");
    expect(searchContacts([ana, bob], "bob").map((c) => c.id)).toStrictEqual([
      "b",
      "d",
    ]);
  });
});

describe(sortContacts, () => {
  it("sorts by name, nameless last", () => {
    expect(
      sortContacts([nameless, carla, bob], "es").map((c) => c.id)
    ).toStrictEqual(["b", "a", "c"]);
  });
});

describe(initialOf, () => {
  it("groups by first letter without accents", () => {
    expect(initialOf("Óscar")).toBe("O");
    expect(initialOf("1Password")).toBe("#");
    expect(initialOf("")).toBe("#");
  });
});

describe("search over a list fetched again", () => {
  it("sees a card's new name (the folded copy is per card object)", () => {
    const before = contact("1", "Carla Pérez");
    expect(searchContacts([before], "carl")).toHaveLength(1);
    const after = { ...before, name: "Marta Pérez" };
    expect(searchContacts([after], "carl")).toHaveLength(0);
    expect(searchContacts([after], "mart")).toHaveLength(1);
    // The old object still matches as it was.
    expect(searchContacts([before], "carl")).toHaveLength(1);
  });

  it("finds phone digits typed with spaces", () => {
    const card = contact("2", "Ana", { phones: ["+34 600 11 22 33"] });
    expect(searchContacts([card], "600 11")).toHaveLength(1);
  });
});
