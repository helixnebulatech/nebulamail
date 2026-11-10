// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// JMAP for Contacts (RFC 9610): AddressBook and ContactCard, where a
// ContactCard is a JSContact Card (RFC 9553). The properties a contacts app
// edits are typed; everything else passes through untouched (loose
// objects), so a card edited here keeps what other clients wrote.
//
// Checked live against Stalwart (2026-11-10):
// - Properties that a card doesn't have are left out of /get, even when
//   asked for by name (RFC 8620 says null). `ContactCard/get` fills them in
//   with null (methods.ts).
// - It doesn't set `uid` when a card is created without one; send it.
// - An inline vCard PHOTO comes back as `media.X.blobId` + `mediaType`,
//   served by the normal download; `/set` accepts `blobId` too.
// - `ContactCard/parse` returns only the first card of a blob with several
//   vCards, and leaves unknown blob ids out instead of in `notFound`.
// - The `text`, `name` and `email` filters match whole words only.

import { z } from "zod";

import {
  AccountId,
  AddressBookId,
  BlobId,
  ContactCardId,
  PrincipalId,
} from "./ids";
import type { GetArgs, PatchObject, QueryArgs, SetArgs } from "./standard";

// ---------------------------------------------------------------- JSContact

/** RFC 9553 §1.5.1 contexts ("work", "private") and similar sets. */
const BooleanSet = z.record(z.string(), z.boolean());

const NameComponentSchema = z.looseObject({
  kind: z.string().optional(),
  value: z.string(),
});

const NameSchema = z.looseObject({
  components: z.array(NameComponentSchema).nullish(),
  full: z.string().nullish(),
  isOrdered: z.boolean().nullish(),
});
export type ContactName = z.infer<typeof NameSchema>;

const EmailAddressSchema = z.looseObject({
  address: z.string(),
  contexts: BooleanSet.nullish(),
  label: z.string().nullish(),
  pref: z.number().nullish(),
});

const PhoneSchema = z.looseObject({
  contexts: BooleanSet.nullish(),
  features: BooleanSet.nullish(),
  label: z.string().nullish(),
  number: z.string(),
  pref: z.number().nullish(),
});

const AddressComponentSchema = z.looseObject({
  kind: z.string(),
  value: z.string(),
});

const AddressSchema = z.looseObject({
  components: z.array(AddressComponentSchema).nullish(),
  contexts: BooleanSet.nullish(),
  countryCode: z.string().nullish(),
  full: z.string().nullish(),
  pref: z.number().nullish(),
});
export type ContactAddress = z.infer<typeof AddressSchema>;

const OrganizationSchema = z.looseObject({
  name: z.string().nullish(),
  units: z.array(z.looseObject({ name: z.string() })).nullish(),
});

const TitleSchema = z.looseObject({
  kind: z.string().nullish(),
  name: z.string(),
});

const NoteSchema = z.looseObject({
  created: z.string().nullish(),
  note: z.string(),
});

/** RFC 9553 §2.8.1: a date with optional parts, or a UTC timestamp. */
const PartialDateSchema = z.looseObject({
  "@type": z.literal("PartialDate").optional(),
  /** CLDR calendar name ("hebrew", "chinese"…); absent: Gregorian. */
  calendarScale: z.string().nullish(),
  day: z.number().int().nullish(),
  month: z.number().int().nullish(),
  year: z.number().int().nullish(),
});
const TimestampSchema = z.looseObject({
  "@type": z.literal("Timestamp"),
  utc: z.string(),
});

const AnniversarySchema = z.looseObject({
  date: z.union([TimestampSchema, PartialDateSchema]),
  kind: z.string().nullish(),
});
export type ContactAnniversary = z.infer<typeof AnniversarySchema>;

const MediaSchema = z.looseObject({
  blobId: BlobId.nullish(),
  kind: z.string(),
  mediaType: z.string().nullish(),
  pref: z.number().nullish(),
  uri: z.string().nullish(),
});
export type ContactMedia = z.infer<typeof MediaSchema>;

const NicknameSchema = z.looseObject({ name: z.string() });

/** The JSContact Card properties, without the JMAP `id`. */
const cardShape = {
  addressBookIds: z.partialRecord(AddressBookId, z.boolean()).nullish(),
  addresses: z.record(z.string(), AddressSchema).nullish(),
  anniversaries: z.record(z.string(), AnniversarySchema).nullish(),
  created: z.string().nullish(),
  emails: z.record(z.string(), EmailAddressSchema).nullish(),
  kind: z.string().nullish(),
  media: z.record(z.string(), MediaSchema).nullish(),
  /** Group cards (`kind: "group"`): member card uids. */
  members: z.record(z.string(), z.boolean()).nullish(),
  name: NameSchema.nullish(),
  nicknames: z.record(z.string(), NicknameSchema).nullish(),
  notes: z.record(z.string(), NoteSchema).nullish(),
  organizations: z.record(z.string(), OrganizationSchema).nullish(),
  phones: z.record(z.string(), PhoneSchema).nullish(),
  titles: z.record(z.string(), TitleSchema).nullish(),
  uid: z.string().nullish(),
  updated: z.string().nullish(),
};

export const ContactCardSchema = z.looseObject({
  ...cardShape,
  id: ContactCardId,
});
export type ContactCard = z.infer<typeof ContactCardSchema>;
export type ContactCardProperty = keyof typeof ContactCardSchema.shape;

/** A card without server-set properties, as parsed from a vCard. */
export const ParsedCardSchema = z.looseObject(cardShape);
export type ParsedCard = z.infer<typeof ParsedCardSchema>;

/** Every card property except `id`: what `/get` fills in with null. */
export const CONTACT_CARD_FILL_INS: Readonly<Record<string, null>> =
  Object.fromEntries(Object.keys(cardShape).map((key) => [key, null]));

export type ContactCardGetArgs = GetArgs<ContactCardId, ContactCardProperty>;

/** RFC 9610 §3.3. Stalwart matches whole words, not prefixes. */
export interface ContactCardFilterCondition {
  readonly inAddressBook?: AddressBookId;
  readonly uid?: string;
  readonly text?: string;
  readonly name?: string;
  readonly email?: string;
  readonly kind?: string;
}

export type ContactCardSortProperty = "created" | "updated" | "name/given";

export type ContactCardQueryArgs = QueryArgs<
  ContactCardFilterCondition,
  { readonly property: ContactCardSortProperty; readonly isAscending?: boolean }
>;

/** Writable card data (JSContact), passed to the server as is. */
// (Not `Omit<ContactCard, "id">`: Omit over a loose object's index
// signature drops the known properties.)
export type ContactCardCreate = Readonly<z.input<typeof ParsedCardSchema>>;

/** Top-level replacements (`null` removes a property) or deeper paths. */
export type ContactCardPatch =
  | { readonly [K in keyof ContactCardCreate]?: ContactCardCreate[K] | null }
  | PatchObject;

export type ContactCardSetArgs = SetArgs<
  ContactCardId,
  ContactCardCreate,
  ContactCardPatch
>;

/** RFC 9610 §3.5 (`urn:ietf:params:jmap:contacts:parse`). */
export interface ContactCardParseArgs {
  readonly accountId: AccountId;
  readonly blobIds: readonly BlobId[];
  readonly properties?: readonly ContactCardProperty[];
}

export const ContactCardParseResponseSchema = z.looseObject({
  accountId: AccountId,
  notFound: z
    .array(BlobId)
    .nullish()
    .transform((ids) => ids ?? []),
  notParsable: z
    .array(BlobId)
    .nullish()
    .transform((ids) => ids ?? []),
  parsed: z
    .partialRecord(BlobId, ParsedCardSchema)
    .nullish()
    .transform((parsed) => parsed ?? {}),
});
export type ContactCardParseResponse = z.infer<
  typeof ContactCardParseResponseSchema
>;

// ---------------------------------------------------------------- AddressBook

/**
 * RFC 9610 §2: mayWrite = create, change and destroy cards; mayDelete =
 * delete the book itself, as Stalwart does. The bridge puts destroying
 * cards in mayDelete too (DAV unbind; apps/web/src/contacts/sharing.ts).
 */
const AddressBookRightsSchema = z.looseObject({
  mayDelete: z.boolean(),
  mayRead: z.boolean(),
  mayShare: z.boolean().optional(),
  mayWrite: z.boolean(),
});
export type AddressBookRights = z.infer<typeof AddressBookRightsSchema>;

export const AddressBookSchema = z.looseObject({
  description: z.string().nullable(),
  id: AddressBookId,
  isDefault: z.boolean(),
  isSubscribed: z.boolean(),
  myRights: AddressBookRightsSchema,
  name: z.string(),
  /**
   * Who else may use it (principal id → rights), with
   * `urn:ietf:params:jmap:principals`. Null in other accounts' books.
   */
  shareWith: z.partialRecord(PrincipalId, AddressBookRightsSchema).nullish(),
  sortOrder: z.number().int(),
});
export type AddressBook = z.infer<typeof AddressBookSchema>;
export type AddressBookProperty = keyof typeof AddressBookSchema.shape;

export type AddressBookGetArgs = GetArgs<AddressBookId, AddressBookProperty>;

export interface AddressBookCreate {
  readonly name: string;
  readonly description?: string | null;
  readonly sortOrder?: number;
  readonly isSubscribed?: boolean;
  readonly shareWith?: Readonly<
    Partial<Record<PrincipalId, AddressBookRights>>
  > | null;
}

/** Also `shareWith/<principal id>`: that principal's rights; null removes. */
export type AddressBookPatch = Partial<AddressBookCreate> &
  Readonly<Record<`shareWith/${string}`, AddressBookRights | null>>;

export interface AddressBookSetArgs extends SetArgs<
  AddressBookId,
  AddressBookCreate,
  AddressBookPatch
> {
  /** RFC 9610 §2.3: also destroy the cards only in this book. */
  readonly onDestroyRemoveContents?: boolean;
}
