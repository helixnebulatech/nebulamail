// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Where the synced preferences live on the server:
//
// 1. `filenode` (Stalwart): `.nebula-mail/preferences.json` in the
//    account's files. Invisible to mail apps (jmap-client/src/
//    filenode.ts).
// 2. `mail`: an email with the JSON in an unsubscribed folder "Nebula
//    Mail" (any JMAP server, the IMAP bridge too). Other mail apps can show
//    that folder, so it is only used when the user turns it on.
// 3. Neither: the preferences stay on this device.

import type {
  AccountId,
  BlobId,
  EmailId,
  FileNodeId,
  JmapClient,
  MailboxId,
} from "@nebulamail/jmap-client";
import {
  CAPABILITIES,
  fileNodeCreate,
  hasCapability,
} from "@nebulamail/jmap-client";

import { chunk } from "../mail/thread-ids";
import { MAX_DOC_BYTES, parseDoc, serializeDoc } from "./model";
import type { SyncDoc } from "./model";

export type BackendKind = "filenode" | "mail";

export const SYNC_DIR = ".nebula-mail";
export const SYNC_FILE = "preferences.json";
/** Top-level folder of the mail fallback (hidden from our folder list). */
export const SYNC_MAILBOX = "Nebula Mail";
const SYNC_SUBJECT = "Nebula Mail settings";

export interface Remote {
  readonly kind: BackendKind;
  /** The saved document, or `null` if there is none yet. */
  readonly read: () => Promise<SyncDoc | null>;
  readonly write: (doc: SyncDoc) => Promise<void>;
}

/** Which backend the account has (`mail` only if the user allowed it). */
export const backendFor = (
  client: JmapClient,
  allowMail: boolean
): BackendKind | null => {
  const { session } = client;
  const accountId = client.primaryAccountId();
  if (hasCapability(session, CAPABILITIES.fileNode, accountId)) {
    return "filenode";
  }
  return allowMail && hasCapability(session, CAPABILITIES.mail, accountId)
    ? "mail"
    : null;
};

const download = async (
  client: JmapClient,
  accountId: AccountId,
  blobId: BlobId,
  type: string
): Promise<string> => {
  const response = await client.download({
    accountId,
    blobId,
    maxBytes: MAX_DOC_BYTES * 2,
    name: SYNC_FILE,
    type,
  });
  return await response.text();
};

const upload = async (
  client: JmapClient,
  accountId: AccountId,
  text: string,
  type: string
): Promise<BlobId> => {
  const blob = new Blob([text], { type });
  const { blobId } = await client.upload(accountId, blob, { type });
  return blobId;
};

// --- 1. FileNode ------------------------------------------------------------

const findFile = async (
  client: JmapClient,
  accountId: AccountId
): Promise<{
  readonly dir: FileNodeId | null;
  readonly file: FileNodeId | null;
  readonly blobId: BlobId | null;
}> => {
  const [dirs] = await client.request((b) => [
    b.call("FileNode/query", {
      accountId,
      filter: { isTopLevel: true, name: SYNC_DIR },
    }),
  ]);
  const dir = dirs.ids[0] ?? null;
  if (dir === null) {
    return { blobId: null, dir: null, file: null };
  }
  const [, files] = await client.request((b) => {
    const query = b.call("FileNode/query", {
      accountId,
      filter: { name: SYNC_FILE, parentId: dir },
    });
    const get = b.call("FileNode/get", {
      "#ids": query.ref("/ids"),
      accountId,
      properties: ["blobId"],
    });
    return [query, get] as const;
  });
  const [found] = files.list;
  return {
    blobId: found?.blobId ?? null,
    dir,
    file: found?.id ?? null,
  };
};

const fileNodeRemote = (client: JmapClient): Remote => {
  const accountId = client.primaryAccountId();
  return {
    kind: "filenode",
    read: async () => {
      const { blobId } = await findFile(client, accountId);
      return blobId === null
        ? null
        : parseDoc(
            await download(client, accountId, blobId, "application/json")
          );
    },
    write: async (doc) => {
      const blobId = await upload(
        client,
        accountId,
        serializeDoc(doc),
        "application/json"
      );
      const { dir, file } = await findFile(client, accountId);
      const responses = await client.request((b) => {
        if (file !== null) {
          return [
            b.call("FileNode/set", {
              accountId,
              update: { [file]: { blobId } },
            }),
          ];
        }
        const calls = [];
        if (dir === null) {
          calls.push(
            b.call("FileNode/set", {
              accountId,
              create: {
                dir: fileNodeCreate({
                  kind: "directory",
                  name: SYNC_DIR,
                  parentId: null,
                }),
              },
            })
          );
        }
        calls.push(
          b.call("FileNode/set", {
            accountId,
            create: {
              file: fileNodeCreate({
                blobId,
                kind: "file",
                mediaType: "application/json",
                name: SYNC_FILE,
                parentId: dir ?? "#dir",
              }),
            },
          })
        );
        return calls;
      });
      for (const response of responses) {
        const error =
          Object.values(response.notCreated)[0] ??
          Object.values(response.notUpdated)[0];
        if (error !== undefined) {
          throw new Error(error.description ?? error.type);
        }
      }
    },
  };
};

// --- 2. An email in a hidden folder -----------------------------------------

/** RFC 5322 message with the JSON as a base64 attachment-free body. */
export const syncMessage = (doc: SyncDoc, date: Date): string => {
  const json = serializeDoc(doc);
  const bytes = new TextEncoder().encode(json);
  const base64 = btoa(String.fromCodePoint(...bytes));
  const lines = base64.match(/.{1,76}/gu) ?? [];
  return [
    "From: Nebula Mail <settings@nebula-mail.invalid>",
    `Subject: ${SYNC_SUBJECT}`,
    `Date: ${date.toUTCString().replace("GMT", "+0000")}`,
    "MIME-Version: 1.0",
    "Content-Type: application/json; charset=utf-8",
    "Content-Transfer-Encoding: base64",
    "",
    ...lines,
    "",
  ].join("\r\n");
};

/** The JSON back from such a message (`null` if it isn't one). */
export const readSyncMessage = (raw: string): SyncDoc | null => {
  const split = raw.search(/\r?\n\r?\n/u);
  if (split === -1) {
    return null;
  }
  try {
    const binary = atob(raw.slice(split).replaceAll(/\s/gu, ""));
    const bytes = Uint8Array.from(binary, (char) => char.codePointAt(0) ?? 0);
    return parseDoc(new TextDecoder().decode(bytes));
  } catch {
    return null;
  }
};

const findMailbox = async (
  client: JmapClient,
  accountId: AccountId
): Promise<MailboxId | null> => {
  const [result] = await client.request((b) => [
    b.call("Mailbox/get", {
      accountId,
      properties: ["name", "parentId", "role"],
    }),
  ]);
  return (
    result.list.find(
      (box) =>
        box.name === SYNC_MAILBOX &&
        (box.parentId ?? null) === null &&
        (box.role ?? null) === null
    )?.id ?? null
  );
};

const latestEmails = async (
  client: JmapClient,
  accountId: AccountId,
  mailbox: MailboxId
): Promise<{ readonly id: EmailId; readonly blobId: BlobId }[]> => {
  const [, get] = await client.request((b) => {
    const query = b.call("Email/query", {
      accountId,
      filter: { inMailbox: mailbox },
      limit: 10,
      sort: [{ isAscending: false, property: "receivedAt" }],
    });
    const emails = b.call("Email/get", {
      "#ids": query.ref("/ids"),
      accountId,
      properties: ["blobId", "receivedAt"],
    });
    return [query, emails] as const;
  });
  return get.list
    .toSorted((a, b) => (b.receivedAt ?? "").localeCompare(a.receivedAt ?? ""))
    .flatMap((email) =>
      email.blobId === undefined ? [] : [{ blobId: email.blobId, id: email.id }]
    );
};

const mailRemote = (client: JmapClient): Remote => {
  const accountId = client.primaryAccountId();
  return {
    kind: "mail",
    read: async () => {
      const mailbox = await findMailbox(client, accountId);
      if (mailbox === null) {
        return null;
      }
      const [latest] = await latestEmails(client, accountId, mailbox);
      return latest === undefined
        ? null
        : readSyncMessage(
            await download(client, accountId, latest.blobId, "message/rfc822")
          );
    },
    write: async (doc) => {
      let mailbox = await findMailbox(client, accountId);
      if (mailbox === null) {
        const [created] = await client.request((b) => [
          b.call("Mailbox/set", {
            accountId,
            create: {
              box: { isSubscribed: false, name: SYNC_MAILBOX, parentId: null },
            },
          }),
        ]);
        mailbox = created.created.box?.id ?? null;
        if (mailbox === null) {
          throw new Error("The server did not create the settings folder");
        }
      }
      const old = await latestEmails(client, accountId, mailbox);
      const blobId = await upload(
        client,
        accountId,
        syncMessage(doc, new Date()),
        "message/rfc822"
      );
      const target = mailbox;
      const [imported] = await client.request((b) => [
        b.call("Email/import", {
          accountId,
          emails: {
            prefs: {
              blobId,
              keywords: { $seen: true },
              mailboxIds: { [target]: true },
            },
          },
        }),
      ]);
      if (imported.created.prefs === undefined) {
        throw new Error("The server did not save the settings");
      }
      // The older copies go once the new one is in.
      // At most 10, but RFC 8620 §5.3 caps a /set at maxObjectsInSet.
      const ids = old.map((email) => email.id);
      for (const part of chunk(ids, client.session.core.maxObjectsInSet)) {
        // oxlint-disable-next-line no-await-in-loop -- one after the other.
        await client.request((b) => [
          b.call("Email/set", { accountId, destroy: part }),
        ]);
      }
    },
  };
};

export const remoteFor = (client: JmapClient, kind: BackendKind): Remote =>
  kind === "filenode" ? fileNodeRemote(client) : mailRemote(client);
