// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Mail domain functions: the UI calls these, never JMAP.
// They map JMAP objects to small view types and keep JMAP details (keywords,
// patches, result references) in one place.

import type {
  AccountId,
  BlobId,
  Email,
  EmailBodyPart,
  EmailFilter,
  EmailId,
  EmailPatch,
  JmapClient,
  Mailbox,
  MailboxId,
  State,
  ThreadId,
} from "@nebulamail/jmap-client";
import {
  CAPABILITIES,
  MailboxId as MailboxIdSchema,
  hasAccountCapability,
  keywordPatch,
} from "@nebulamail/jmap-client";
import { escapeHtml } from "@nebulamail/mail-render";

import { SYNC_MAILBOX } from "../prefs-sync/remote";
import type { MailAccount } from "./account-scope";
import type { AuthVerdict } from "./auth-results";
import { authVerdict } from "./auth-results";
import { getClient, serverHostname } from "./client";
import { isInlineImage, referencedCids } from "./inline";
import {
  PRELOAD_PROXY_ORIGIN,
  PRELOAD_TOKEN_PROPERTY,
  preloadTokenOf,
} from "./preload-token";
import type { ReceiptRequest } from "./read-receipts";
import { RECEIPT_PROPERTIES, readReceiptRequest } from "./read-receipts";
import type { MailRights } from "./rights";
import { toMailRights } from "./rights";
import { chunk, memberIdsToFetch, newestIds } from "./thread-ids";
import type { UnsubscribeOptions } from "./unsubscribe";
import {
  UNSUBSCRIBE_PROPERTIES,
  unsubscribeHeaders,
  unsubscribeOptions,
} from "./unsubscribe";

// --- View types -------------------------------------------------------------

export type MailboxRole =
  | "inbox"
  | "drafts"
  | "sent"
  | "archive"
  | "junk"
  | "trash";

export interface MailboxSummary {
  readonly id: MailboxId;
  /** The account it belongs to; null: the user's own (account-scope.ts). */
  readonly accountId: MailAccount;
  readonly name: string;
  readonly role: MailboxRole | null;
  readonly parentId: MailboxId | null;
  readonly unread: number;
  readonly total: number;
  /** Other users it is shared with (own folders, mail:share). */
  readonly sharedWith: number;
  /** The user may change who it is shared with. */
  readonly mayShare: boolean;
  /** What the user may do with its emails (rights.ts). */
  readonly rights: MailRights;
}

export interface Person {
  readonly name: string | null;
  readonly email: string;
}

export interface EmailSummary {
  readonly id: EmailId;
  readonly threadId: ThreadId;
  readonly subject: string;
  readonly from: readonly Person[];
  /** Shown instead of the sender in Sent and Drafts. */
  readonly to: readonly Person[];
  readonly receivedAt: string;
  readonly preview: string;
  readonly unread: boolean;
  readonly flagged: boolean;
  readonly hasAttachment: boolean;
  /** Folders it is in (usually one). */
  readonly mailboxIds: readonly MailboxId[];
  /** An unsent draft (`$draft`). */
  readonly draft: boolean;
}

/** What a list row needs to know about each email of its thread. */
export type ThreadMember = Pick<
  EmailSummary,
  | "id"
  | "from"
  | "receivedAt"
  | "unread"
  | "flagged"
  | "draft"
  | "mailboxIds"
  | "hasAttachment"
>;

/**
 * One row of a list: the newest email of a thread that matches the list
 * (JMAP `collapseThreads`) plus every email of that thread, oldest first.
 * Counts, unread and starred are worked out per view (threads.ts).
 */
export interface ConversationRow extends EmailSummary {
  readonly members: readonly ThreadMember[];
}

/** An open conversation: its emails oldest first (light, no bodies). */
export interface ThreadData {
  readonly threadId: ThreadId;
  readonly emails: readonly EmailSummary[];
  /** Emails in the thread, including any left out (`emails` is capped). */
  readonly total: number;
}

export interface Attachment {
  readonly blobId: BlobId;
  readonly name: string;
  readonly type: string;
  readonly size: number;
  /** Set when the email is in another (shared) account. */
  readonly accountId?: AccountId;
}

/** An image referenced from the HTML as `cid:`. */
export interface InlinePart extends Attachment {
  readonly cid: string;
}

export interface EmailDetail extends EmailSummary {
  /** The account it was read from; null: the user's own. */
  readonly accountId: MailAccount;
  readonly to: readonly Person[];
  readonly cc: readonly Person[];
  /** Only set on our own messages (drafts, Sent). */
  readonly bcc: readonly Person[];
  readonly replyTo: readonly Person[];
  readonly sentAt: string | null;
  /** Message-ID, In-Reply-To and References (RFC 5322 §3.6.4). */
  readonly messageId: readonly string[];
  readonly inReplyTo: readonly string[];
  readonly references: readonly string[];
  /** Raw HTML from the server. Only `@nebulamail/mail-render` may display it. */
  readonly html: string | null;
  readonly text: string | null;
  /** A body was bigger than MAX_BODY_BYTES and was cut by the server. */
  readonly truncated: boolean;
  readonly attachments: readonly Attachment[];
  readonly inlineParts: readonly InlinePart[];
  /**
   * Sender check from the receiving server's Authentication-Results
   * (DKIM/SPF/DMARC); null when there's no trustworthy verdict.
   */
  readonly senderVerdict: AuthVerdict | null;
  /** How to unsubscribe (List-Unsubscribe, one-click, unsubscribe.ts). */
  readonly unsubscribe: UnsubscribeOptions;
  /** The raw message (RFC 5322), for .eml download and "view original". */
  readonly blobId: BlobId;
  /** Size of the raw message in bytes. */
  readonly size: number;
  /** The sender asked for a read receipt (read-receipts.ts); else null. */
  readonly readReceipt: ReceiptRequest | null;
  /** Static build: the image proxy's token for this email. */
  readonly preloadToken: string | null;
}

export interface EmailPage {
  readonly emails: readonly ConversationRow[];
  readonly total: number;
  /** Position for the next page, or null at the end. */
  readonly next: number | null;
  /** The JMAP filter of the list, to ask again what it holds (sync.ts). */
  readonly filter: EmailFilter;
  /**
   * The `queryState` the page was read at, to ask `Email/queryChanges`
   * from it (sync.ts); null: unknown, or the rows were changed here.
   */
  readonly queryState: State | null;
}

// --- Mapping ----------------------------------------------------------------

/** The roles the UI treats specially, in display order. */
const ROLE_NAMES = [
  "inbox",
  "drafts",
  "sent",
  "archive",
  "junk",
  "trash",
] as const satisfies readonly MailboxRole[];

const toRole = (role: string | null): MailboxRole | null =>
  ROLE_NAMES.find((name) => name === role) ?? null;

const roleRank = (role: string | null): number => {
  const known = toRole(role);
  return known === null ? ROLE_NAMES.length : ROLE_NAMES.indexOf(known);
};

/** Special folders first in a fixed order, then the rest by sortOrder and name. */
export const sortMailboxes = (
  mailboxes: readonly (Pick<
    Mailbox,
    | "id"
    | "name"
    | "role"
    | "parentId"
    | "sortOrder"
    | "unreadEmails"
    | "totalEmails"
  > &
    Partial<Pick<Mailbox, "shareWith" | "myRights">>)[],
  accountId: MailAccount = null
): MailboxSummary[] =>
  [...mailboxes]
    .toSorted(
      (a, b) =>
        roleRank(a.role) - roleRank(b.role) ||
        a.sortOrder - b.sortOrder ||
        a.name.localeCompare(b.name)
    )
    .map((mailbox) => ({
      accountId,
      id: mailbox.id,
      mayShare: mailbox.myRights?.mayShare === true,
      name: mailbox.name,
      parentId: mailbox.parentId,
      rights: toMailRights(mailbox.myRights),
      role: toRole(mailbox.role),
      sharedWith: Object.keys(mailbox.shareWith ?? {}).length,
      total: mailbox.totalEmails,
      unread: mailbox.unreadEmails,
    }));

const toPeople = (addresses: Email["from"]): Person[] =>
  (addresses ?? []).map((address) => ({
    email: address.email,
    name: address.name ?? null,
  }));

type SummaryFields = Pick<
  Email,
  | "id"
  | "threadId"
  | "subject"
  | "from"
  | "receivedAt"
  | "preview"
  | "keywords"
  | "hasAttachment"
> &
  Partial<Pick<Email, "to" | "mailboxIds">>;

/** The keys of JMAP's `mailboxIds` set (already checked ids). */
const mailboxIdsOf = (record: Email["mailboxIds"] | undefined): MailboxId[] =>
  Object.keys(record ?? {}).map((key) => MailboxIdSchema.parse(key));

type MemberFields = Pick<
  Email,
  "id" | "from" | "receivedAt" | "keywords" | "hasAttachment" | "mailboxIds"
>;

export const toMember = (email: MemberFields): ThreadMember => ({
  draft: email.keywords.$draft === true,
  flagged: email.keywords.$flagged === true,
  from: toPeople(email.from),
  hasAttachment: email.hasAttachment,
  id: email.id,
  mailboxIds: mailboxIdsOf(email.mailboxIds),
  receivedAt: email.receivedAt,
  unread: email.keywords.$seen !== true,
});

export const toSummary = (email: SummaryFields): EmailSummary => ({
  draft: email.keywords.$draft === true,
  flagged: email.keywords.$flagged === true,
  from: toPeople(email.from),
  hasAttachment: email.hasAttachment,
  id: email.id,
  mailboxIds: mailboxIdsOf(email.mailboxIds),
  preview: email.preview,
  receivedAt: email.receivedAt,
  subject: email.subject ?? "",
  threadId: email.threadId,
  to: toPeople(email.to ?? null),
  unread: email.keywords.$seen !== true,
});

const SUMMARY_PROPERTIES = [
  "threadId",
  "subject",
  "from",
  "to",
  "receivedAt",
  "preview",
  "keywords",
  "hasAttachment",
  "mailboxIds",
] as const;

const MEMBER_PROPERTIES = [
  "from",
  "receivedAt",
  "keywords",
  "hasAttachment",
  "mailboxIds",
] as const;

/** Bodies bigger than this are cut by the server and flagged as truncated. */
export const MAX_BODY_BYTES = 2 * 1024 * 1024;

type BodyValues = Email["bodyValues"];

interface JoinedBody {
  readonly content: string | null;
  readonly truncated: boolean;
}

const valueOf = (part: EmailBodyPart, values: BodyValues) =>
  part.partId === undefined || part.partId === null
    ? undefined
    : values[part.partId];

/** Joins the parts of one MIME type, in order (RFC 8621 §4.1.4 textBody). */
const joinParts = (
  parts: readonly EmailBodyPart[],
  values: BodyValues,
  type: string
): JoinedBody => {
  let content: string | null = null;
  let truncated = false;
  for (const part of parts) {
    const value = valueOf(part, values);
    if (part.type?.toLowerCase() === type && value !== undefined) {
      content = (content ?? "") + value.value;
      truncated ||= value.isTruncated === true;
    }
  }
  return { content, truncated };
};

/** A text/plain part as HTML: escaped, one `<br>` per line break. */
const plainPartHtml = (text: string): string =>
  `<div>${escapeHtml(text).replaceAll(/\r?\n|\r/gu, "<br>")}</div>`;

/**
 * The HTML view of an email. RFC 8621 §4.1.4: `htmlBody` is a list of
 * text/plain, text/html, image/*, audio/* and video/* parts "to display
 * (sequentially) as the message body", so a text/plain part between two
 * HTML ones (a footer added by a list, a part of a multipart/mixed) is
 * shown in its place, escaped: it is text, never markup. The joined HTML
 * then goes through the same sanitising as any email. With no
 * text/html part at all there is no HTML view (null): the text view shows
 * `textBody` instead. Images, audio and video stay with the attachments
 * (RFC 8621 lists them there too unless they are in both bodies).
 */
export const htmlBodyContent = (
  parts: readonly EmailBodyPart[],
  values: BodyValues
): JoinedBody => {
  let content = "";
  let hasHtml = false;
  let truncated = false;
  for (const part of parts) {
    const value = valueOf(part, values);
    const type = part.type?.toLowerCase();
    if (
      value === undefined ||
      (type !== "text/html" && type !== "text/plain")
    ) {
      continue;
    }
    hasHtml ||= type === "text/html";
    content += type === "text/html" ? value.value : plainPartHtml(value.value);
    truncated ||= value.isTruncated === true;
  }
  return hasHtml ? { content, truncated } : { content: null, truncated: false };
};

const toAttachment = (
  part: EmailBodyPart,
  accountId: MailAccount
): Attachment | null =>
  part.blobId === undefined || part.blobId === null
    ? null
    : {
        blobId: part.blobId,
        name: part.name ?? "",
        size: part.size ?? 0,
        type: part.type ?? "application/octet-stream",
        ...(accountId === null ? {} : { accountId }),
      };

/**
 * `cid:` images the HTML shows are not attachments to list; every other
 * part is (`isInlineImage`), so nothing is hidden or lost when forwarding.
 */
export const splitAttachments = (
  parts: readonly EmailBodyPart[],
  accountId: MailAccount = null,
  html: string | null = null
): {
  readonly attachments: Attachment[];
  readonly inlineParts: InlinePart[];
} => {
  const attachments: Attachment[] = [];
  const inlineParts: InlinePart[] = [];
  const referenced = referencedCids(html);
  for (const part of parts) {
    const attachment = toAttachment(part, accountId);
    const cid = part.cid?.replaceAll(/^<|>$/gu, "");
    if (attachment === null) {
      continue;
    }
    if (
      cid !== undefined &&
      isInlineImage(
        { cid, disposition: part.disposition, type: attachment.type },
        referenced
      )
    ) {
      inlineParts.push({ ...attachment, cid });
    } else {
      attachments.push(attachment);
    }
  }
  return { attachments, inlineParts };
};

// --- Calls ------------------------------------------------------------------

/** The JMAP account id to use: the given one (checked) or the primary. */
export const resolveAccount = (
  client: JmapClient,
  account: MailAccount
): AccountId => {
  if (account === null) {
    return client.primaryAccountId();
  }
  if (client.session.accounts[account] === undefined) {
    throw new Error("Unknown account");
  }
  return account;
};

const withClient = async <T>(
  run: (client: JmapClient, accountId: AccountId) => Promise<T>,
  account: MailAccount = null
): Promise<T> => {
  const client = await getClient();
  return await run(client, resolveAccount(client, account));
};

/** Creates a folder; used when the account lacks one with a needed role. */
export const createMailbox = async (
  name: string,
  role: MailboxRole | null = null,
  account: MailAccount = null
): Promise<MailboxId> =>
  await withClient(async (client, accountId) => {
    const [result] = await client.request((b) => [
      b.call("Mailbox/set", {
        accountId,
        create: { box: { name, role } },
      }),
    ]);
    const created = result.created?.box;
    if (created === undefined || created === null) {
      throw new Error("The server did not create the mailbox");
    }
    return created.id;
  }, account);

/** Who is signed in (JMAP session `username`). */
export const getAccount = async (): Promise<{ readonly username: string }> => {
  const client = await getClient();
  return { username: client.session.username };
};

export const listMailboxes = async (
  account: MailAccount = null
): Promise<MailboxSummary[]> =>
  await withClient(async (client, accountId) => {
    // Who a folder is shared with: only with mail:share (Stalwart
    // announces it per account only).
    const sharing = hasAccountCapability(
      client.session,
      CAPABILITIES.mailShare,
      accountId
    );
    const [result] = await client.request((b) => {
      if (sharing) {
        b.using(CAPABILITIES.mailShare);
      }
      return [
        b.call("Mailbox/get", {
          accountId,
          properties: [
            "name",
            "role",
            "parentId",
            "sortOrder",
            "unreadEmails",
            "totalEmails",
            "myRights",
            ...(sharing ? (["shareWith"] as const) : []),
          ],
        }),
      ];
    });
    // The folder that may hold the synced settings (prefs-sync/remote.ts)
    // isn't mail.
    const list =
      account === null
        ? result.list.filter(
            (box) =>
              !(
                box.name === SYNC_MAILBOX &&
                (box.parentId ?? null) === null &&
                (box.role ?? null) === null
              )
          )
        : result.list;
    return sortMailboxes(list, account);
  }, account);

export const PAGE_SIZE = 50;

/** Thread emails fetched per list page at most (huge threads are cut). */
const MAX_MEMBERS_PER_PAGE = 4000;

/** Emails shown at most in one open conversation (the newest ones). */
export const MAX_THREAD_EMAILS = 300;

/** Emails of these threads (light properties), in `Email/get` chunks. */
const getMembers = async (
  client: JmapClient,
  accountId: AccountId,
  ids: readonly EmailId[]
): Promise<Map<EmailId, ThreadMember>> => {
  const members = new Map<EmailId, ThreadMember>();
  if (ids.length === 0) {
    return members;
  }
  const responses = await client.request((b) =>
    chunk(ids, client.session.core.maxObjectsInGet).map((part) =>
      b.call("Email/get", {
        accountId,
        ids: part,
        properties: [...MEMBER_PROPERTIES],
      })
    )
  );
  for (const response of responses) {
    for (const email of response.list) {
      members.set(email.id, toMember(email));
    }
  }
  return members;
};

type MemberEmail = Parameters<typeof toMember>[0];

/** Every list: newest first, one row per thread (JMAP `collapseThreads`). */
export const LIST_SORT = [
  { isAscending: false, property: "receivedAt" },
] as const satisfies readonly { isAscending: boolean; property: string }[];

type SummaryEmail = Parameters<typeof toSummary>[0];

/**
 * List rows from their emails and threads: the other emails of each thread
 * (unread, starred, senders, folders) come in a second request, only when
 * some thread has more than one and `prefetched` (the same request's
 * members, see queryEmails) doesn't have them. Either way only the
 * `memberIdsToFetch` choice is used, so the rows don't depend on how the
 * members arrived.
 */
const toRows = async (
  client: JmapClient,
  accountId: AccountId,
  emails: readonly SummaryEmail[],
  threads: readonly { readonly id: ThreadId; readonly emailIds?: EmailId[] }[],
  prefetched: readonly MemberEmail[] = []
): Promise<ConversationRow[]> => {
  const summaries = emails.map((email) => toSummary(email));
  const threadIds = new Map(
    threads.map((thread) => [thread.id, thread.emailIds ?? []])
  );
  const known = new Map<EmailId, ThreadMember>(
    summaries.map((email) => [email.id, email])
  );
  const wanted = memberIdsToFetch(
    summaries.map((email) => threadIds.get(email.threadId) ?? [email.id]),
    known,
    MAX_MEMBERS_PER_PAGE
  );
  const arrived = new Map(prefetched.map((email) => [email.id, email]));
  const fetched = await getMembers(
    client,
    accountId,
    wanted.filter((id) => !arrived.has(id))
  );
  for (const id of wanted) {
    const email = arrived.get(id);
    if (email !== undefined) {
      fetched.set(id, toMember(email));
    }
  }
  return summaries.map((email) => ({
    ...email,
    members: (threadIds.get(email.threadId) ?? [email.id]).flatMap((id) => {
      const member = known.get(id) ?? fetched.get(id);
      return member === undefined ? [] : [member];
    }),
  }));
};

/** Newest first, one row per thread, for any JMAP filter (folder, search). */
export const queryEmails = async (
  filter: EmailFilter,
  position = 0,
  account: MailAccount = null
): Promise<EmailPage> =>
  await withClient(async (client, accountId) => {
    // The threads' other emails in the same request (RFC 8621 §4.10's
    // example: Thread/get's `emailIds` as a back-reference), instead of a
    // second round trip after it. A server may refuse them as too many for
    // one Email/get (RFC 8620 §5.1 `requestTooLarge`, huge threads): only
    // that call fails, and toRows asks for its usual share as before.
    const [query, emails, threads, members] = await client.requestSettled(
      (b) => {
        const q = b.call("Email/query", {
          accountId,
          calculateTotal: true,
          collapseThreads: true,
          filter,
          limit: PAGE_SIZE,
          position,
          sort: LIST_SORT,
        });
        const g = b.call("Email/get", {
          "#ids": q.ref("/ids"),
          accountId,
          properties: [...SUMMARY_PROPERTIES],
        });
        const t = b.call("Thread/get", {
          "#ids": g.ref("/list/*/threadId"),
          accountId,
          properties: ["emailIds"],
        });
        const m = b.call("Email/get", {
          "#ids": t.ref("/list/*/emailIds"),
          accountId,
          properties: [...MEMBER_PROPERTIES],
        });
        return [q, g, t, m] as const;
      }
    );
    if (!query.ok) {
      throw query.error;
    }
    if (!emails.ok) {
      throw emails.error;
    }
    if (!threads.ok) {
      throw threads.error;
    }
    const rows = await toRows(
      client,
      accountId,
      emails.value.list,
      threads.value.list,
      members.ok ? members.value.list : []
    );
    const total = query.value.total ?? emails.value.list.length;
    const end = position + query.value.ids.length;
    return {
      emails: rows,
      filter,
      next: end < total ? end : null,
      queryState: query.value.queryState,
      total,
    };
  }, account);

/**
 * The rows these emails head (sync.ts), by id. An email that is gone is
 * missing from the map.
 */
export const getRows = async (
  client: JmapClient,
  accountId: AccountId,
  ids: readonly EmailId[]
): Promise<Map<EmailId, ConversationRow>> => {
  if (ids.length === 0) {
    return new Map();
  }
  // Usually a handful of ids: one request per chunk, side by side.
  const responses = await Promise.all(
    chunk(ids, client.session.core.maxObjectsInGet).map(
      async (part) =>
        await client.request((b) => {
          const g = b.call("Email/get", {
            accountId,
            ids: part,
            properties: [...SUMMARY_PROPERTIES],
          });
          const t = b.call("Thread/get", {
            "#ids": g.ref("/list/*/threadId"),
            accountId,
            properties: ["emailIds"],
          });
          return [g, t] as const;
        })
    )
  );
  const rows = await toRows(
    client,
    accountId,
    responses.flatMap(([emails]) => emails.list),
    responses.flatMap(([, threads]) => threads.list)
  );
  return new Map(rows.map((row) => [row.id, row]));
};

export const listEmails = async (
  mailboxId: MailboxId,
  position = 0,
  account: MailAccount = null
): Promise<EmailPage> =>
  await queryEmails({ inMailbox: mailboxId }, position, account);

export const getEmail = async (
  id: EmailId,
  account: MailAccount = null
): Promise<EmailDetail | null> =>
  await withClient(async (client, accountId) => {
    const summary = [...SUMMARY_PROPERTIES];
    const [result] = await client.request((b) => [
      b.call("Email/get", {
        accountId,
        bodyProperties: [
          "partId",
          "blobId",
          "type",
          "size",
          "name",
          "disposition",
          "cid",
        ],
        fetchHTMLBodyValues: true,
        fetchTextBodyValues: true,
        ids: [id],
        maxBodyValueBytes: MAX_BODY_BYTES,
        properties: [
          ...summary,
          "to",
          "cc",
          "bcc",
          "replyTo",
          "sentAt",
          "messageId",
          "inReplyTo",
          "references",
          "htmlBody",
          "textBody",
          "attachments",
          "bodyValues",
          "blobId",
          "size",
          ...UNSUBSCRIBE_PROPERTIES,
          ...RECEIPT_PROPERTIES,
          // Static build with an image proxy: the preload token.
          ...(PRELOAD_PROXY_ORIGIN === "" ? [] : [PRELOAD_TOKEN_PROPERTY]),
        ],
      }),
    ]);
    const [email] = result.list;
    if (email === undefined) {
      return null;
    }
    const html = htmlBodyContent(email.htmlBody, email.bodyValues);
    const text = joinParts(email.textBody, email.bodyValues, "text/plain");
    const headers = unsubscribeHeaders(email, serverHostname(client));
    return {
      ...toSummary(email),
      ...splitAttachments(email.attachments, account, html.content),
      accountId: account,
      blobId: email.blobId,
      // Our server's hostname is the authserv-id it writes (Stalwart).
      // Whether "pass" is shown is a setting (useVerifiedBadge); dangers in
      // docs/verified-sender.md.
      senderVerdict: authVerdict(
        headers.authResults,
        headers.serverHostname,
        headers.from
      ),
      size: email.size,
      bcc: toPeople(email.bcc),
      cc: toPeople(email.cc),
      html: html.content,
      inReplyTo: email.inReplyTo ?? [],
      readReceipt: readReceiptRequest(email, email.keywords),
      messageId: email.messageId ?? [],
      preloadToken: preloadTokenOf(email),
      references: email.references ?? [],
      replyTo: toPeople(email.replyTo),
      sentAt: email.sentAt,
      text: text.content,
      to: toPeople(email.to),
      truncated: html.truncated || text.truncated,
      unsubscribe: unsubscribeOptions(headers),
    };
  }, account);

/**
 * The conversation an email belongs to: every email of its thread in any
 * folder, oldest first, without bodies (each open card fetches its own
 * with `getEmail`). Which ones to show is decided per view (threads.ts).
 * null: the email is gone.
 */
export const getThread = async (
  id: EmailId,
  account: MailAccount = null
): Promise<ThreadData | null> =>
  await withClient(async (client, accountId) => {
    // One round trip for the usual thread: the members' Email/get takes
    // the thread's emailIds by result reference (RFC 8620 §3.7; the
    // pointer's arrays are flattened). A thread over the server's
    // maxObjectsInGet makes that call fail (`requestTooLarge`, RFC 8620
    // §5.1), and then the chunked request below fetches them.
    const [, threads, members] = await client.requestSettled((b) => {
      const g = b.call("Email/get", {
        accountId,
        ids: [id],
        properties: ["threadId"],
      });
      const t = b.call("Thread/get", {
        "#ids": g.ref("/list/*/threadId"),
        accountId,
        properties: ["emailIds"],
      });
      const e = b.call("Email/get", {
        "#ids": t.ref("/list/*/emailIds"),
        accountId,
        properties: [...SUMMARY_PROPERTIES],
      });
      return [g, t, e] as const;
    });
    if (!threads.ok) {
      throw threads.error;
    }
    const [thread] = threads.value.list;
    if (thread === undefined) {
      return null;
    }
    const all = thread.emailIds ?? [id];
    const ids = newestIds(all, MAX_THREAD_EMAILS, id);
    const byId = new Map<EmailId, EmailSummary>();
    if (members.ok) {
      for (const email of members.value.list) {
        byId.set(email.id, toSummary(email));
      }
    } else {
      const responses = await client.request((b) =>
        chunk(ids, client.session.core.maxObjectsInGet).map((part) =>
          b.call("Email/get", {
            accountId,
            ids: part,
            properties: [...SUMMARY_PROPERTIES],
          })
        )
      );
      for (const response of responses) {
        for (const email of response.list) {
          byId.set(email.id, toSummary(email));
        }
      }
    }
    return {
      emails: ids.flatMap((emailId) => {
        const email = byId.get(emailId);
        return email === undefined ? [] : [email];
      }),
      threadId: thread.id,
      total: all.length,
    };
  }, account);

/** Some emails of a bulk `Email/set` were not changed (`notUpdated`). */
export class EmailUpdateError extends Error {
  readonly failed: readonly EmailId[];

  constructor(failed: readonly EmailId[]) {
    super(`The server did not update ${failed.length} email(s)`);
    this.name = "EmailUpdateError";
    this.failed = failed;
  }
}

/**
 * Applies `patch` to every email. RFC 8620 §5.3: a `/set` call may carry at
 * most `maxObjectsInSet` objects (session `core` capability, §2), so a big
 * selection goes in several calls, one request each, in order. Every chunk
 * is tried; any email the server didn't update (`notUpdated`, or a chunk
 * that failed as a whole) ends in an error once all are done, so the
 * caller can show the real state again.
 */
const updateEmails = async (
  ids: readonly EmailId[],
  patch: EmailPatch,
  account: MailAccount
): Promise<void> => {
  await withClient(async (client, accountId) => {
    const failed: EmailId[] = [];
    let firstError: Error | null = null;
    for (const part of chunk(ids, client.session.core.maxObjectsInSet)) {
      try {
        // One chunk after the other: the server's limits are per request.
        // oxlint-disable-next-line no-await-in-loop
        const [result] = await client.request((b) => [
          b.call("Email/set", {
            accountId,
            update: Object.fromEntries(part.map((id) => [id, patch])),
          }),
        ]);
        failed.push(
          ...part.filter((id) => result.notUpdated[id] !== undefined)
        );
      } catch (error_) {
        firstError ??=
          error_ instanceof Error ? error_ : new Error(String(error_));
        failed.push(...part);
      }
    }
    if (failed.length === ids.length && firstError !== null) {
      // Nothing changed: the original error says best why.
      throw firstError;
    }
    if (failed.length > 0) {
      throw new EmailUpdateError(failed);
    }
  }, account);
};

export const setSeen = async (
  ids: readonly EmailId[],
  seen: boolean,
  account: MailAccount = null
): Promise<void> => {
  await updateEmails(ids, keywordPatch({ $seen: seen }), account);
};

export const setFlagged = async (
  ids: readonly EmailId[],
  flagged: boolean,
  account: MailAccount = null
): Promise<void> => {
  await updateEmails(ids, keywordPatch({ $flagged: flagged }), account);
};

/** The `Email/set` patch for `moveTo`. */
export const movePatch = (
  mailboxId: MailboxId,
  keywords: Readonly<Record<string, boolean>> = {}
): EmailPatch => ({
  ...keywordPatch(keywords),
  mailboxIds: { [mailboxId]: true },
});

/**
 * Moves to exactly one mailbox (the usual "move" in a mail client), optionally
 * setting or clearing keywords in the same update.
 */
export const moveTo = async (
  ids: readonly EmailId[],
  mailboxId: MailboxId,
  keywords: Readonly<Record<string, boolean>> = {},
  account: MailAccount = null
): Promise<void> => {
  await updateEmails(ids, movePatch(mailboxId, keywords), account);
};

/**
 * Keywords for "Report spam" / "Not spam" (RFC 8621 §2.1, IANA `$junk` and
 * `$notjunk`), set together with the move. Stalwart trains its spam
 * classifier from both the keyword and the move in or out of Junk.
 */
export const SPAM_KEYWORDS = { $junk: true, $notjunk: false } as const;
export const NOT_SPAM_KEYWORDS = { $junk: false, $notjunk: true } as const;

export type UnsubscribeResult =
  | "done"
  /** The sender's server refused or could not be reached. */
  | "failed"
  /**
   * The sender answered with a redirect: not confirmed (RFC 8058 §3.1, the
   * sender MUST NOT redirect) and never followed.
   */
  | "redirected"
  /** The email no longer qualifies for one-click (re-checked by the BFF). */
  | "not-allowed";

/**
 * One-click unsubscribe (RFC 8058): the BFF fetches the email itself and
 * POSTs to the sender through net-guard (server/unsubscribe.ts).
 */
const unsubscribeThroughBff = async (
  emailId: EmailId,
  account: MailAccount = null
): Promise<UnsubscribeResult> => {
  const response = await fetch("/api/unsubscribe", {
    body: JSON.stringify(
      account === null ? { emailId } : { accountId: account, emailId }
    ),
    credentials: "same-origin",
    headers: { "content-type": "application/json" },
    method: "POST",
  });
  if (response.ok) {
    return "done";
  }
  if (response.status === 422) {
    return "not-allowed";
  }
  const body: unknown = await response.json().catch(() => null);
  return typeof body === "object" &&
    body !== null &&
    "error" in body &&
    body.error === "redirected"
    ? "redirected"
    : "failed";
};

/**
 * The static build has no BFF: one-click goes through
 * unsubscribe.html instead (components/mail/unsubscribe-button.tsx).
 */
export const unsubscribeOneClick = import.meta.env.NEBULA_STATIC
  ? async (): Promise<UnsubscribeResult> => await Promise.resolve("not-allowed")
  : unsubscribeThroughBff;
