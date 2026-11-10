// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// JSContact cards (RFC 9553) ↔ what the contacts screens show and edit.
// Pure functions, no JMAP calls (those are in api.ts).
//
// Everything in a card is untrusted text from whoever wrote it (a vCard in
// an email, another client): the UI shows it only as text, cleaned of
// bidi and invisible characters (`cleanDisplayText`, `cleanMultiline`),
// inside `<bdi>`. Photos: `data:` images of a few raster types are shown
// as they are; remote URLs only through the image proxy.
//
// Editing keeps what we don't edit: each email/phone/address keeps its key
// and the properties we don't show (labels, `pref`…), and properties we
// don't know (links, nicknames, vCard extras) aren't touched.

import type {
  AddressBookId,
  BlobId,
  ContactAnniversary,
  ContactCardCreate,
  ContactCardId,
  ContactCardPatch,
  ParsedCard,
} from "@nebulamail/jmap-client";
import { cleanDisplayText } from "@nebulamail/mail-render";

// --- Display ------------------------------------------------------------------

/** Card fields the list and the detail read (any subset of a card). */
export type CardFields = ParsedCard & { readonly id: ContactCardId };

/** One-line text from a card, safe to show. */
export const clean = (text: string | null | undefined): string =>
  cleanDisplayText(text ?? "");

/** Notes: like `clean`, but keeps line breaks. */
export const cleanMultiline = (text: string): string =>
  text
    .split(/\r\n|\r|\n/u)
    .map((line) => cleanDisplayText(line))
    .join("\n")
    .replaceAll(/\n{3,}/gu, "\n\n")
    .trim();

const nameFromComponents = (card: CardFields): string =>
  (card.name?.components ?? [])
    .filter((part) => part.kind !== "separator")
    .map((part) => clean(part.value))
    .filter((part) => part !== "")
    .join(" ");

const byPref = <T extends { readonly pref?: number | null | undefined }>(
  items: Readonly<Record<string, T>> | null | undefined
): [string, T][] =>
  Object.entries(items ?? {}).toSorted(
    ([, a], [, b]) => (a.pref ?? 100) - (b.pref ?? 100)
  );

export const emailsOf = (card: CardFields): string[] =>
  byPref(card.emails)
    .map(([, email]) => clean(email.address))
    .filter((address) => address !== "");

export const organizationOf = (card: CardFields): string =>
  Object.values(card.organizations ?? {})
    .map((org) => clean(org.name))
    .find((name) => name !== "") ?? "";

/** Full name, else its parts, else organization, else first address. */
export const displayName = (card: CardFields): string => {
  const full = clean(card.name?.full);
  if (full !== "") {
    return full;
  }
  return (
    [nameFromComponents(card), organizationOf(card), emailsOf(card)[0] ?? ""]
      .map((text) => text.trim())
      .find((text) => text !== "") ?? ""
  );
};

export const isGroup = (card: CardFields): boolean => card.kind === "group";

// --- Photos -------------------------------------------------------------------

/** Raster types only: an SVG can carry scripts and remote references. */
const DATA_IMAGE =
  /^data:image\/(?:png|jpeg|gif|webp);base64,[a-z0-9+/]+={0,2}$/iu;

/** The same raster types, for a photo stored as a blob. */
const RASTER_TYPES = new Set([
  "image/png",
  "image/jpeg",
  "image/gif",
  "image/webp",
]);

/** Biggest inline photo shown or saved (about 190 KB of image). */
export const MAX_PHOTO_DATA_URL = 256 * 1024;

export type Photo =
  | { readonly kind: "data"; readonly uri: string }
  /** Never loaded directly: through the proxy, on request. */
  | { readonly kind: "remote"; readonly url: string }
  /**
   * Stalwart 1.0 turns a vCard's inline PHOTO into a blob of the card
   * (`media.X.blobId` + `mediaType`). Downloaded with the session's
   * credentials and redrawn (photo.ts) before it is shown.
   */
  | {
      readonly kind: "blob";
      readonly blobId: BlobId;
      readonly mediaType: string;
    };

export const photoOf = (card: CardFields): Photo | null => {
  for (const [, media] of byPref(card.media)) {
    if (media.kind.toLowerCase() !== "photo") {
      continue;
    }
    const uri = media.uri?.trim() ?? "";
    if (uri === "") {
      const mediaType = media.mediaType?.trim().toLowerCase() ?? "";
      if (
        media.blobId !== null &&
        media.blobId !== undefined &&
        RASTER_TYPES.has(mediaType)
      ) {
        return { blobId: media.blobId, kind: "blob", mediaType };
      }
      continue;
    }
    if (uri.length <= MAX_PHOTO_DATA_URL && DATA_IMAGE.test(uri)) {
      return { kind: "data", uri };
    }
    if (/^https?:\/\//iu.test(uri)) {
      return { kind: "remote", url: uri };
    }
  }
  return null;
};

const samePhoto = (a: Photo | null, b: Photo | null): boolean => {
  if (a === null || b === null) {
    return a === b;
  }
  switch (a.kind) {
    case "data": {
      return b.kind === "data" && a.uri === b.uri;
    }
    case "remote": {
      return b.kind === "remote" && a.url === b.url;
    }
    case "blob": {
      return b.kind === "blob" && a.blobId === b.blobId;
    }
    default: {
      return a satisfies never;
    }
  }
};

// --- Dates --------------------------------------------------------------------

export interface PartialDate {
  readonly year: number | null;
  readonly month: number;
  readonly day: number;
  /**
   * RFC 9553 §2.8.1 `calendarScale` (CLDR name, lower case: "hebrew",
   * "chinese"…), or null for the Gregorian calendar. Day, month and year
   * are in that calendar.
   */
  readonly calendarScale: string | null;
}

const scaleOf = (value: unknown): string | null => {
  if (typeof value !== "string") {
    return null;
  }
  const scale = value.trim().toLowerCase();
  return scale === "" || scale === "gregorian" || scale === "gregory"
    ? null
    : scale;
};

export const birthdayOf = (card: CardFields): PartialDate | null => {
  const entry = Object.values(card.anniversaries ?? {}).find(
    (anniversary) => anniversary.kind === "birth"
  );
  const date = entry?.date;
  if (date === undefined) {
    return null;
  }
  if ("utc" in date && typeof date.utc === "string") {
    const parsed = new Date(date.utc);
    return Number.isNaN(parsed.getTime())
      ? null
      : {
          calendarScale: null,
          day: parsed.getUTCDate(),
          month: parsed.getUTCMonth() + 1,
          year: parsed.getUTCFullYear(),
        };
  }
  const { day, month, year } = date;
  const calendarScale = scaleOf(date.calendarScale);
  // Some calendars have a 13th month (Hebrew, Ethiopic, Coptic).
  const months = calendarScale === null ? 12 : 13;
  return typeof day === "number" &&
    typeof month === "number" &&
    month >= 1 &&
    month <= months &&
    day >= 1 &&
    day <= 31
    ? {
        calendarScale,
        day,
        month,
        year: typeof year === "number" ? year : null,
      }
    : null;
};

/**
 * The birthday as the editor holds it: day, month and an optional year,
 * as typed. A date in another calendar is kept as it is (the editor only
 * shows it, and offers to change it).
 */
export interface BirthdayInput {
  readonly day: string;
  readonly month: string;
  /** "" for a birthday without a year (vCard `--MMDD`). */
  readonly year: string;
  readonly calendarScale: string | null;
}

export const NO_BIRTHDAY: BirthdayInput = {
  calendarScale: null,
  day: "",
  month: "",
  year: "",
};

export const birthdayInput = (date: PartialDate | null): BirthdayInput =>
  date === null
    ? NO_BIRTHDAY
    : {
        calendarScale: date.calendarScale,
        day: String(date.day),
        month: String(date.month),
        year: date.year === null ? "" : String(date.year),
      };

/** Days in a Gregorian month; without a year February has 29. */
export const daysInMonth = (month: number, year: number | null): number => {
  const date = new Date(Date.UTC(2000, month, 0));
  date.setUTCFullYear(year ?? 2000, month, 0);
  return date.getUTCDate();
};

const whole = (text: string): number | null =>
  /^\d{1,4}$/u.test(text.trim()) ? Number(text.trim()) : null;

/** null: no birthday; "invalid": half filled in or impossible. */
export const parseBirthday = (
  input: BirthdayInput
): PartialDate | null | "invalid" => {
  const typedYear = input.year.trim() !== "";
  if (input.day.trim() === "" && input.month.trim() === "") {
    return typedYear ? "invalid" : null;
  }
  const day = whole(input.day);
  const month = whole(input.month);
  const year = typedYear ? whole(input.year) : null;
  if (day === null || month === null || (typedYear && (year ?? 0) < 1)) {
    return "invalid";
  }
  if (input.calendarScale !== null) {
    return month >= 1 && month <= 13 && day >= 1 && day <= 31
      ? { calendarScale: input.calendarScale, day, month, year }
      : "invalid";
  }
  return month >= 1 &&
    month <= 12 &&
    day >= 1 &&
    day <= daysInMonth(month, year)
    ? { calendarScale: null, day, month, year }
    : "invalid";
};

const sameDate = (a: PartialDate | null, b: PartialDate | null): boolean =>
  a === null || b === null
    ? a === b
    : a.day === b.day &&
      a.month === b.month &&
      a.year === b.year &&
      a.calendarScale === b.calendarScale;

// --- Editing ------------------------------------------------------------------

export const PHONE_KINDS = ["mobile", "voice", "work", "home"] as const;
export type PhoneKind = (typeof PHONE_KINDS)[number];

export const PLACE_KINDS = ["home", "work", "other"] as const;
export type PlaceKind = (typeof PLACE_KINDS)[number];

interface Row {
  /** The entry's key in the card (kept on edit), or null for a new one. */
  readonly key: string | null;
}

export interface EmailRow extends Row {
  readonly address: string;
  readonly place: PlaceKind;
}

export interface PhoneRow extends Row {
  readonly number: string;
  readonly kind: PhoneKind;
}

export interface AddressRow extends Row {
  readonly street: string;
  readonly locality: string;
  readonly region: string;
  readonly postcode: string;
  readonly country: string;
  readonly place: PlaceKind;
}

export interface ContactForm {
  readonly kind: "individual" | "group";
  readonly given: string;
  readonly surname: string;
  /** Group name, or a full name that isn't just given + surname. */
  readonly full: string;
  readonly emails: readonly EmailRow[];
  readonly phones: readonly PhoneRow[];
  readonly addresses: readonly AddressRow[];
  readonly organization: string;
  readonly title: string;
  readonly note: string;
  readonly birthday: BirthdayInput;
  /** `data:` URL; a remote or blob photo stays as it is unless removed. */
  readonly photo: Photo | null;
  /** Groups: member uids. */
  readonly members: readonly string[];
}

export const emptyForm = (
  init: Partial<Pick<ContactForm, "full" | "given" | "kind">> & {
    readonly email?: string;
  } = {}
): ContactForm => ({
  addresses: [],
  birthday: NO_BIRTHDAY,
  emails:
    init.email === undefined || init.email === ""
      ? []
      : [{ address: init.email, key: null, place: "other" }],
  full: init.full ?? "",
  given: init.given ?? "",
  kind: init.kind ?? "individual",
  members: [],
  note: "",
  organization: "",
  phones: [],
  photo: null,
  surname: "",
  title: "",
});

const placeOf = (
  contexts: Readonly<Record<string, boolean>> | null | undefined
): PlaceKind => {
  if (contexts?.work === true) {
    return "work";
  }
  return contexts?.private === true ? "home" : "other";
};

const contextsFor = (
  place: PlaceKind
): Readonly<Record<string, boolean>> | undefined => {
  if (place === "work") {
    return { work: true };
  }
  return place === "home" ? { private: true } : undefined;
};

const phoneKindOf = (
  features: Readonly<Record<string, boolean>> | null | undefined,
  contexts: Readonly<Record<string, boolean>> | null | undefined
): PhoneKind => {
  if (features?.mobile === true) {
    return "mobile";
  }
  if (contexts?.work === true) {
    return "work";
  }
  return contexts?.private === true ? "home" : "voice";
};

const component = (
  components:
    | readonly { readonly kind?: string | undefined; readonly value: string }[]
    | null
    | undefined,
  kind: string
): string =>
  (components ?? [])
    .filter((part) => part.kind === kind)
    .map((part) => part.value)
    .join(" ");

export const cardToForm = (card: CardFields): ContactForm => {
  const components = card.name?.components ?? [];
  const given = component(components, "given");
  const surname = component(components, "surname");
  const full = card.name?.full ?? "";
  const joined = [given, surname].filter((part) => part !== "").join(" ");
  return {
    addresses: Object.entries(card.addresses ?? {}).map(([key, address]) => {
      const parts = address.components ?? [];
      const street = [
        component(parts, "name"),
        component(parts, "number"),
      ].filter((part) => part !== "");
      return {
        country: component(parts, "country"),
        key,
        locality: component(parts, "locality"),
        place: placeOf(address.contexts),
        postcode: component(parts, "postcode"),
        region: component(parts, "region"),
        street:
          street.length === 0 && parts.length === 0
            ? (address.full ?? "")
            : street.join(" "),
      };
    }),
    birthday: birthdayInput(birthdayOf(card)),
    emails: byPref(card.emails).map(([key, email]) => ({
      address: email.address,
      key,
      place: placeOf(email.contexts),
    })),
    // Shown only when it says more than given + surname.
    full: full === joined ? "" : full,
    given,
    kind: isGroup(card) ? "group" : "individual",
    members: Object.entries(card.members ?? {})
      .filter(([, member]) => member)
      .map(([uid]) => uid),
    note: Object.values(card.notes ?? {})
      .map((note) => note.note)
      .join("\n\n"),
    organization: Object.values(card.organizations ?? {})[0]?.name ?? "",
    phones: byPref(card.phones).map(([key, phone]) => ({
      key,
      kind: phoneKindOf(phone.features, phone.contexts),
      number: phone.number,
    })),
    photo: photoOf(card),
    surname,
    title: Object.values(card.titles ?? {})[0]?.name ?? "",
  };
};

const trimmed = (text: string): string => text.trim();

/** Keys for new entries that don't clash with the card's own. */
const freshKey = (prefix: string, taken: ReadonlySet<string>): string => {
  let index = 1;
  while (taken.has(`${prefix}${index}`)) {
    index += 1;
  }
  return `${prefix}${index}`;
};

const rowsToMap = <R extends Row, V>(
  rows: readonly R[],
  original: Readonly<Record<string, V>> | null | undefined,
  prefix: string,
  build: (row: R, previous: V | undefined) => V | null
): Record<string, V> | null => {
  const taken = new Set(
    rows.flatMap((row) => (row.key === null ? [] : [row.key]))
  );
  const result: Record<string, V> = {};
  for (const row of rows) {
    const previous = row.key === null ? undefined : original?.[row.key];
    const value = build(row, previous);
    if (value !== null) {
      const key = row.key ?? freshKey(prefix, taken);
      taken.add(key);
      result[key] = value;
    }
  }
  return Object.keys(result).length === 0 ? null : result;
};

type Card = ContactCardCreate;
type Emails = NonNullable<Card["emails"]>[string];
type Phones = NonNullable<Card["phones"]>[string];
type Addresses = NonNullable<Card["addresses"]>[string];

type Media = NonNullable<Card["media"]>[string];

const nameProperty = (form: ContactForm): Card["name"] => {
  const given = trimmed(form.given);
  const surname = trimmed(form.surname);
  const full = trimmed(form.full);
  const components =
    form.kind === "group"
      ? []
      : [
          ...(given === "" ? [] : [{ kind: "given", value: given }]),
          ...(surname === "" ? [] : [{ kind: "surname", value: surname }]),
        ];
  const fullName =
    full === ""
      ? [given, surname].filter((part) => part !== "").join(" ")
      : full;
  if (fullName === "" && components.length === 0) {
    return null;
  }
  return {
    ...(components.length === 0 ? {} : { components, isOrdered: true }),
    full: fullName,
  };
};

const emailsProperty = (
  form: ContactForm,
  original: CardFields | null
): Card["emails"] =>
  rowsToMap<EmailRow, Emails>(
    form.emails,
    original?.emails,
    "e",
    (row, previous) => {
      const address = trimmed(row.address);
      if (address === "") {
        return null;
      }
      const { contexts: _contexts, ...rest } = previous ?? {};
      const contexts = contextsFor(row.place);
      return {
        ...rest,
        address,
        ...(contexts === undefined ? {} : { contexts }),
      };
    }
  );

const PHONE_CONTEXTS: Readonly<
  Record<PhoneKind, Readonly<Record<string, boolean>> | undefined>
> = {
  home: { private: true },
  mobile: undefined,
  voice: undefined,
  work: { work: true },
};

const phonesProperty = (
  form: ContactForm,
  original: CardFields | null
): Card["phones"] =>
  rowsToMap<PhoneRow, Phones>(
    form.phones,
    original?.phones,
    "p",
    (row, previous) => {
      const number = trimmed(row.number);
      if (number === "") {
        return null;
      }
      const {
        contexts: _contexts,
        features: _features,
        ...rest
      } = previous ?? {};
      const contexts = PHONE_CONTEXTS[row.kind];
      return {
        ...rest,
        features: row.kind === "mobile" ? { mobile: true } : { voice: true },
        number,
        ...(contexts === undefined ? {} : { contexts }),
      };
    }
  );

const addressesProperty = (
  form: ContactForm,
  original: CardFields | null
): Card["addresses"] =>
  rowsToMap<AddressRow, Addresses>(
    form.addresses,
    original?.addresses,
    "a",
    (row, previous) => {
      const parts = (
        [
          ["name", row.street],
          ["locality", row.locality],
          ["region", row.region],
          ["postcode", row.postcode],
          ["country", row.country],
        ] as const
      )
        .map(([kind, value]) => ({ kind, value: trimmed(value) }))
        .filter((part) => part.value !== "");
      if (parts.length === 0) {
        return null;
      }
      const {
        components: _components,
        contexts: _contexts,
        full: _full,
        ...rest
      } = previous ?? {};
      const contexts = contextsFor(row.place);
      return {
        ...rest,
        components: parts,
        ...(contexts === undefined ? {} : { contexts }),
      };
    }
  );

/**
 * Other anniversaries are kept; the birthday is replaced (or removed). An
 * unchanged birthday keeps its entry as it is (key, calendar, extras).
 */
const anniversariesProperty = (
  form: ContactForm,
  original: CardFields | null
): Card["anniversaries"] => {
  const parsed = parseBirthday(form.birthday);
  const birthday = parsed === "invalid" ? null : parsed;
  const before = original === null ? null : birthdayOf(original);
  if (original !== null && sameDate(birthday, before)) {
    const kept = original.anniversaries ?? {};
    return Object.keys(kept).length === 0 ? null : kept;
  }
  const others = Object.entries(original?.anniversaries ?? {}).filter(
    ([, anniversary]) => anniversary.kind !== "birth"
  );
  const all: [string, ContactAnniversary][] =
    birthday === null
      ? others
      : [
          ...others,
          [
            freshKey("b", new Set(others.map(([key]) => key))),
            {
              date: {
                "@type": "PartialDate",
                day: birthday.day,
                month: birthday.month,
                ...(birthday.year === null ? {} : { year: birthday.year }),
                ...(birthday.calendarScale === null
                  ? {}
                  : { calendarScale: birthday.calendarScale }),
              },
              kind: "birth",
            },
          ],
        ];
  return all.length === 0 ? null : Object.fromEntries(all);
};

/**
 * Logos and sounds are kept; the photo is replaced (or removed). An
 * unchanged photo leaves `media` out of the patch (undefined), so the
 * server keeps its entries exactly as they were: a blob photo of Stalwart
 * 1.0 can't be rebuilt here.
 */
const mediaProperty = (
  form: ContactForm,
  original: CardFields | null
): Card["media"] | undefined => {
  const { photo } = form;
  if (original !== null && samePhoto(photo, photoOf(original))) {
    return undefined;
  }
  const others = Object.entries(original?.media ?? {}).filter(
    ([, media]) => media.kind.toLowerCase() !== "photo"
  );
  let entry: Media | null = null;
  if (photo?.kind === "data") {
    const type = /^data:(?<type>[^;,]+)/u.exec(photo.uri)?.groups?.type;
    entry = {
      kind: "photo",
      uri: photo.uri,
      ...(type === undefined ? {} : { mediaType: type }),
    };
  } else if (photo?.kind === "remote") {
    entry = { kind: "photo", uri: photo.url };
  } else if (photo?.kind === "blob") {
    entry = { blobId: photo.blobId, kind: "photo", mediaType: photo.mediaType };
  }
  const all: [string, Media][] =
    entry === null
      ? others
      : [
          ...others,
          [freshKey("m", new Set(others.map(([key]) => key))), entry],
        ];
  return all.length === 0 ? null : Object.fromEntries(all);
};

const mediaPatch = (
  form: ContactForm,
  original: CardFields | null
): { readonly media?: Card["media"] } => {
  const media = mediaProperty(form, original);
  return media === undefined ? {} : { media };
};

/** A one-entry map (`{ n1: { note } }`), or null for empty text. */
const single = <V>(
  key: string,
  value: string,
  build: (text: string) => V
): Record<string, V> | null =>
  value.trim() === "" ? null : { [key]: build(value.trim()) };

/** The editable properties of a card, from a form (null: remove it). */
export const formProperties = (
  form: ContactForm,
  original: CardFields | null
): ContactCardPatch => {
  const group = form.kind === "group";
  return {
    addresses: group ? null : addressesProperty(form, original),
    anniversaries: anniversariesProperty(form, original),
    emails: emailsProperty(form, original),
    kind: form.kind,
    ...mediaPatch(form, original),
    members:
      group && form.members.length > 0
        ? Object.fromEntries(form.members.map((uid) => [uid, true]))
        : null,
    name: nameProperty(form),
    notes: single("n1", form.note, (note) => ({ note })),
    organizations: single("o1", form.organization, (name) => ({ name })),
    phones: group ? null : phonesProperty(form, original),
    titles: single("t1", form.title, (name) => ({ kind: "title", name })),
  };
};

/** Drops removals (`null`): what a new card is created with. */
const withoutNulls = (patch: ContactCardPatch): Record<string, unknown> =>
  Object.fromEntries(
    Object.entries(patch).filter(([, value]) => value !== null)
  );

/** A new card for `ContactCard/set` create. We always set `uid` (Stalwart doesn't). */
export const newCard = (
  form: ContactForm,
  addressBookId: AddressBookId,
  uid: string
): ContactCardCreate => ({
  ...withoutNulls(formProperties(form, null)),
  "@type": "Card",
  addressBookIds: { [addressBookId]: true },
  kind: form.kind,
  uid,
  version: "1.0",
});

export const newUid = (): string => `urn:uuid:${crypto.randomUUID()}`;

/** What a form must have before saving; null when it's fine. */
export const formProblem = (
  form: ContactForm,
  isValidEmail: (email: string) => boolean
): "empty" | "invalid-email" | "invalid-birthday" | null => {
  if (
    form.emails.some(
      (row) => row.address.trim() !== "" && !isValidEmail(row.address)
    )
  ) {
    return "invalid-email";
  }
  if (
    form.kind === "individual" &&
    parseBirthday(form.birthday) === "invalid"
  ) {
    return "invalid-birthday";
  }
  const named = [form.given, form.surname, form.full, form.organization].some(
    (text) => text.trim() !== ""
  );
  const reachable =
    form.emails.some((row) => row.address.trim() !== "") ||
    form.phones.some((row) => row.number.trim() !== "");
  return named || reachable ? null : "empty";
};

/** An imported card (from `ContactCard/parse`), ready to create. */
export const importedCard = (
  parsed: Readonly<Record<string, unknown>>,
  addressBookId: AddressBookId,
  uid: string
): ContactCardCreate => {
  // Server-set and per-book properties are ours to choose; Stalwart's
  // `vCard` extension (RFC 9555 leftovers) is kept: it round-trips.
  const {
    addressBookIds: _books,
    created: _created,
    id: _id,
    updated: _updated,
    ...rest
  } = parsed;
  const existingUid = typeof rest.uid === "string" ? rest.uid : "";
  return {
    ...rest,
    "@type": "Card",
    addressBookIds: { [addressBookId]: true },
    uid: existingUid === "" ? uid : existingUid,
    version: "1.0",
  };
};
