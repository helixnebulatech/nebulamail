// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Shared folders: folders other users share with us appear
// as other accounts of the JMAP session (`urn:ietf:params:jmap:mail:share`,
// RFC 9670); we share our own folders with `Mailbox.shareWith`, choosing
// people from `Principal/query`.
//
// Stalwart (checked live): the shared account shows up in the
// session at once, with only the shared folders and our rights in
// `myRights`; its `shareWith` is null. `Principal/query` filters match
// whole words only, so the directory is loaded (capped) and searched here.

import type { MailboxId, MailboxRights } from "@nebulamail/jmap-client";
import {
  AccountId,
  CAPABILITIES,
  JmapError,
  PrincipalId,
  hasAccountCapability,
} from "@nebulamail/jmap-client";
import { z } from "zod";

import type { MailboxSummary } from "./api";
import { listMailboxes } from "./api";
import { connectedFor, getClient } from "./client";

export interface SharedAccount {
  readonly accountId: AccountId;
  /** The owner, as the server names the account (usually an address). */
  readonly name: string;
  readonly mailboxes: readonly MailboxSummary[];
}

/** Other accounts of the session that have mail (folders shared with us). */
export const sharedMailAccounts = (
  accounts: Readonly<
    Partial<
      Record<
        AccountId,
        {
          readonly name: string;
          readonly accountCapabilities: Readonly<Record<string, unknown>>;
        }
      >
    >
  >,
  primary: AccountId
): { readonly accountId: AccountId; readonly name: string }[] =>
  Object.entries(accounts)
    .flatMap(([id, account]) =>
      account !== undefined &&
      id !== primary &&
      Object.hasOwn(account.accountCapabilities, CAPABILITIES.mail)
        ? [{ accountId: AccountId.parse(id), name: account.name }]
        : []
    )
    .toSorted((a, b) => a.name.localeCompare(b.name));

/** A session fetched this recently (by connecting) isn't asked for again. */
const FRESH_SESSION_MS = 10_000;

export const listSharedAccounts = async (): Promise<SharedAccount[]> => {
  const client = await getClient();
  // The session may have changed since the client connected (a new
  // share). Right after connecting (the page's first load) it is the one
  // just fetched: one request less.
  const session =
    connectedFor(client) < FRESH_SESSION_MS
      ? client.session
      : await client.refreshSession();
  const others = sharedMailAccounts(
    session.accounts,
    client.primaryAccountId()
  );
  const results = await Promise.all(
    others.map(async (account) => ({
      ...account,
      mailboxes: await listMailboxes(account.accountId).catch(() => []),
    }))
  );
  return results.filter((account) => account.mailboxes.length > 0);
};

// --- Rights -------------------------------------------------------------------

/** What the share dialog offers (RFC 8621 §2.4 rights, grouped). */
export const SHARE_LEVELS = ["read", "write", "manage"] as const;
export type ShareLevel = (typeof SHARE_LEVELS)[number];

const RIGHT_NAMES = [
  "mayAddItems",
  "mayCreateChild",
  "mayDelete",
  "mayReadItems",
  "mayRemoveItems",
  "mayRename",
  "maySetKeywords",
  "maySetSeen",
  "mayShare",
  "maySubmit",
] as const satisfies readonly (keyof MailboxRights)[];

const NONE: MailboxRights = {
  mayAddItems: false,
  mayCreateChild: false,
  mayDelete: false,
  mayReadItems: false,
  mayRemoveItems: false,
  mayRename: false,
  maySetKeywords: false,
  maySetSeen: false,
  mayShare: false,
  maySubmit: false,
};

/**
 * - read: see the emails;
 * - write: also mark read/starred, add, move and delete emails;
 * - manage: also create subfolders, rename and delete the folder.
 * Sending as us (`maySubmit`) and re-sharing (`mayShare`) are never given.
 */
export const rightsFor = (level: ShareLevel): MailboxRights => {
  const read = { ...NONE, mayReadItems: true };
  const write = {
    ...read,
    mayAddItems: true,
    mayRemoveItems: true,
    maySetKeywords: true,
    maySetSeen: true,
  };
  switch (level) {
    case "read": {
      return read;
    }
    case "write": {
      return write;
    }
    case "manage": {
      return {
        ...write,
        mayCreateChild: true,
        mayDelete: true,
        mayRename: true,
      };
    }
    default: {
      return level satisfies never;
    }
  }
};

/** The level these rights match, or "custom" (set by another client). */
export const levelOf = (rights: MailboxRights): ShareLevel | "custom" =>
  SHARE_LEVELS.find((level) => {
    const wanted = rightsFor(level);
    return RIGHT_NAMES.every(
      (name) => (rights[name] ?? false) === (wanted[name] ?? false)
    );
  }) ?? "custom";

// --- People and shares --------------------------------------------------------

export interface Person {
  readonly id: PrincipalId;
  readonly name: string;
  readonly email: string | null;
  /** "individual", "group"… */
  readonly type: string;
}

/**
 * A group (RFC 9670 `type`): one entry that stands for its members, as a
 * Nextcloud or Stalwart group through the bridge, or a Stalwart group in
 * its own JMAP.
 */
export const isGroup = (person: Person): boolean => person.type === "group";

/**
 * What the share dialog shows under a name: the address (unless it is
 * the name itself), and for a group the word for it before, so that a
 * group without an address still reads as one.
 */
export const personDetail = (
  person: {
    readonly name: string;
    readonly email: string | null;
    readonly type: string;
  },
  groupLabel: string
): string | null => {
  const email =
    person.email === null || person.email === "" || person.email === person.name
      ? null
      : person.email;
  if (person.type !== "group") {
    return email;
  }
  return email === null ? groupLabel : `${groupLabel} · ${email}`;
};

/** Someone with access and how (mail, address book or calendar levels). */
export interface Share<L extends string = ShareLevel> {
  readonly person: Person;
  readonly level: L | "custom";
}

/** Directory entries fetched at most (filtered in the browser). */
export const MAX_PRINCIPALS = 500;

const PrincipalsCapability = z.looseObject({
  currentUserPrincipalId: z.string().nullish(),
});

/** Fold for search: lower case, no accents. */
const fold = (text: string): string =>
  text.normalize("NFD").replaceAll(/\p{M}/gu, "").toLowerCase();

/** People whose name or address contains every word typed. */
export const matchPeople = (
  people: readonly Person[],
  query: string,
  exclude: ReadonlySet<PrincipalId> = new Set()
): Person[] => {
  const words = fold(query).split(/\s+/u).filter(Boolean);
  return people.filter((person) => {
    if (exclude.has(person.id)) {
      return false;
    }
    const haystack = fold(`${person.name} ${person.email ?? ""}`);
    return words.every((word) => haystack.includes(word));
  });
};

/** Whether we can share folders here (mail:share and principals). */
export const sharingSupported = async (): Promise<boolean> => {
  const client = await getClient();
  const accountId = client.primaryAccountId();
  // Stalwart announces mail:share per account only.
  return (
    hasAccountCapability(client.session, CAPABILITIES.mailShare, accountId) &&
    hasAccountCapability(client.session, CAPABILITIES.principals, accountId)
  );
};

/** Everyone in the directory (users and groups), without ourselves. */
const loadPeople = async (): Promise<Person[]> => {
  const client = await getClient();
  const accountId = client.primaryAccountId();
  const me = PrincipalsCapability.safeParse(
    client.session.accounts[accountId]?.accountCapabilities[
      CAPABILITIES.principals
    ]
  ).data?.currentUserPrincipalId;
  const [, principals] = await client.request((b) => {
    const query = b.call("Principal/query", {
      accountId,
      limit: Math.min(MAX_PRINCIPALS, client.session.core.maxObjectsInGet),
    });
    const get = b.call("Principal/get", {
      "#ids": query.ref("/ids"),
      accountId,
      properties: ["name", "email", "type"],
    });
    return [query, get] as const;
  });
  return principals.list
    .filter(
      (principal) =>
        principal.id !== me &&
        (principal.type === "individual" || principal.type === "group")
    )
    .map((principal) => ({
      email: principal.email ?? null,
      id: principal.id,
      name: principal.name,
      type: principal.type,
    }))
    .toSorted((a, b) => a.name.localeCompare(b.name));
};

/**
 * Whether the server refused to list or search its users: Stalwart 1.0
 * answers `Principal/query` with `forbidden` unless the administrator
 * allows directory queries (`Sharing.allowDirectoryQueries`).
 * Then nobody can be found to share with; what is already shared
 * still shows and can be changed.
 */
export const isDirectoryClosed = (error: unknown): boolean =>
  error instanceof JmapError &&
  error.detail.kind === "method" &&
  error.detail.type === "forbidden";

/**
 * Everyone we could share with (users and groups), without ourselves;
 * null when the server doesn't let us look (`isDirectoryClosed`).
 */
export const listPeople = async (): Promise<Person[] | null> => {
  try {
    return await loadPeople();
  } catch (error) {
    if (isDirectoryClosed(error)) {
      return null;
    }
    throw error;
  }
};

/** Shortest text the server is asked about (`Principal/query` `text`). */
const MIN_SERVER_SEARCH = 2;
/** Longest one (the bridge ignores longer). */
const MAX_SERVER_SEARCH = 64;
/** Server matches fetched at most per search. */
const MAX_SERVER_MATCHES = 20;

const looksLikeAddress = (text: string): boolean =>
  /^[^\s@]+@[^\s@]+\.[^\s@]+$/u.test(text);

/**
 * The `Principal/query` filter for what was typed in the share dialog, or
 * null when the server isn't asked: an address is looked up exactly (on
 * servers that hide their users, like Baïkal through the bridge, that is
 * the only way to name someone), anything else by name. The directory
 * loaded by `listPeople` misses people the server only finds by search
 * (Nextcloud's groups through the bridge.7).
 */
export const serverSearchFilter = (
  query: string
): { readonly email: string } | { readonly text: string } | null => {
  const text = query.trim();
  if (looksLikeAddress(text)) {
    return { email: text };
  }
  if (
    text.length < MIN_SERVER_SEARCH ||
    text.length > MAX_SERVER_SEARCH ||
    /\p{Cc}/u.test(text)
  ) {
    return null;
  }
  return { text };
};

/** People and groups the server finds for what was typed (not us). */
export const searchPeople = async (query: string): Promise<Person[]> => {
  const filter = serverSearchFilter(query);
  if (filter === null) {
    return [];
  }
  const client = await getClient();
  const accountId = client.primaryAccountId();
  const me = PrincipalsCapability.safeParse(
    client.session.accounts[accountId]?.accountCapabilities[
      CAPABILITIES.principals
    ]
  ).data?.currentUserPrincipalId;
  const [, principals] = await client.request((b) => {
    const search = b.call("Principal/query", {
      accountId,
      filter,
      limit: MAX_SERVER_MATCHES,
    });
    const get = b.call("Principal/get", {
      "#ids": search.ref("/ids"),
      accountId,
      properties: ["name", "email", "type"],
    });
    return [search, get] as const;
  });
  return principals.list
    .filter(
      (principal) =>
        principal.id !== me &&
        (principal.type === "individual" || principal.type === "group")
    )
    .map((principal) => ({
      email: principal.email ?? null,
      id: principal.id,
      name: principal.name,
      type: principal.type,
    }));
};

/**
 * What the dialog offers: the local matches first, then what only the
 * server found (not already shared), at most `limit`.
 */
export const mergeMatches = (
  local: readonly Person[],
  server: readonly Person[],
  taken: ReadonlySet<PrincipalId>,
  limit: number
): Person[] => {
  const seen = new Set(local.map((person) => person.id));
  const extra = server.filter(
    (person) => !seen.has(person.id) && !taken.has(person.id)
  );
  return [...local, ...extra].slice(0, limit);
};

/**
 * Names for people a share names that aren't in the directory (found by
 * a search, or an address the bridge gave an id): one `Principal/get`.
 */
export const describePeople = async (
  ids: readonly PrincipalId[]
): Promise<Person[]> => {
  if (ids.length === 0) {
    return [];
  }
  const client = await getClient();
  const accountId = client.primaryAccountId();
  const [principals] = await client.request((b) => [
    b.call("Principal/get", {
      accountId,
      ids: [...ids],
      properties: ["name", "email", "type"],
    }),
  ]);
  return principals.list.map((principal) => ({
    email: principal.email ?? null,
    id: principal.id,
    name: principal.name,
    type: principal.type,
  }));
};

/** Who one of our folders is shared with, and how. */
export const getShares = async (
  mailboxId: MailboxId,
  people: readonly Person[]
): Promise<Share[]> => {
  const client = await getClient();
  const accountId = client.primaryAccountId();
  const [result] = await client.request((b) => {
    b.using(CAPABILITIES.mailShare);
    return [
      b.call("Mailbox/get", {
        accountId,
        ids: [mailboxId],
        properties: ["shareWith"],
      }),
    ];
  });
  const shareWith = result.list[0]?.shareWith ?? {};
  return Object.entries(shareWith).flatMap(([id, rights]) => {
    if (rights === undefined) {
      return [];
    }
    const person = people.find((candidate) => candidate.id === id) ?? {
      email: null,
      id: PrincipalId.parse(id),
      name: id,
      type: "individual",
    };
    return [{ level: levelOf(rights), person }];
  });
};

/** Shares one of our folders (or changes how); `null` stops sharing. */
export const setShare = async (
  mailboxId: MailboxId,
  principalId: PrincipalId,
  level: ShareLevel | null
): Promise<void> => {
  const client = await getClient();
  const accountId = client.primaryAccountId();
  const [result] = await client.request((b) => {
    b.using(CAPABILITIES.mailShare);
    return [
      b.call("Mailbox/set", {
        accountId,
        update: {
          [mailboxId]: {
            [`shareWith/${principalId}`]:
              level === null ? null : rightsFor(level),
          },
        },
      }),
    ];
  });
  if (result.notUpdated[mailboxId] !== undefined) {
    throw new Error("The server did not change the sharing");
  }
};
