// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// vCard text (RFC 6350), only what the contacts app needs:
// - `splitVcards`: a .vcf file with several cards → one text per card, so
//   each can go through `ContactCard/parse` (Stalwart parses only the first
//   card of a blob).
// - `toVcard`: export cards as vCard 4.0 (escaped, folded at 75 octets).
// Parsing vCards into cards is the server's job (`contacts:parse`).

import type { CardFields, PartialDate, Photo } from "./card";
import { birthdayOf, cardToForm, displayName, isGroup, photoOf } from "./card";

/** Cards taken from one file at most (the rest is ignored, with a notice). */
export const MAX_IMPORT_CARDS = 200;

/**
 * Each BEGIN:VCARD … END:VCARD block (nesting, as in vCard 2.1 AGENT, is
 * kept inside its parent). Text outside cards is ignored.
 */
export const splitVcards = (text: string): string[] => {
  const cards: string[] = [];
  let depth = 0;
  let current: string[] = [];
  for (const line of text.split(/\r\n|\r|\n/u)) {
    const upper = line.trim().toUpperCase();
    if (upper === "BEGIN:VCARD") {
      depth += 1;
    }
    if (depth > 0) {
      current.push(line);
    }
    if (upper === "END:VCARD" && depth > 0) {
      depth -= 1;
      if (depth === 0) {
        cards.push(`${current.join("\r\n")}\r\n`);
        current = [];
      }
    }
  }
  return cards;
};

/** RFC 6350 §3.4 text value escaping. */
export const escapeText = (value: string): string =>
  value
    .replaceAll("\\", "\\\\")
    .replaceAll(/\r\n|\r|\n/gu, "\\n")
    .replaceAll(",", "\\,")
    .replaceAll(";", "\\;");

const encoder = new TextEncoder();

/** RFC 6350 §3.2: lines over 75 octets continue with CRLF + space. */
export const foldLine = (line: string): string => {
  const parts: string[] = [];
  let current = "";
  let size = 0;
  for (const char of line) {
    const bytes = encoder.encode(char).length;
    // The first line has 75 octets; the rest 74 after the leading space.
    const limit = parts.length === 0 ? 75 : 74;
    if (size + bytes > limit) {
      parts.push(current);
      current = "";
      size = 0;
    }
    current += char;
    size += bytes;
  }
  parts.push(current);
  return parts.join("\r\n ");
};

const property = (name: string, value: string, params = ""): string =>
  foldLine(`${name}${params}:${value}`);

/**
 * `value` if it can be written as a URI value (RFC 6350 §4.2, RFC 3986
 * §3.1): a scheme, then no space or control character (they'd have to be
 * percent-encoded, and CR/LF would end the line). `null` otherwise.
 */
export const uriValue = (value: string): string | null =>
  /^[a-z][a-z0-9+.-]*:[^\s\p{Cc}]*$/iu.test(value) ? value : null;

/**
 * RFC 6350 §6.7.6: UID is a URI by default (`urn:uuid:…`, what JMAP
 * servers give); any other text goes as `VALUE=text`, escaped.
 */
const uidLine = (uid: string): string => {
  const uri = uriValue(uid);
  return uri === null
    ? property("UID", escapeText(uid), ";VALUE=text")
    : property("UID", uri);
};

/** URI schemes a TEL value is read as (RFC 3966 tel, RFC 3261 sip/sips). */
const PHONE_SCHEMES = /^(?:tel|sips?):/iu;
/** A global number (RFC 3966 §5.1.4): "+", digits and visual separators. */
const GLOBAL_NUMBER = /^\+[\d\s().-]*\d[\d\s().-]*$/u;

/**
 * TEL as a URI (RFC 6350 §6.4.1: "it SHOULD be reset to a URI value",
 * the "tel" scheme of RFC 3966). A number that already is one (Stalwart
 * keeps an imported `VALUE=uri` as "tel:…") goes as it is; a global number
 * as `tel:+…`, its spaces as "-" (RFC 3966 §5.1.1 visual separators).
 * Anything else stays text, the default vCard 4 still allows: a local
 * number needs a phone-context we don't know (RFC 3966 §5.1.5), and an
 * extension in words isn't a URI.
 */
const telLine = (number: string, type: string): string => {
  const value = number.trim();
  const uri = PHONE_SCHEMES.test(value) ? uriValue(value) : null;
  if (uri !== null) {
    return property("TEL", uri, `;VALUE=uri;TYPE=${type}`);
  }
  if (GLOBAL_NUMBER.test(value)) {
    return property(
      "TEL",
      `tel:${value.replaceAll(/\s+/gu, "-")}`,
      `;VALUE=uri;TYPE=${type}`
    );
  }
  return property("TEL", escapeText(value), `;TYPE=${type}`);
};

const pad = (value: number, length = 2): string =>
  String(value).padStart(length, "0");

/** `BDAY:--1224` without a year; `;CALSCALE=` for another calendar. */
const birthdayLine = (birthday: PartialDate): string =>
  property(
    "BDAY",
    `${birthday.year === null ? "--" : pad(birthday.year, 4)}${pad(birthday.month)}${pad(birthday.day)}`,
    // RFC 6350 §5.8; the scale is a CLDR name ([a-z0-9-]) we checked.
    birthday.calendarScale === null ||
      !/^[a-z0-9-]{1,40}$/u.test(birthday.calendarScale)
      ? ""
      : `;CALSCALE=${birthday.calendarScale}`
  );

/** Blob photos (Stalwart 1.0) as `data:` URLs, by `blobId`. */
export type ExportPhotos = ReadonlyMap<string, string>;

/** The card's photo if it has to be downloaded to go in a vCard. */
export const blobPhotoOf = (
  card: CardFields
): Extract<Photo, { readonly kind: "blob" }> | null => {
  const photo = photoOf(card);
  return photo?.kind === "blob" ? photo : null;
};

/** The PHOTO value. A remote photo stays a URL: it's never downloaded. */
const photoValue = (
  photo: Photo | null,
  photos: ExportPhotos
): string | null => {
  if (photo?.kind === "blob") {
    return photos.get(photo.blobId) ?? null;
  }
  if (photo?.kind === "data") {
    return photo.uri;
  }
  return photo?.kind === "remote" ? photo.url : null;
};

/**
 * PHOTO and MEMBER lines. Both are URI values only (RFC 6350 §6.2.4,
 * §6.6.5): one that isn't a URI can't be written, so it is left out.
 */
const uriLines = (
  photo: Photo | null,
  members: readonly string[],
  photos: ExportPhotos
): string[] => {
  const value = photoValue(photo, photos);
  const photoUri = value === null ? null : uriValue(value);
  const lines = photoUri === null ? [] : [property("PHOTO", photoUri)];
  for (const uid of members) {
    const member = uriValue(uid);
    if (member !== null) {
      lines.push(property("MEMBER", member));
    }
  }
  return lines;
};

/** One card as vCard 4.0. Unknown JSContact properties aren't exported. */
export const cardToVcard = (
  card: CardFields,
  photos: ExportPhotos = new Map()
): string => {
  const form = cardToForm(card);
  const lines = ["BEGIN:VCARD", "VERSION:4.0"];
  if (typeof card.uid === "string" && card.uid !== "") {
    lines.push(uidLine(card.uid));
  }
  if (isGroup(card)) {
    lines.push("KIND:group");
  }
  lines.push(property("FN", escapeText(displayName(card))));
  if (!isGroup(card)) {
    lines.push(
      property(
        "N",
        [form.surname, form.given, "", "", ""].map(escapeText).join(";")
      )
    );
  }
  for (const email of form.emails) {
    const type =
      email.place === "other"
        ? ""
        : `;TYPE=${email.place === "work" ? "work" : "home"}`;
    lines.push(property("EMAIL", escapeText(email.address), type));
  }
  for (const phone of form.phones) {
    const type = { home: "home", mobile: "cell", voice: "voice", work: "work" }[
      phone.kind
    ];
    lines.push(telLine(phone.number, type));
  }
  for (const address of form.addresses) {
    const type =
      address.place === "other"
        ? ""
        : `;TYPE=${address.place === "work" ? "work" : "home"}`;
    lines.push(
      property(
        "ADR",
        [
          "",
          "",
          address.street,
          address.locality,
          address.region,
          address.postcode,
          address.country,
        ]
          .map(escapeText)
          .join(";"),
        type
      )
    );
  }
  if (form.organization !== "") {
    lines.push(property("ORG", escapeText(form.organization)));
  }
  if (form.title !== "") {
    lines.push(property("TITLE", escapeText(form.title)));
  }
  const birthday = birthdayOf(card);
  if (birthday !== null) {
    lines.push(birthdayLine(birthday));
  }
  if (form.note !== "") {
    lines.push(property("NOTE", escapeText(form.note)));
  }
  lines.push(...uriLines(form.photo, form.members, photos), "END:VCARD");
  return `${lines.join("\r\n")}\r\n`;
};

export const cardsToVcf = (
  cards: readonly CardFields[],
  photos: ExportPhotos = new Map()
): string => cards.map((card) => cardToVcard(card, photos)).join("");
