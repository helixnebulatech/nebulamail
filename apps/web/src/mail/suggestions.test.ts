// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { contactsFromCards } from "./contacts";
import { MAX_RECENT, addRecent, suggestRecipients } from "./suggestions";

describe(addRecent, () => {
  it("counts sends, keeps the latest name and sorts by last use", () => {
    const first = addRecent([], [{ email: "ana@x.com", name: "Ana" }], 1);
    const second = addRecent(
      first,
      [{ email: "ANA@x.com" }, { email: "bob@y.com", name: "Bob" }],
      2
    );
    expect(second).toStrictEqual([
      { count: 2, email: "ANA@x.com", lastUsed: 2, name: "Ana" },
      { count: 1, email: "bob@y.com", lastUsed: 2, name: "Bob" },
    ]);
  });

  it("keeps at most MAX_RECENT, dropping the oldest", () => {
    const many = Array.from({ length: MAX_RECENT + 5 }, (_, index) => ({
      email: `p${String(index)}@x.com`,
    }));
    let result = addRecent([], [], 0);
    for (const [index, recipient] of many.entries()) {
      result = addRecent(result, [recipient], index);
    }
    expect(result).toHaveLength(MAX_RECENT);
    expect(result.at(-1)?.email).toBe("p5@x.com");
  });
});

describe(suggestRecipients, () => {
  const contacts = [
    { email: "carla@example.com", name: "Carla Gómez" },
    { email: "jose@example.com", name: "José Pérez" },
    { email: "ana.lopez@corp.com", name: "" },
  ];
  const recent = [
    { count: 5, email: "carlos@x.com", lastUsed: 10, name: "Carlos" },
    { count: 1, email: "carla@example.com", lastUsed: 20, name: "C." },
  ];

  it("matches names and addresses by prefix, ignoring accents", () => {
    expect(
      suggestRecipients("car", { contacts, exclude: [], recent })
    ).toStrictEqual([
      { email: "carlos@x.com", name: "Carlos" },
      { email: "carla@example.com", name: "Carla Gómez" },
    ]);
    expect(
      suggestRecipients("perez", { contacts, exclude: [], recent }).map(
        (item) => item.email
      )
    ).toStrictEqual(["jose@example.com"]);
    expect(
      suggestRecipients("lopez", { contacts, exclude: [], recent }).map(
        (item) => item.email
      )
    ).toStrictEqual(["ana.lopez@corp.com"]);
  });

  it("leaves out who is already a recipient, and nothing for empty input", () => {
    expect(
      suggestRecipients("car", {
        contacts,
        exclude: [" CARLOS@x.com"],
        recent,
      }).map((item) => item.email)
    ).toStrictEqual(["carla@example.com"]);
    expect(
      suggestRecipients(" ", { contacts, exclude: [], recent })
    ).toStrictEqual([]);
  });

  it("follows new data: the index is per address book and history", () => {
    const first = suggestRecipients("c", { contacts, exclude: [], recent });
    expect(
      suggestRecipients("c", { contacts, exclude: [], recent })
    ).toStrictEqual(first);
    const newer = [
      ...recent,
      { count: 9, email: "zoe@zeta.example", lastUsed: 30, name: "Zoe" },
    ];
    expect(
      suggestRecipients("zo", { contacts, exclude: [], recent: newer }).map(
        (item) => item.email
      )
    ).toStrictEqual(["zoe@zeta.example"]);
    expect(
      suggestRecipients("zo", { contacts, exclude: [], recent })
    ).toStrictEqual([]);
  });
});

describe(contactsFromCards, () => {
  it("takes the full name, else the components, and every address", () => {
    expect(
      contactsFromCards([
        {
          emails: {
            b: { address: "b@x.com", pref: 2 },
            a: { address: " a@x.com ", pref: 1 },
          },
          name: { full: "Ana" },
        },
        {
          emails: { e: { address: "bob@y.com" } },
          name: {
            components: [
              { kind: "given", value: "Bob" },
              { kind: "separator", value: " " },
              { kind: "surname", value: "Ray" },
            ],
          },
        },
        { emails: null, name: { full: "No email" } },
      ])
    ).toStrictEqual([
      { email: "a@x.com", name: "Ana" },
      { email: "b@x.com", name: "Ana" },
      { email: "bob@y.com", name: "Bob Ray" },
    ]);
  });
});
