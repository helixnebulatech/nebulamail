// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { BlobId, ContactCardId } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import {
  blobPhotoOf,
  cardToVcard,
  cardsToVcf,
  escapeText,
  foldLine,
  splitVcards,
  uriValue,
} from "./vcard";

describe(splitVcards, () => {
  it("splits a file with several cards and ignores text outside", () => {
    const text = [
      "junk",
      "BEGIN:VCARD",
      "VERSION:4.0",
      "FN:Dora",
      "END:VCARD",
      "begin:vcard",
      "VERSION:3.0",
      "FN:Eve",
      "AGENT:BEGIN:VCARD",
      "end:vcard",
      "",
    ].join("\n");
    const cards = splitVcards(text);
    expect(cards).toHaveLength(2);
    expect(cards[0]).toBe(
      "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Dora\r\nEND:VCARD\r\n"
    );
    expect(cards[1]).toContain("FN:Eve");
  });

  it("finds nothing in a file without cards", () => {
    expect(splitVcards("hello")).toStrictEqual([]);
    expect(splitVcards("BEGIN:VCARD\nFN:never closed")).toStrictEqual([]);
  });
});

describe("text values", () => {
  it("escapes RFC 6350 specials", () => {
    expect(escapeText("a,b;c\\d\ne")).toBe("a\\,b\\;c\\\\d\\ne");
  });

  it("folds at 75 octets without splitting characters", () => {
    const folded = foldLine(`NOTE:${"ñ".repeat(60)}`);
    const lines = folded.split("\r\n");
    expect(lines.length).toBeGreaterThan(1);
    for (const line of lines) {
      expect(new TextEncoder().encode(line).length).toBeLessThanOrEqual(75);
    }
    expect(
      lines.map((line, index) => (index === 0 ? line : line.slice(1))).join("")
    ).toBe(`NOTE:${"ñ".repeat(60)}`);
  });
});

/** The TEL line of a card with one phone. */
const tel = (number: string) =>
  cardToVcard({
    id: ContactCardId.parse("t"),
    name: { full: "T" },
    phones: { p: { features: { voice: true }, number } },
  })
    .split("\r\n")
    .find((line) => line.startsWith("TEL"));

describe(cardToVcard, () => {
  it("exports the fields the app edits", () => {
    const text = cardToVcard({
      anniversaries: {
        b: {
          date: { "@type": "PartialDate", day: 2, month: 3 },
          kind: "birth",
        },
      },
      emails: { e: { address: "dora@example.com", contexts: { work: true } } },
      id: ContactCardId.parse("x"),
      name: {
        components: [
          { kind: "given", value: "Dora" },
          { kind: "surname", value: "Díaz; Ruiz" },
        ],
        full: "Dora Díaz",
      },
      notes: { n: { note: "línea 1\nlínea 2" } },
      phones: { p: { features: { mobile: true }, number: "+34 611" } },
      uid: "urn:uuid:1",
    });
    expect(text).toBe(
      [
        "BEGIN:VCARD",
        "VERSION:4.0",
        "UID:urn:uuid:1",
        "FN:Dora Díaz",
        "N:Díaz\\; Ruiz;Dora;;;",
        "EMAIL;TYPE=work:dora@example.com",
        // RFC 6350 §6.4.1: a URI (RFC 3966 tel), spaces as "-".
        "TEL;VALUE=uri;TYPE=cell:tel:+34-611",
        "BDAY:--0302",
        "NOTE:línea 1\\nlínea 2",
        "END:VCARD",
        "",
      ].join("\r\n")
    );
  });

  it("writes TEL as a tel: URI when it can (RFC 6350 §6.4.1)", () => {
    expect(tel("+34 (91) 122.33-44")).toBe(
      "TEL;VALUE=uri;TYPE=voice:tel:+34-(91)-122.33-44"
    );
    // As Stalwart keeps an imported VALUE=uri: unchanged.
    expect(tel("tel:+34-600-111-222")).toBe(
      "TEL;VALUE=uri;TYPE=voice:tel:+34-600-111-222"
    );
    // A local number (no phone-context known) or words: text, escaped.
    expect(tel("600 111 222")).toBe("TEL;TYPE=voice:600 111 222");
    expect(tel("+34 600, ext. 2")).toBe("TEL;TYPE=voice:+34 600\\, ext. 2");
    expect(tel("tel:+34 600")).toBe("TEL;TYPE=voice:tel:+34 600");
  });

  it("exports groups with KIND and MEMBER", () => {
    const text = cardToVcard({
      id: ContactCardId.parse("g"),
      kind: "group",
      members: { "urn:uuid:a": true },
      name: { full: "Equipo" },
    });
    expect(text).toContain("KIND:group\r\nFN:Equipo\r\nMEMBER:urn:uuid:a\r\n");
    expect(text).not.toContain("\r\nN:");
  });

  it("writes a text UID as VALUE=text and only URI members (RFC 6350 §6.7.6, §6.6.5)", () => {
    const text = cardToVcard({
      id: ContactCardId.parse("g"),
      kind: "group",
      members: {
        "abc,1": true,
        "urn:uuid:a": true,
        "urn:uuid:b\r\nX-EVIL:1": true,
      },
      name: { full: "Equipo" },
      uid: "apple-1;x\r\nX-EVIL:1",
    });
    expect(text).toContain("UID;VALUE=text:apple-1\\;x\\nX-EVIL:1\r\n");
    expect(text).toContain("\r\nMEMBER:urn:uuid:a\r\n");
    expect(text).not.toContain("abc");
    expect(text).not.toContain("\r\nX-EVIL");
    expect(text.match(/MEMBER/gu)).toHaveLength(1);
  });

  it("writes a URI UID as it is and tells URIs apart", () => {
    expect(
      cardToVcard({ id: ContactCardId.parse("u"), uid: "urn:uuid:1" })
    ).toContain("\r\nUID:urn:uuid:1\r\n");
    expect(uriValue("mailto:a@b.c")).toBe("mailto:a@b.c");
    expect(uriValue("no scheme")).toBeNull();
    expect(uriValue("urn:a b")).toBeNull();
  });
});

const unfolded = (text: string): string => text.replaceAll("\r\n ", "");

describe("blob photos", () => {
  const blobId = BlobId.parse("Gphoto");
  const card = {
    id: ContactCardId.parse("c1"),
    media: { m: { blobId, kind: "photo", mediaType: "image/jpeg" } },
    name: { full: "Dora" },
  };

  it("says which photo has to be downloaded", () => {
    expect(blobPhotoOf(card)).toStrictEqual({
      blobId,
      kind: "blob",
      mediaType: "image/jpeg",
    });
    expect(
      blobPhotoOf({
        id: card.id,
        media: { m: { kind: "photo", uri: "https://evil.example/a.png" } },
      })
    ).toBeNull();
  });

  it("writes the downloaded photo as a data: URL", () => {
    const uri = `data:image/jpeg;base64,${"A".repeat(200)}`;
    const text = cardsToVcf([card], new Map([[blobId, uri]]));
    expect(unfolded(text)).toContain(`PHOTO:${uri}\r\n`);
    expect(text.split("\r\n").every((line) => line.length <= 75)).toBeTruthy();
  });

  it("leaves it out when it wasn't downloaded", () => {
    expect(cardToVcard(card)).not.toContain("PHOTO");
  });
});
