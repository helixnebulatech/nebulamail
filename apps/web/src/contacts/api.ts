// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Contacts domain functions: the contacts screens call
// these, never JMAP. JMAP for Contacts (RFC 9610) in the user's own
// contacts account; checked against Stalwart (quirks in
// packages/jmap-client/src/contacts.ts).

import type {
  BlobId,
  ContactCard,
  JmapClient,
  ParsedCard,
} from "@nebulamail/jmap-client";
import {
  AccountId,
  AddressBookId,
  CAPABILITIES,
  ContactCardId,
  hasCapability,
} from "@nebulamail/jmap-client";

import { getClient } from "../mail/client";
import { chunk } from "../mail/thread-ids";
import type { CardFields, ContactForm, Photo } from "./card";
import {
  displayName,
  emailsOf,
  formProperties,
  importedCard,
  newCard,
  newUid,
  organizationOf,
} from "./card";
import { MAX_PHOTO_FILE_BYTES, photoFromFile } from "./photo";
import type { BookRef, ContactRef } from "./ref";
import { bookKey, contactKey } from "./ref";
import type { ExportPhotos } from "./vcard";
import {
  MAX_IMPORT_CARDS,
  blobPhotoOf,
  cardsToVcf,
  splitVcards,
} from "./vcard";

// --- View types -----------------------------------------------------------------

export interface AddressBookSummary {
  readonly id: AddressBookId;
  /** null: ours; otherwise someone's account shared with us. */
  readonly accountId: AccountId | null;
  /** Unique across accounts (ref.ts). */
  readonly key: string;
  readonly name: string;
  readonly isDefault: boolean;
  /** Create and change cards. */
  readonly mayWrite: boolean;
  /** Destroy cards (see `removesWithWrite`). */
  readonly mayRemove: boolean;
  /** Change who else may use it. */
  readonly mayShare: boolean;
  /** The owner's account name, for books shared with us. */
  readonly owner: string | null;
}

export interface ContactSummary {
  readonly id: ContactCardId;
  readonly accountId: AccountId | null;
  /** Unique across accounts; the URL segment (ref.ts). */
  readonly key: string;
  /** Needed to be a group member (Stalwart doesn't set one by itself). */
  readonly uid: string | null;
  readonly group: boolean;
  /** Cleaned for display (card.ts). */
  readonly name: string;
  readonly emails: readonly string[];
  readonly phones: readonly string[];
  readonly organization: string;
  readonly addressBookIds: readonly AddressBookId[];
  /** Groups: member uids. */
  readonly members: readonly string[];
  /** What its address books let us do (`myRights`). */
  readonly mayWrite: boolean;
  readonly mayRemove: boolean;
  /** Shared with us by this account (its name); null: ours. */
  readonly owner: string | null;
}

const LIST_PROPERTIES = [
  "uid",
  "kind",
  "name",
  "emails",
  "phones",
  "organizations",
  "addressBookIds",
  "members",
] as const;

type ListFields = Pick<ContactCard, (typeof LIST_PROPERTIES)[number] | "id">;

/** What we may do with the cards of some books. */
export interface Rights {
  readonly mayWrite: boolean;
  readonly mayRemove: boolean;
}

const FULL_RIGHTS: Rights = { mayRemove: true, mayWrite: true };

/** Any of the card's books allows it (RFC 9610 rights are per book). */
export const cardRights = (
  bookIds: readonly AddressBookId[],
  books: readonly AddressBookSummary[] | null
): Rights => {
  // Rights unknown (books not loaded): let the server decide.
  if (books === null) {
    return FULL_RIGHTS;
  }
  const mine = books.filter((book) => bookIds.includes(book.id));
  return {
    mayRemove: mine.some((book) => book.mayRemove),
    mayWrite: mine.some((book) => book.mayWrite),
  };
};

export const toSummary = (
  card: ListFields,
  account: {
    readonly accountId: AccountId | null;
    readonly owner: string | null;
    readonly books: readonly AddressBookSummary[] | null;
  }
): ContactSummary => {
  const addressBookIds = Object.entries(card.addressBookIds ?? {}).flatMap(
    ([id, inBook]) => (inBook === true ? [AddressBookId.parse(id)] : [])
  );
  return {
    accountId: account.accountId,
    addressBookIds,
    emails: emailsOf(card),
    group: card.kind === "group",
    id: card.id,
    key: contactKey({ accountId: account.accountId, id: card.id }),
    members: Object.entries(card.members ?? {})
      .filter(([, member]) => member)
      .map(([uid]) => uid),
    name: displayName(card),
    organization: organizationOf(card),
    owner: account.owner,
    phones: Object.values(card.phones ?? {})
      .map((phone) => phone.number.trim())
      .filter((number) => number !== ""),
    uid: card.uid ?? null,
    ...cardRights(addressBookIds, account.books),
  };
};

// --- Calls ----------------------------------------------------------------------

/** Biggest address book loaded into the list (the rest isn't shown). */
export const MAX_CONTACTS = 10_000;

const contactsAccount = (client: JmapClient): AccountId | null => {
  if (!hasCapability(client.session, CAPABILITIES.contacts)) {
    return null;
  }
  try {
    return client.primaryAccountId(CAPABILITIES.contacts);
  } catch {
    return null;
  }
};

interface SharedAccount {
  readonly accountId: AccountId;
  readonly name: string;
}

/** Other accounts of the session with contacts: books shared with us. */
export const sharedContactAccounts = (
  session: JmapClient["session"],
  own: AccountId
): SharedAccount[] =>
  Object.entries(session.accounts)
    .flatMap(([id, account]) =>
      account !== undefined &&
      id !== own &&
      Object.hasOwn(account.accountCapabilities, CAPABILITIES.contacts)
        ? [{ accountId: AccountId.parse(id), name: account.name }]
        : []
    )
    .toSorted((a, b) => a.name.localeCompare(b.name));

const withContacts = async <T>(
  run: (client: JmapClient, accountId: AccountId) => Promise<T>
): Promise<T> => {
  const client = await getClient();
  const accountId = contactsAccount(client);
  if (accountId === null) {
    throw new Error("The server has no contacts");
  }
  return await run(client, accountId);
};

/** In `accountId` (a shared account), or ours when null. */
const withAccount = async <T>(
  accountId: AccountId | null,
  run: (client: JmapClient, accountId: AccountId) => Promise<T>
): Promise<T> =>
  await withContacts(
    async (client, own) => await run(client, accountId ?? own)
  );

type BlobPhoto = Extract<Photo, { readonly kind: "blob" }>;

/**
 * A card's blob photo (Stalwart 1.0), downloaded with the session's
 * credentials and redrawn as a small JPEG `data:` URL: only its pixels
 * reach the page. The type was checked to be a raster one (card.ts).
 */
const redrawnPhoto = async (
  client: JmapClient,
  accountId: AccountId,
  photo: BlobPhoto
): Promise<string> => {
  const blob = await client.downloadBlob({
    accountId,
    blobId: photo.blobId,
    maxBytes: MAX_PHOTO_FILE_BYTES,
    name: "photo",
    type: photo.mediaType,
  });
  return await photoFromFile(new Blob([blob], { type: photo.mediaType }));
};

export const loadBlobPhoto = async (
  accountId: AccountId | null,
  photo: BlobPhoto
): Promise<string> =>
  await withAccount(
    accountId,
    async (client, account) => await redrawnPhoto(client, account, photo)
  );

export const contactsSupported = async (): Promise<boolean> =>
  contactsAccount(await getClient()) !== null;

/**
 * Whether `mayWrite` covers deleting cards. Stalwart puts RemoveItems in
 * `mayWrite` and leaves `mayDelete` for deleting the book; it is known by
 * the `maySetMetadata` right that it always lists. The bridge puts
 * deleting cards in `mayDelete` (sharing.ts).
 */
export const removesWithWrite = (rights: object): boolean =>
  Object.hasOwn(rights, "maySetMetadata");

const getBooks = async (
  client: JmapClient,
  accountId: AccountId,
  shared: SharedAccount | null
): Promise<AddressBookSummary[]> => {
  const [books] = await client.request((b) => [
    b.call("AddressBook/get", {
      accountId,
      properties: ["name", "isDefault", "myRights", "sortOrder"],
    }),
  ]);
  const ownerAccount = shared === null ? null : shared.accountId;
  return books.list
    .toSorted(
      (a, b) =>
        Number(b.isDefault) - Number(a.isDefault) ||
        a.sortOrder - b.sortOrder ||
        a.name.localeCompare(b.name)
    )
    .map((book) => ({
      accountId: ownerAccount,
      id: book.id,
      isDefault: book.isDefault,
      key: bookKey({ accountId: ownerAccount, id: book.id }),
      mayRemove:
        book.myRights.mayWrite &&
        (removesWithWrite(book.myRights) || book.myRights.mayDelete),
      mayShare: book.myRights.mayShare === true,
      mayWrite: book.myRights.mayWrite,
      name: book.name,
      owner: shared?.name ?? null,
    }));
};

/** Our books first, then those shared with us (by owner). */
export const listAddressBooks = async (): Promise<AddressBookSummary[]> =>
  await withContacts(async (client, accountId) => {
    const shared = sharedContactAccounts(client.session, accountId);
    const [own, ...others] = await Promise.all([
      getBooks(client, accountId, null),
      // A share being removed shouldn't break our own list.
      ...shared.map(
        async (account) =>
          await getBooks(client, account.accountId, account).catch(() => [])
      ),
    ]);
    return [...own, ...others.flat()];
  });

export interface ContactList {
  readonly contacts: readonly ContactSummary[];
  /** Cards in the account (more than `contacts` when over MAX_CONTACTS). */
  readonly total: number;
}

/** One page of an account's cards: the query and the get in one request. */
const cardPage = async (
  client: JmapClient,
  accountId: AccountId,
  position: number,
  limit: number
) => {
  const [query, cards] = await client.request((b) => {
    const q = b.call("ContactCard/query", {
      accountId,
      calculateTotal: true,
      limit,
      position,
    });
    const g = b.call("ContactCard/get", {
      "#ids": q.ref("/ids"),
      accountId,
      properties: [...LIST_PROPERTIES],
    });
    return [q, g] as const;
  });
  return { cards: cards.list, ids: query.ids.length, total: query.total };
};

/** An account's books and first page, asked at the same time. */
const startAccount = async (
  client: JmapClient,
  accountId: AccountId,
  shared: SharedAccount | null
) => {
  const page = client.session.core.maxObjectsInGet;
  const [books, first] = await Promise.all([
    getBooks(client, accountId, shared).catch(() => null),
    cardPage(client, accountId, 0, Math.min(page, MAX_CONTACTS)),
  ]);
  return {
    account: {
      accountId: shared === null ? null : shared.accountId,
      books,
      owner: shared?.name ?? null,
    },
    accountId,
    first,
    page,
    /** Null: the server didn't say (RFC 8620 §5.5 lets it). */
    total: first.total ?? null,
  };
};

/**
 * The rest of an account's cards, up to `limit`, all pages at once (the
 * total is known from the first; the client queues them under the
 * session's `maxConcurrentRequests`, RFC 8620 §2), in order.
 */
const finishAccount = async (
  client: JmapClient,
  started: Awaited<ReturnType<typeof startAccount>>,
  limit: number
): Promise<ContactList> => {
  const { account, accountId, first, page } = started;
  const step = first.ids;
  // A short first page is all there is.
  const more = step >= Math.min(page, MAX_CONTACTS);
  if (started.total === null && more) {
    // Without a total, one page after another until a short one.
    const parts = [first];
    let position = step;
    let last = first;
    while (last.ids >= step && position < limit) {
      // oxlint-disable-next-line no-await-in-loop -- each page says whether there is another.
      last = await cardPage(
        client,
        accountId,
        position,
        Math.min(step, limit - position)
      );
      parts.push(last);
      position += step;
    }
    const contacts = parts
      .flatMap((part) => part.cards)
      .slice(0, limit)
      .map((card) => toSummary(card, account));
    return { contacts, total: position - step + last.ids };
  }
  const total = started.total ?? first.ids;
  const positions: number[] = [];
  // The server's pages may be shorter than asked: they go by the length
  // of the first.
  if (more) {
    for (
      let position = step;
      position < Math.min(limit, total);
      position += step
    ) {
      positions.push(position);
    }
  }
  const rest = await Promise.all(
    positions.map(
      async (position) =>
        await cardPage(
          client,
          accountId,
          position,
          Math.min(step, limit - position)
        )
    )
  );
  const contacts = [first, ...rest]
    .flatMap((part) => part.cards)
    .slice(0, limit)
    .map((card) => toSummary(card, account));
  return { contacts, total };
};

/**
 * Our address book and the books shared with us (names, addresses,
 * phones; no photos), page by page of `maxObjectsInGet`, since searching
 * happens in the browser. At most MAX_CONTACTS cards in all: ours first,
 * then each shared account in turn while there is room. Every account
 * starts at once; the totals of their first pages say how much of each
 * fits.
 */
export const listContacts = async (): Promise<ContactList> =>
  await withContacts(async (client, accountId) => {
    const sharedAccounts = sharedContactAccounts(client.session, accountId);
    const [own, ...shared] = await Promise.all([
      startAccount(client, accountId, null),
      // A share being removed shouldn't break our own list.
      ...sharedAccounts.map(
        async (account) =>
          await startAccount(client, account.accountId, account).catch(
            () => null
          )
      ),
    ]);
    // The room each account gets, in order: what the ones before leave.
    let room = MAX_CONTACTS;
    const budgets = [own, ...shared].map((started) => {
      if (started === null || started === undefined) {
        return 0;
      }
      const budget = room;
      room = Math.max(
        0,
        room - Math.min(started.total ?? MAX_CONTACTS, budget)
      );
      return budget;
    });
    const lists = await Promise.all(
      [own, ...shared].map(async (started, index) => {
        const budget = budgets[index] ?? 0;
        if (started === null || started === undefined || budget <= 0) {
          return null;
        }
        if (started === own) {
          return await finishAccount(client, started, budget);
        }
        return await finishAccount(client, started, budget).catch(() => null);
      })
    );
    const contacts: ContactSummary[] = [];
    let total = 0;
    for (const list of lists) {
      if (list !== null) {
        contacts.push(...list.contacts);
        total += list.total;
      }
    }
    return { contacts: contacts.slice(0, MAX_CONTACTS), total };
  });

/** One card with everything (photo included); null if it's gone. */
export const getContact = async (
  ref: ContactRef
): Promise<ContactCard | null> =>
  await withAccount(ref.accountId, async (client, accountId) => {
    const [cards] = await client.request((b) => [
      b.call("ContactCard/get", { accountId, ids: [ref.id] }),
    ]);
    return cards.list[0] ?? null;
  });

const setFailed = (description: string | null | undefined): Error =>
  new Error(description ?? "The server refused the change");

/** Creates a contact in the given book (ours or a writable shared one). */
export const createContact = async (
  form: ContactForm,
  book: BookRef
): Promise<ContactRef> =>
  await withAccount(book.accountId, async (client, accountId) => {
    const [result] = await client.request((b) => [
      b.call("ContactCard/set", {
        accountId,
        create: { card: newCard(form, book.id, newUid()) },
      }),
    ]);
    const created = result.created.card;
    if (created === undefined) {
      throw setFailed(result.notCreated.card?.description);
    }
    return { accountId: book.accountId, id: created.id };
  });

export const updateContact = async (
  accountId: AccountId | null,
  original: CardFields,
  form: ContactForm
): Promise<void> => {
  await withAccount(accountId, async (client, account) => {
    const [result] = await client.request((b) => [
      b.call("ContactCard/set", {
        accountId: account,
        update: { [original.id]: formProperties(form, original) },
      }),
    ]);
    const failed = result.notUpdated[original.id];
    if (failed !== undefined) {
      throw setFailed(failed.description);
    }
  });
};

/** Groups refs by account: one `/set` or `/get` per account. */
const byAccount = (
  refs: readonly ContactRef[]
): [AccountId | null, ContactCardId[]][] => {
  const groups = new Map<AccountId | null, ContactCardId[]>();
  for (const ref of refs) {
    groups.set(ref.accountId, [...(groups.get(ref.accountId) ?? []), ref.id]);
  }
  return [...groups.entries()];
};

export const deleteContacts = async (
  refs: readonly ContactRef[]
): Promise<void> => {
  for (const [accountId, ids] of byAccount(refs)) {
    // oxlint-disable-next-line no-await-in-loop -- usually one account.
    await withAccount(accountId, async (client, account) => {
      // RFC 8620 §5.3: at most maxObjectsInSet objects per /set call.
      let failed = false;
      for (const part of chunk(ids, client.session.core.maxObjectsInSet)) {
        // oxlint-disable-next-line no-await-in-loop -- one after the other.
        const [result] = await client.request((b) => [
          b.call("ContactCard/set", { accountId: account, destroy: part }),
        ]);
        failed ||= Object.keys(result.notDestroyed).length > 0;
      }
      if (failed) {
        throw setFailed(null);
      }
    });
  }
};

/** Changes a group's members (uids). */
export const setGroupMembers = async (
  group: ContactRef,
  members: readonly string[]
): Promise<void> => {
  await withAccount(group.accountId, async (client, accountId) => {
    const [result] = await client.request((b) => [
      b.call("ContactCard/set", {
        accountId,
        update: {
          [group.id]: {
            members:
              members.length === 0
                ? null
                : Object.fromEntries(members.map((uid) => [uid, true])),
          },
        },
      }),
    ]);
    if (result.notUpdated[group.id] !== undefined) {
      throw setFailed(result.notUpdated[group.id]?.description);
    }
  });
};

// --- Import (.vcf attachments) --------------------------------------------------

/** A .vcf bigger than this isn't read (a contact list, not an archive). */
export const MAX_VCF_BYTES = 2 * 1024 * 1024;

export interface ImportCandidate {
  /** Index in the file, for the preview's checkboxes. */
  readonly index: number;
  readonly card: ParsedCard;
  readonly name: string;
  readonly emails: readonly string[];
  readonly organization: string;
}

export interface ImportPreview {
  readonly candidates: readonly ImportCandidate[];
  /** Cards the server couldn't read. */
  readonly unreadable: number;
  /** Cards left out because the file has more than MAX_IMPORT_CARDS. */
  readonly omitted: number;
}

const PREVIEW_ID = ContactCardId.parse("preview");

/** Where a .vcf comes from: an email's attachment or a file on the device. */
export type VcfSource =
  | {
      readonly kind: "attachment";
      readonly blobId: BlobId;
      /** The email's account (shared folders); null: ours. */
      readonly accountId: AccountId | null;
    }
  | { readonly kind: "file"; readonly file: File };

export class VcfTooBigError extends Error {
  constructor() {
    super("The .vcf file is too big");
    this.name = "VcfTooBigError";
  }
}

const readVcf = async (
  client: JmapClient,
  source: VcfSource
): Promise<string> => {
  if (source.kind === "file") {
    if (source.file.size > MAX_VCF_BYTES) {
      throw new VcfTooBigError();
    }
    return await source.file.text();
  }
  const blob = await client.downloadBlob({
    accountId: source.accountId ?? client.primaryAccountId(),
    blobId: source.blobId,
    maxBytes: MAX_VCF_BYTES,
    name: "contacts.vcf",
    type: "application/octet-stream",
  });
  return await blob.text();
};

/**
 * Reads the cards of a .vcf attachment with `ContactCard/parse`, for a
 * preview. Nothing is saved: `importContacts` does that after the user
 * confirms (never automatic). Each card is uploaded to our
 * account on its own and parsed there.
 */
export const previewVcf = async (source: VcfSource): Promise<ImportPreview> =>
  await withContacts(async (client, accountId) => {
    const texts = splitVcards(await readVcf(client, source));
    const wanted = texts.slice(0, MAX_IMPORT_CARDS);
    // Each card on its own: Stalwart parses only the first card of a blob.
    const blobIds: BlobId[] = [];
    for (const text of wanted) {
      // oxlint-disable-next-line no-await-in-loop -- one upload at a time (maxConcurrentUpload).
      const uploaded = await client.upload(
        accountId,
        new Blob([text], { type: "text/vcard" })
      );
      blobIds.push(uploaded.blobId);
    }
    if (blobIds.length === 0) {
      return { candidates: [], omitted: 0, unreadable: 0 };
    }
    const [parsed] = await client.request((b) => [
      b.call("ContactCard/parse", { accountId, blobIds }),
    ]);
    const candidates = blobIds.flatMap((id, index): ImportCandidate[] => {
      const card = parsed.parsed[id];
      if (card === undefined) {
        return [];
      }
      const fields: CardFields = { ...card, id: PREVIEW_ID };
      return [
        {
          card,
          emails: emailsOf(fields),
          index,
          name: displayName(fields),
          organization: organizationOf(fields),
        },
      ];
    });
    return {
      candidates,
      omitted: texts.length - wanted.length,
      unreadable: wanted.length - candidates.length,
    };
  });

export interface ImportResult {
  readonly created: number;
  readonly failed: number;
}

export const importContacts = async (
  cards: readonly ParsedCard[],
  book: BookRef
): Promise<ImportResult> =>
  await withAccount(book.accountId, async (client, accountId) => {
    if (cards.length === 0) {
      return { created: 0, failed: 0 };
    }
    // RFC 8620 §5.3: at most maxObjectsInSet objects per /set call. A
    // chunk that fails as a whole counts as failed; the rest still go.
    let created = 0;
    let firstError: Error | null = null;
    for (const part of chunk(cards, client.session.core.maxObjectsInSet)) {
      try {
        // oxlint-disable-next-line no-await-in-loop -- one after the other.
        const [result] = await client.request((b) => [
          b.call("ContactCard/set", {
            accountId,
            create: Object.fromEntries(
              part.map((card, index) => [
                `c${index}`,
                importedCard(card, book.id, newUid()),
              ])
            ),
          }),
        ]);
        created += Object.keys(result.created).length;
      } catch (error_) {
        firstError ??=
          error_ instanceof Error ? error_ : new Error(String(error_));
      }
    }
    if (created === 0 && firstError !== null) {
      throw firstError;
    }
    return { created, failed: cards.length - created };
  });

// --- Export ---------------------------------------------------------------------

/** Blob photos that go in one export; the rest of the cards go without. */
export const MAX_EXPORT_PHOTOS = 500;

/** Downloaded at the same time (the server allows few, and the app works). */
const EXPORT_PHOTOS_AT_ONCE = 3;

/**
 * The cards' blob photos, as the small redrawn JPEGs the app shows (so a
 * file has at most about 25 MB of them). They come from the session's
 * download URL: nothing else is fetched, and a remote photo stays a URL.
 * One that fails leaves its card without a photo, not the export without
 * the cards.
 */
const exportPhotos = async (
  client: JmapClient,
  cards: readonly {
    readonly accountId: AccountId;
    readonly card: ContactCard;
  }[]
): Promise<{ readonly missing: number; readonly photos: ExportPhotos }> => {
  // Same `blobId`, same bytes: shared by several cards, it's fetched once.
  const wanted = new Map<
    BlobId,
    { readonly accountId: AccountId; readonly photo: BlobPhoto }
  >();
  for (const { accountId, card } of cards) {
    const photo = blobPhotoOf(card);
    if (photo !== null && !wanted.has(photo.blobId)) {
      wanted.set(photo.blobId, { accountId, photo });
    }
  }
  const list = [...wanted.values()].slice(0, MAX_EXPORT_PHOTOS);
  const photos = new Map<string, string>();
  for (let index = 0; index < list.length; index += EXPORT_PHOTOS_AT_ONCE) {
    // A few at a time, in turns.
    // oxlint-disable-next-line no-await-in-loop
    await Promise.all(
      list
        .slice(index, index + EXPORT_PHOTOS_AT_ONCE)
        .map(async ({ accountId, photo }) => {
          try {
            photos.set(
              photo.blobId,
              await redrawnPhoto(client, accountId, photo)
            );
          } catch {
            // Counted below.
          }
        })
    );
  }
  return { missing: wanted.size - photos.size, photos };
};

export interface VcfExport {
  readonly text: string;
  /** Photos left out: over the limit, or they couldn't be read. */
  readonly missingPhotos: number;
}

/** vCard 4.0 text of these cards (full, with photos). */
export const exportVcf = async (
  refs: readonly ContactRef[]
): Promise<VcfExport> =>
  await withContacts(async (client, own) => {
    const page = client.session.core.maxObjectsInGet;
    const chunks: [AccountId, ContactCardId[]][] = [];
    for (const [accountId, ids] of byAccount(refs)) {
      for (let index = 0; index < ids.length; index += page) {
        chunks.push([accountId ?? own, ids.slice(index, index + page)]);
      }
    }
    if (chunks.length === 0) {
      return { missingPhotos: 0, text: "" };
    }
    const responses = await client.request((b) =>
      chunks.map(([accountId, part]) =>
        b.call("ContactCard/get", { accountId, ids: part })
      )
    );
    const cards = responses.flatMap((response) =>
      response.list.map((card) => ({ accountId: response.accountId, card }))
    );
    const { missing, photos } = await exportPhotos(client, cards);
    return {
      missingPhotos: missing,
      text: cardsToVcf(
        cards.map(({ card }) => card),
        photos
      ),
    };
  });
