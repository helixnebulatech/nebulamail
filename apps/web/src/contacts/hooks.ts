// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// TanStack Query over the contacts domain functions (browser only, like
// mail). Every change also refreshes the composer's address book
// (mail/use-suggestions.ts), so new contacts are suggested at once.

import type { AccountId, ParsedCard } from "@nebulamail/jmap-client";
import {
  queryOptions,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import type { QueryClient } from "@tanstack/react-query";
import { useEffect } from "react";

import { mailKeys } from "../mail/queries";
import {
  contactsSupported,
  createContact,
  deleteContacts,
  getContact,
  importContacts,
  listAddressBooks,
  listContacts,
  loadBlobPhoto,
  updateContact,
} from "./api";
import type { CardFields, ContactForm, Photo } from "./card";
import { watchContacts } from "./push";
import type { BookRef, ContactRef } from "./ref";
import { contactKey } from "./ref";

export const contactKeys = {
  all: ["contacts"] as const,
  books: () => [...contactKeys.all, "books"] as const,
  card: (ref: ContactRef) =>
    [...contactKeys.all, "card", contactKey(ref)] as const,
  list: () => [...contactKeys.all, "list"] as const,
  photo: (accountId: AccountId | null, blobId: string) =>
    [...contactKeys.all, "photo", accountId ?? "", blobId] as const,
  supported: () => [...contactKeys.all, "supported"] as const,
};

/** The composer's copy of the address book (mail/use-suggestions.ts). */
const COMPOSER_CONTACTS = [...mailKeys.all, "contacts"] as const;

export const contactsSupportedQuery = () =>
  queryOptions({
    queryFn: contactsSupported,
    queryKey: contactKeys.supported(),
    staleTime: Infinity,
  });

export const useContactsSupported = () => useQuery(contactsSupportedQuery());

/**
 * What an `<img>` may show for a card's photo: the `data:` URL, the redrawn
 * blob photo once downloaded (Stalwart 1.0), or undefined (none yet, or a
 * remote one, which loads only on request through the proxy).
 */
export const usePhotoSrc = (
  accountId: AccountId | null,
  photo: Photo | null
): string | undefined => {
  const blob = photo?.kind === "blob" ? photo : null;
  const query = useQuery({
    enabled: blob !== null,
    queryFn: async () =>
      blob === null ? null : await loadBlobPhoto(accountId, blob),
    queryKey: contactKeys.photo(accountId, blob?.blobId ?? ""),
    retry: false,
    staleTime: Infinity,
  });
  if (photo?.kind === "data") {
    return photo.uri;
  }
  return blob === null ? undefined : (query.data ?? undefined);
};

/** Also the /contacts route's loader (lib/route-prefetch.ts). */
export const addressBooksQuery = () =>
  queryOptions({ queryFn: listAddressBooks, queryKey: contactKeys.books() });

export const contactListQuery = () =>
  queryOptions({ queryFn: listContacts, queryKey: contactKeys.list() });

export const useAddressBooks = () => useQuery(addressBooksQuery());

export const useContactList = () => useQuery(contactListQuery());

export const useContact = (ref: ContactRef | undefined) =>
  useQuery({
    enabled: ref !== undefined,
    queryFn: async () => (ref === undefined ? null : await getContact(ref)),
    queryKey: ref === undefined ? contactKeys.all : contactKeys.card(ref),
  });

/**
 * After a change of cards or books: the lists, the open card and the
 * composer's copy. Not the photos (a blob never changes: a new photo has a
 * new blobId, RFC 8620 §6) nor whether the server has contacts.
 */
const refreshContacts = async (client: QueryClient): Promise<void> => {
  await Promise.all([
    client.invalidateQueries({
      predicate: (query) => {
        const [, kind] = query.queryKey;
        return kind !== "photo" && kind !== "supported";
      },
      queryKey: contactKeys.all,
    }),
    client.invalidateQueries({ queryKey: COMPOSER_CONTACTS }),
  ]);
};

const useRefresh = () => {
  const client = useQueryClient();
  return async (): Promise<void> => {
    await refreshContacts(client);
  };
};

/** Live updates of the contacts (JMAP push) while mounted. */
export const useContactsPush = (): void => {
  const client = useQueryClient();
  useEffect(() => {
    const controller = new AbortController();
    // Push is a nicety: without it the list refreshes when it is opened.
    watchContacts(() => {
      void refreshContacts(client);
    }, controller.signal).catch(() => null);
    return () => {
      controller.abort();
    };
  }, [client]);
};

export const useCreateContact = () => {
  const refresh = useRefresh();
  return useMutation({
    mutationFn: async ({
      book,
      form,
    }: {
      readonly form: ContactForm;
      readonly book: BookRef;
    }) => await createContact(form, book),
    onSettled: refresh,
  });
};

export const useUpdateContact = () => {
  const refresh = useRefresh();
  return useMutation({
    mutationFn: async ({
      accountId,
      form,
      original,
    }: {
      readonly accountId: AccountId | null;
      readonly original: CardFields;
      readonly form: ContactForm;
    }) => {
      await updateContact(accountId, original, form);
    },
    onSettled: refresh,
  });
};

export const useDeleteContacts = () => {
  const refresh = useRefresh();
  return useMutation({
    mutationFn: async (refs: readonly ContactRef[]) => {
      await deleteContacts(refs);
    },
    onSettled: refresh,
  });
};

export const useImportContacts = () => {
  const refresh = useRefresh();
  return useMutation({
    mutationFn: async ({
      book,
      cards,
    }: {
      readonly cards: readonly ParsedCard[];
      readonly book: BookRef;
    }) => await importContacts(cards, book),
    onSettled: refresh,
  });
};
