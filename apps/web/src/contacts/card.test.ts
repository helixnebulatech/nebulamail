// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { AddressBookId, BlobId, ContactCardId } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import { isValidEmail } from "../mail/compose";
import type { CardFields } from "./card";
import {
  birthdayInput,
  birthdayOf,
  cardToForm,
  cleanMultiline,
  displayName,
  emptyForm,
  formProblem,
  formProperties,
  importedCard,
  newCard,
  parseBirthday,
  photoOf,
} from "./card";

const id = ContactCardId.parse("c1");
const book = AddressBookId.parse("b");

/** As Stalwart returns a card created by another client. */
const card: CardFields = {
  addresses: {
    a1: {
      components: [
        { kind: "name", value: "Calle Mayor 1" },
        { kind: "locality", value: "Madrid" },
        { kind: "postcode", value: "28001" },
        { kind: "country", value: "España" },
      ],
      contexts: { private: true },
      isOrdered: true,
    },
  },
  anniversaries: {
    b1: {
      date: { "@type": "PartialDate", day: 17, month: 5, year: 1990 },
      kind: "birth",
    },
    w1: { date: { "@type": "PartialDate", day: 1, month: 6 }, kind: "wedding" },
  },
  emails: {
    e1: { address: "probe@example.com", contexts: { work: true }, pref: 2 },
    e2: { address: "first@example.com", label: "Casa", pref: 1 },
  },
  id,
  kind: "individual",
  links: { l1: { uri: "https://example.com" } },
  media: {
    logo: { kind: "logo", uri: "https://example.com/logo.png" },
    m1: { kind: "photo", uri: "https://example.com/p.jpg" },
  },
  name: {
    components: [
      { kind: "given", value: "Test" },
      { kind: "surname", value: "Probe" },
    ],
    full: "Test Probe",
    isOrdered: true,
  },
  notes: { n1: { note: "Hola <b>nota</b>" } },
  organizations: { o1: { name: "ACME" } },
  phones: { p1: { features: { mobile: true }, number: "+34 600 000 000" } },
  titles: { t1: { kind: "title", name: "Jefa" } },
};

describe(displayName, () => {
  it("prefers the full name, then parts, organization and address", () => {
    expect(displayName(card)).toBe("Test Probe");
    expect(
      displayName({ ...card, name: { components: card.name?.components } })
    ).toBe("Test Probe");
    expect(displayName({ id, organizations: { o: { name: "ACME" } } })).toBe(
      "ACME"
    );
    expect(displayName({ emails: { e: { address: "a@x.com" } }, id })).toBe(
      "a@x.com"
    );
    expect(displayName({ id })).toBe("");
  });

  it("cleans bidi and invisible characters", () => {
    expect(displayName({ id, name: { full: "Ana‮gnp.exe​" } })).toBe(
      "Anagnp.exe"
    );
  });
});

describe(cleanMultiline, () => {
  it("keeps line breaks but not controls", () => {
    expect(cleanMultiline("Hola‮\r\nadiós\n\n\n\nfin\u0007")).toBe(
      "Hola\nadiós\n\nfin"
    );
  });
});

describe(photoOf, () => {
  it("shows raster data: images and marks http(s) photos as remote", () => {
    expect(photoOf(card)).toStrictEqual({
      kind: "remote",
      url: "https://example.com/p.jpg",
    });
    expect(
      photoOf({
        id,
        media: {
          m: { kind: "photo", uri: "data:image/png;base64,iVBORw0KGgo=" },
        },
      })
    ).toStrictEqual({
      kind: "data",
      uri: "data:image/png;base64,iVBORw0KGgo=",
    });
  });

  it("shows a raster blob photo (Stalwart 1.0), never an SVG one", () => {
    const blobId = BlobId.parse("Gphoto");
    expect(
      photoOf({
        id,
        media: { m: { blobId, kind: "photo", mediaType: "image/JPEG" } },
      })
    ).toStrictEqual({ blobId, kind: "blob", mediaType: "image/jpeg" });
    for (const mediaType of ["image/svg+xml", "text/html", null]) {
      expect(
        photoOf({ id, media: { m: { blobId, kind: "photo", mediaType } } })
      ).toBeNull();
    }
  });

  it("never shows SVG, scripts or broken data", () => {
    for (const uri of [
      "data:image/svg+xml;base64,PHN2Zz4=",
      ["javascript", "alert(1)"].join(":"),
      "data:image/png;base64,<script>",
      "data:text/html;base64,PGI+",
      "//example.com/p.jpg",
    ]) {
      expect(photoOf({ id, media: { m: { kind: "photo", uri } } })).toBeNull();
    }
  });
});

describe("dates", () => {
  it("reads partial and full birthdays", () => {
    expect(birthdayOf(card)).toStrictEqual({
      calendarScale: null,
      day: 17,
      month: 5,
      year: 1990,
    });
    expect(
      birthdayOf({
        anniversaries: {
          b: {
            date: { "@type": "PartialDate", day: 2, month: 3 },
            kind: "birth",
          },
        },
        id,
      })
    ).toStrictEqual({ calendarScale: null, day: 2, month: 3, year: null });
    expect(
      birthdayOf({
        anniversaries: {
          b: {
            date: { "@type": "Timestamp", utc: "1985-03-02T00:00:00Z" },
            kind: "birth",
          },
        },
        id,
      })
    ).toStrictEqual({ calendarScale: null, day: 2, month: 3, year: 1985 });
  });

  it("reads the calendar of a date", () => {
    expect(
      birthdayOf({
        anniversaries: {
          b: {
            date: {
              "@type": "PartialDate",
              calendarScale: "Hebrew",
              day: 10,
              month: 13,
            },
            kind: "birth",
          },
        },
        id,
      })
    ).toStrictEqual({
      calendarScale: "hebrew",
      day: 10,
      month: 13,
      year: null,
    });
    expect(
      birthdayOf({
        anniversaries: {
          b: {
            date: { calendarScale: "gregorian", day: 1, month: 13 },
            kind: "birth",
          },
        },
        id,
      })
    ).toBeNull();
  });

  it("edits birthdays with and without year", () => {
    const yearless = { calendarScale: null, day: 24, month: 12, year: null };
    expect(birthdayInput(yearless)).toStrictEqual({
      calendarScale: null,
      day: "24",
      month: "12",
      year: "",
    });
    expect(parseBirthday(birthdayInput(yearless))).toStrictEqual(yearless);
    const input = { calendarScale: null, day: "", month: "", year: "" };
    expect(parseBirthday(input)).toBeNull();
    expect(
      parseBirthday({ ...input, day: "7", month: "1", year: "2001" })
    ).toStrictEqual({ calendarScale: null, day: 7, month: 1, year: 2001 });
  });

  it("refuses birthdays that don't exist or are half typed", () => {
    const input = { calendarScale: null, day: "", month: "", year: "" };
    // 29 February: fine without a year or in a leap year only.
    expect(parseBirthday({ ...input, day: "29", month: "2" })).toMatchObject({
      day: 29,
    });
    expect(
      parseBirthday({ ...input, day: "29", month: "2", year: "2001" })
    ).toBe("invalid");
    expect([
      parseBirthday({ ...input, day: "31", month: "4" }),
      parseBirthday({ ...input, day: "3" }),
    ]).toStrictEqual(["invalid", "invalid"]);
    expect(parseBirthday({ ...input, year: "1990" })).toBe("invalid");
    expect(
      parseBirthday({ ...input, day: "3", month: "4", year: "19x0" })
    ).toBe("invalid");
  });

  it("keeps an unchanged birthday as it was, calendar and all", () => {
    const hebrew: CardFields = {
      anniversaries: {
        x9: {
          date: {
            "@type": "PartialDate",
            calendarScale: "hebrew",
            day: 10,
            month: 6,
            year: 5784,
          },
          kind: "birth",
          place: { full: "Haifa" },
        },
      },
      id,
      name: { full: "Dana" },
    };
    const form = cardToForm(hebrew);
    expect(formProperties(form, hebrew).anniversaries).toStrictEqual(
      hebrew.anniversaries
    );
    const changed = formProperties(
      { ...form, birthday: { ...form.birthday, day: "11" } },
      hebrew
    );
    expect(Object.values(changed.anniversaries ?? {})).toStrictEqual([
      {
        date: {
          "@type": "PartialDate",
          calendarScale: "hebrew",
          day: 11,
          month: 6,
          year: 5784,
        },
        kind: "birth",
      },
    ]);
  });

  it("writes a birthday without year without a year", () => {
    const created = newCard(
      {
        ...emptyForm({ given: "Ana" }),
        birthday: { calendarScale: null, day: "24", month: "12", year: "" },
      },
      book,
      "urn:uuid:x"
    );
    expect(Object.values(created.anniversaries ?? {})).toStrictEqual([
      { date: { "@type": "PartialDate", day: 24, month: 12 }, kind: "birth" },
    ]);
  });

  it("refuses a half-typed birthday", () => {
    expect(
      formProblem(
        {
          ...emptyForm({ given: "Ana" }),
          birthday: { calendarScale: null, day: "4", month: "", year: "" },
        },
        isValidEmail
      )
    ).toBe("invalid-birthday");
  });
});

describe("editing", () => {
  it("reads a card into the form, emails by preference", () => {
    const form = cardToForm(card);
    expect(form).toMatchObject({
      full: "",
      given: "Test",
      note: "Hola <b>nota</b>",
      organization: "ACME",
      surname: "Probe",
      title: "Jefa",
    });
    expect(form.emails.map((row) => [row.key, row.place])).toStrictEqual([
      ["e2", "other"],
      ["e1", "work"],
    ]);
    expect(form.phones[0]).toMatchObject({ key: "p1", kind: "mobile" });
    expect(form.addresses[0]).toMatchObject({
      country: "España",
      locality: "Madrid",
      place: "home",
      street: "Calle Mayor 1",
    });
  });

  it("keeps keys, labels and other anniversaries and media on save", () => {
    const form = cardToForm(card);
    const patch = formProperties(
      {
        ...form,
        emails: [
          ...form.emails,
          { address: " new@example.com ", key: null, place: "home" },
        ],
        phones: [],
      },
      card
    );
    expect(patch).toMatchObject({
      emails: {
        e1: { address: "probe@example.com", contexts: { work: true }, pref: 2 },
        e2: { address: "first@example.com", label: "Casa", pref: 1 },
        e3: { address: "new@example.com", contexts: { private: true } },
      },
      phones: null,
    });
    expect(Object.keys(patch.anniversaries ?? {})).toHaveLength(2);
    // Untouched properties aren't in the patch; an unchanged photo leaves
    // all media (the logo too) as the server has them.
    expect(patch).not.toHaveProperty("links");
    expect(patch).not.toHaveProperty("media");
  });

  it("keeps a blob photo exactly as it was unless it changes", () => {
    const blobId = BlobId.parse("Gphoto");
    const photo = {
      blobId,
      kind: "photo",
      mediaType: "image/jpeg",
      pref: 1,
    } as const;
    const stored = { ...card, media: { p1: photo } };
    expect(
      formProperties({ ...cardToForm(stored), note: "x" }, stored)
    ).not.toHaveProperty("media");
    const chosen = "data:image/jpeg;base64,/9j/4AAQ";
    expect(
      formProperties(
        { ...cardToForm(stored), photo: { kind: "data", uri: chosen } },
        stored
      ).media
    ).toStrictEqual({
      m1: { kind: "photo", mediaType: "image/jpeg", uri: chosen },
    });
    expect(
      formProperties({ ...cardToForm(stored), photo: null }, stored).media
    ).toBeNull();
  });

  it("removes what the form empties", () => {
    const patch = formProperties(
      {
        ...cardToForm(card),
        birthday: { calendarScale: null, day: "", month: "", year: "" },
        note: " ",
        photo: null,
        title: "",
      },
      card
    );
    expect(patch).toMatchObject({
      notes: null,
      titles: null,
    });
    expect(patch.anniversaries).toMatchObject({ w1: { kind: "wedding" } });
    expect(patch.media).toStrictEqual({
      logo: { kind: "logo", uri: "https://example.com/logo.png" },
    });
  });

  it("creates a card with a uid and only what was filled in", () => {
    const created = newCard(
      { ...emptyForm({ email: "ana@x.com" }), given: "Ana" },
      book,
      "urn:uuid:1"
    );
    expect(created).toStrictEqual({
      "@type": "Card",
      addressBookIds: { b: true },
      emails: { e1: { address: "ana@x.com" } },
      kind: "individual",
      name: {
        components: [{ kind: "given", value: "Ana" }],
        full: "Ana",
        isOrdered: true,
      },
      uid: "urn:uuid:1",
      version: "1.0",
    });
  });

  it("makes groups with members and no phones", () => {
    const created = newCard(
      {
        ...emptyForm({ full: "Equipo", kind: "group" }),
        members: ["urn:uuid:a"],
      },
      book,
      "urn:uuid:g"
    );
    expect(created).toMatchObject({
      kind: "group",
      members: { "urn:uuid:a": true },
      name: { full: "Equipo" },
    });
  });

  it("asks for a name or a way to reach the contact, and valid addresses", () => {
    expect(formProblem(emptyForm(), isValidEmail)).toBe("empty");
    expect(formProblem(emptyForm({ email: "nope" }), isValidEmail)).toBe(
      "invalid-email"
    );
    expect(
      formProblem(emptyForm({ email: "a@x.com" }), isValidEmail)
    ).toBeNull();
    expect(formProblem(emptyForm({ given: "Ana" }), isValidEmail)).toBeNull();
  });
});

describe(importedCard, () => {
  it("puts a parsed card in our book with a uid, without server ids", () => {
    expect(
      importedCard(
        {
          addressBookIds: { x: true },
          id: "zz",
          name: { full: "Dora" },
          vCard: { properties: [] },
        },
        book,
        "urn:uuid:new"
      )
    ).toStrictEqual({
      "@type": "Card",
      addressBookIds: { b: true },
      name: { full: "Dora" },
      uid: "urn:uuid:new",
      vCard: { properties: [] },
      version: "1.0",
    });
    expect(importedCard({ uid: "urn:uuid:own" }, book, "x")).toMatchObject({
      uid: "urn:uuid:own",
    });
  });
});
