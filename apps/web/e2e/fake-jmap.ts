// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// A tiny in-memory JMAP server for the browser tests, answered through
// Playwright's `page.route` (nothing leaves the machine). It knows only the
// calls the mail UI makes and keeps the changes (moves, keywords) so a test
// can check what the app asked for. Phase 2: contacts, a quota near its
// limit, the directory, and a folder another user ("b") shares with us,
// whose email ids repeat ours on purpose (caches must be per account).

import type { Page, Route } from "@playwright/test";
import { z } from "zod";

import type { FakeCalendar } from "./fake-calendar";
import {
  CALENDAR_CAPABILITIES,
  CALENDAR_METHODS,
  FakeMethodError,
  SHARED_CALENDAR_CAPABILITIES,
  eventSourceBody,
  fakeCalendar,
} from "./fake-calendar";

export const JMAP_ORIGIN = "http://jmap.test";
const ACCOUNT = "a";
/** bob's account: one folder shared with us. */
export const SHARED_ACCOUNT = "b";

interface FakeMailbox {
  readonly id: string;
  readonly name: string;
  readonly role: string | null;
  readonly sortOrder: number;
  /** Our rights in it (default: all but sharing and sending as the owner). */
  readonly rights?: Readonly<Record<string, boolean>>;
}

export interface FakeEmail {
  readonly id: string;
  readonly subject: string;
  readonly from: { readonly name: string; readonly email: string };
  readonly receivedAt: string;
  readonly html: string;
  mailboxIds: Record<string, true>;
  keywords: Record<string, true>;
  readonly attachment?: string;
  /** An inline image (`cid:` in the HTML), blob `b-<id>-img`. */
  readonly inlineCid?: string;
  /** List-Id header value (newsletters). */
  readonly listId?: string;
  /** blobId of a text/calendar part (an invitation). */
  readonly calendarBlob?: string;
  /** Disposition-Notification-To: the sender asks for a read receipt. */
  readonly readReceiptTo?: string;
  /** Return-Path (`<…>`), as the receiving server wrote it. */
  readonly returnPath?: string;
  /** Authentication-Results our server (authserv-id jmap.test) added. */
  readonly authResults?: string;
  /**
   * A text/plain part after the HTML one in `htmlBody` (part 4), like a
   * footer a mailing list adds in a multipart/mixed (RFC 8621 §4.1.4).
   */
  readonly plainFooter?: string;
}

/** An EmailSubmission with its delivery status (RFC 8621 §7). */
export interface FakeSubmission {
  readonly id: string;
  readonly emailId: string;
  readonly sendAt: string;
  undoStatus: string;
  readonly identityId?: string;
  /** As the app gave it (held sends keep HOLDUNTIL here, like Stalwart). */
  readonly envelope?: {
    readonly mailFrom: {
      readonly email: string;
      readonly parameters: Record<string, string> | null;
    };
    readonly rcptTo: readonly {
      readonly email: string;
      readonly parameters: null;
    }[];
  } | null;
  /**
   * The bridge's vendor property (with `FakeJmap.sendWhenSignedOut`): it
   * goes out even after the user signs out.
   */
  "https://helixnebula.tech/jmap/bridge/sendWhenSignedOut"?: boolean;
  /** Recipient → `{delivered, displayed, smtpReply}`. */
  deliveryStatus: Record<
    string,
    { delivered: string; displayed: string; smtpReply: string }
  > | null;
}

/** A file or folder (FileNode), as Stalwart 1.0 keeps it (filenode-18). */
export interface FakeFile {
  readonly "@type": "Directory" | "File";
  readonly id: string;
  readonly name: string;
  readonly parentId: string | null;
  blobId: string | null;
  readonly mediaType: string | null;
}

/** A Sieve script (RFC 9661) as Stalwart keeps it. */
export interface FakeScript {
  readonly id: string;
  name: string;
  blobId: string;
  isActive: boolean;
}

export interface FakeVacation {
  isEnabled: boolean;
  fromDate: string | null;
  toDate: string | null;
  subject: string | null;
  textBody: string | null;
  htmlBody: string | null;
}

/** A 1×1 red PNG: the inline image's bytes. */
export const PIXEL_PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
  "base64"
);

/** Hostile bits the pipeline must remove (the print test checks them). */
const HOSTILE_HTML = `<p>Print me, please.</p><script>window.parent.postMessage("pwned", "*")</script><img src="x" onerror="window.parent.postMessage('pwned', '*')"><p onclick="alert(1)">Click</p>`;

const MAILBOXES: readonly FakeMailbox[] = [
  { id: "inbox", name: "Inbox", role: "inbox", sortOrder: 0 },
  { id: "drafts", name: "Drafts", role: "drafts", sortOrder: 1 },
  { id: "sent", name: "Sent", role: "sent", sortOrder: 2 },
  { id: "archive", name: "Archive", role: "archive", sortOrder: 3 },
  { id: "junk", name: "Junk", role: "junk", sortOrder: 4 },
  { id: "trash", name: "Trash", role: "trash", sortOrder: 5 },
  { id: "projects", name: "Projects", role: null, sortOrder: 10 },
  { id: "receipts", name: "Receipts", role: null, sortOrder: 11 },
];

/** A newsletter laid out with a fixed 640 px table, like many real ones. */
const WIDE_HTML = `<table width="640" style="width:640px;border-collapse:collapse"><tr><td style="padding:24px;background:#f3f0ff"><h1 style="font-size:28px;margin:0">Weekly digest</h1><p>A layout that assumes a desktop screen, with a very long word: Supercalifragilisticexpialidocious-and-even-longer-than-that.</p></td></tr></table>`;

export const emails = (): FakeEmail[] => {
  const base = Date.parse("2026-10-23T09:00:00Z");
  const list: FakeEmail[] = [
    {
      from: { email: "carla@example.com", name: "Carla Gómez" },
      html: "<p>Hi! Could you review the quarterly report before Friday? Thanks.</p>",
      id: "e1",
      keywords: {},
      mailboxIds: { inbox: true },
      receivedAt: new Date(base).toISOString(),
      subject:
        "Quarterly report review before the Friday meeting with the board",
    },
    {
      from: { email: "news@weekly.example", name: "The Weekly" },
      html: WIDE_HTML,
      id: "e2",
      keywords: { $seen: true },
      listId: "The Weekly <weekly.example>",
      mailboxIds: { inbox: true },
      receivedAt: new Date(base - 3_600_000).toISOString(),
      subject: "Weekly digest",
    },
    {
      attachment: "invoice-october.pdf",
      from: { email: "billing@shop.example", name: "Shop" },
      html: "<p>Your invoice is attached.</p>",
      id: "e3",
      keywords: { $flagged: true },
      mailboxIds: { inbox: true },
      receivedAt: new Date(base - 7_200_000).toISOString(),
      subject: "Your October invoice",
    },
  ];
  for (let index = 4; index <= 14; index += 1) {
    list.push({
      from: { email: `person${index}@example.com`, name: `Person ${index}` },
      html: `<p>Message number ${index}.</p>`,
      id: `e${index}`,
      keywords: index % 3 === 0 ? {} : { $seen: true },
      mailboxIds: { inbox: true },
      receivedAt: new Date(base - index * 86_400_000).toISOString(),
      subject: `Message number ${index}`,
    });
  }
  // Older than the rest (the end of the list); tests open them by URL.
  list.push(
    {
      from: { email: "ana@example.com", name: "Ana Pérez" },
      html: `<p>Here is our team photo:</p><p><img src="cid:photo@example.com" alt="Team photo"><img src="https://tracker.example/open.gif" width="1" height="1"><img src="https://cdn.example/banner.png" alt="Banner"></p>`,
      id: "e15",
      inlineCid: "photo@example.com",
      keywords: { $seen: true },
      mailboxIds: { inbox: true },
      receivedAt: new Date(base - 20 * 86_400_000).toISOString(),
      subject: "Team photo",
    },
    {
      from: { email: "printer@example.com", name: "Pat Printer" },
      html: HOSTILE_HTML,
      id: "e16",
      keywords: { $seen: true },
      mailboxIds: { inbox: true },
      receivedAt: new Date(base - 21 * 86_400_000).toISOString(),
      subject: "Print test",
    }
  );
  return list;
};

export interface FakeCard {
  readonly id: string;
  readonly [property: string]: unknown;
}

/**
 * Bob's address book, shared with us read-only. Its card id repeats one
 * of ours (k1): the app must keep accounts apart. The birthday is in the
 * Hebrew calendar (10 Adar 5784 = 20 March 2024).
 */
export const sharedCards = (): FakeCard[] => [
  {
    addressBookIds: { bbook: true },
    anniversaries: {
      b1: {
        date: {
          "@type": "PartialDate",
          calendarScale: "hebrew",
          day: 10,
          month: 6,
          year: 5784,
        },
        kind: "birth",
      },
    },
    emails: { e1: { address: "dana@example.com" } },
    id: "k1",
    kind: "individual",
    name: { full: "Dana Levi" },
    uid: "urn:uuid:dana",
  },
];

/** Carla's photo: Stalwart 1.0 hands a vCard's inline PHOTO as a blob. */
export const CARD_PHOTO_BLOB = "b-photo-carla";

export const cards = (): FakeCard[] => [
  {
    addressBookIds: { book: true },
    emails: { e1: { address: "carla@example.com" } },
    id: "k1",
    kind: "individual",
    media: {
      p1: { blobId: CARD_PHOTO_BLOB, kind: "photo", mediaType: "image/png" },
    },
    name: { full: "Carla Gómez" },
    phones: { p1: { features: { mobile: true }, number: "+34 600 111 222" } },
    uid: "urn:uuid:carla",
  },
  {
    addressBookIds: { book: true },
    emails: { e1: { address: "bob@example.com" } },
    id: "k2",
    kind: "individual",
    name: {
      components: [
        { kind: "given", value: "Bob" },
        { kind: "surname", value: "Pérez" },
      ],
      full: "Bob Pérez",
    },
    notes: { n1: { note: "Line one\nLine two" } },
    organizations: { o1: { name: "ACME" } },
    uid: "urn:uuid:bob",
  },
  {
    addressBookIds: { book: true },
    id: "k3",
    kind: "group",
    members: { "urn:uuid:carla": true, "urn:uuid:bob": true },
    name: { full: "Team" },
    uid: "urn:uuid:team",
  },
];

const FILENODE = { "urn:ietf:params:jmap:filenode": {} };
/** Stalwart 1.0's account capability: `supportedNodeTypes` (filenode-18). */
const FILENODE_ACCOUNT = {
  "urn:ietf:params:jmap:filenode": {
    mayCreateTopLevelFileNode: true,
    supportedNodeTypes: ["Directory", "File", "SymLink"],
  },
};
const MDN = { "urn:ietf:params:jmap:mdn": {} };
const EMAIL_PUSH = { "urn:ietf:params:jmap:emailpush": {} };

/** Like Stalwart 1.0.1: FUTURERELEASE without arguments, 30 days. */
export const FUTURE_RELEASE_SECONDS = 2_592_000;

/**
 * `fileNode`: the server has files (Stalwart), for synced preferences.
 * `mdn`: it can send read receipts (RFC 9007).
 * `emailPush`: mail inside the push (Stalwart 1.0).
 * `futureRelease`: it can hold messages (undo send, scheduled send).
 */
export const session = (
  fileNode = false,
  mdn = false,
  emailPush = false,
  futureRelease = false
) => ({
  accounts: {
    [ACCOUNT]: {
      accountCapabilities: {
        ...CALENDAR_CAPABILITIES,
        ...(fileNode ? FILENODE_ACCOUNT : {}),
        ...(mdn ? MDN : {}),
        ...(emailPush ? EMAIL_PUSH : {}),
        "urn:ietf:params:jmap:contacts": {},
        "urn:ietf:params:jmap:contacts:parse": {},
        // Like Stalwart: mail:share only per account.
        "urn:ietf:params:jmap:mail:share": {},
        "urn:ietf:params:jmap:principals": { currentUserPrincipalId: "pa" },
        "urn:ietf:params:jmap:quota": {},
        "urn:ietf:params:jmap:mail": {
          emailQuerySortOptions: ["receivedAt"],
          mayCreateTopLevelMailbox: true,
          maxMailboxDepth: 10,
          maxMailboxesPerEmail: 100,
          maxSizeAttachmentsPerEmail: 50_000_000,
          maxSizeMailboxName: 255,
        },
        "urn:ietf:params:jmap:sieve": {
          maxNumberRedirects: 1,
          maxNumberScripts: 100,
          maxSizeScript: 102_400,
          maxSizeScriptName: 512,
          sieveExtensions: [
            "body",
            "copy",
            "fileinto",
            "imap4flags",
            "include",
            "mailbox",
            "mailboxid",
            "mime",
            "regex",
            "reject",
            "vacation",
          ],
        },
        "urn:ietf:params:jmap:submission": futureRelease
          ? {
              maxDelayedSend: FUTURE_RELEASE_SECONDS,
              submissionExtensions: { FUTURERELEASE: [] },
            }
          : { maxDelayedSend: 0, submissionExtensions: {} },
        "urn:ietf:params:jmap:vacationresponse": {},
      },
      isPersonal: true,
      isReadOnly: false,
      name: "jane@example.com",
    },
    [SHARED_ACCOUNT]: {
      accountCapabilities: {
        ...SHARED_CALENDAR_CAPABILITIES,
        "urn:ietf:params:jmap:mail": {
          emailQuerySortOptions: ["receivedAt"],
          mayCreateTopLevelMailbox: false,
          maxMailboxDepth: 10,
          maxMailboxesPerEmail: 100,
          maxSizeAttachmentsPerEmail: 50_000_000,
          maxSizeMailboxName: 255,
        },
        "urn:ietf:params:jmap:mail:share": {},
        // Bob also shares an address book (read only).
        "urn:ietf:params:jmap:contacts": {},
      },
      isPersonal: false,
      isReadOnly: false,
      name: "bob@example.com",
    },
  },
  apiUrl: `${JMAP_ORIGIN}/jmap/`,
  capabilities: {
    ...CALENDAR_CAPABILITIES,
    ...(fileNode ? FILENODE : {}),
    ...(mdn ? MDN : {}),
    ...(emailPush ? EMAIL_PUSH : {}),
    "urn:ietf:params:jmap:contacts": {},
    "urn:ietf:params:jmap:contacts:parse": {},
    "urn:ietf:params:jmap:principals": {},
    "urn:ietf:params:jmap:quota": {},
    "urn:ietf:params:jmap:core": {
      collationAlgorithms: ["i;unicode-casemap"],
      maxCallsInRequest: 16,
      maxConcurrentRequests: 4,
      maxConcurrentUpload: 4,
      maxObjectsInGet: 500,
      maxObjectsInSet: 500,
      maxSizeRequest: 10_000_000,
      maxSizeUpload: 50_000_000,
    },
    "urn:ietf:params:jmap:mail": {},
    "urn:ietf:params:jmap:sieve": {},
    "urn:ietf:params:jmap:submission": {},
    "urn:ietf:params:jmap:vacationresponse": {},
  },
  downloadUrl: `${JMAP_ORIGIN}/jmap/download/{accountId}/{blobId}/{name}?accept={type}`,
  eventSourceUrl: `${JMAP_ORIGIN}/jmap/eventsource/?types={types}&closeafter={closeafter}&ping={ping}`,
  primaryAccounts: {
    "urn:ietf:params:jmap:contacts": ACCOUNT,
    "urn:ietf:params:jmap:calendars": ACCOUNT,
    "urn:ietf:params:jmap:mail": ACCOUNT,
    "urn:ietf:params:jmap:principals": ACCOUNT,
    "urn:ietf:params:jmap:quota": ACCOUNT,
    "urn:ietf:params:jmap:submission": ACCOUNT,
  },
  state: "s1",
  uploadUrl: `${JMAP_ORIGIN}/jmap/upload/{accountId}/`,
  username: "jane@example.com",
});

type Json = Record<string, unknown>;

// What the fake server reads from the app's requests, checked with zod
// (the app's own requests, but a test should fail loudly on a surprise).
const Args = z.record(z.string(), z.unknown());
const List = z.array(z.unknown());
const Body = z.object({
  methodCalls: z.array(z.tuple([z.string(), Args, z.string()])),
});
const Reference = z.object({ path: z.string(), resultOf: z.string() });
const Ids = z.array(z.string()).catch([]);
/**
 * Like the bridge: any `header` condition, or a text over 1 KiB. With
 * `refuseText`, also any `text` condition (the app's search box keeps
 * queries short, so the tests can't reach the real limits).
 */
const refusedFilter = (filter: unknown, refuseText: boolean): boolean => {
  if (typeof filter === "string") {
    return filter.length > 1024;
  }
  if (Array.isArray(filter)) {
    return filter.some((item) => refusedFilter(item, refuseText));
  }
  if (typeof filter === "object" && filter !== null) {
    return Object.entries(filter).some(
      ([key, value]) =>
        key === "header" ||
        (refuseText && key === "text") ||
        refusedFilter(value, refuseText)
    );
  }
  return false;
};

/** The user's address in every fake email's To (see emailObject). */
const JANE = ["jane", "example.com"].join("@");

const contains = (value: string, wanted: unknown): boolean =>
  typeof wanted === "string" &&
  value.toLowerCase().includes(wanted.toLowerCase());

/**
 * RFC 8621 §4.4.1, the part the tests use: AND/OR/NOT, `inMailbox`,
 * `before`/`after` (receivedAt) and `from`/`to`/`cc` (text in the address
 * or name, as loose as a word search). Other conditions match everything,
 * as before.
 */
const matchesFilter = (email: FakeEmail, filter: unknown): boolean => {
  const parsed = z.record(z.string(), z.unknown()).safeParse(filter);
  if (!parsed.success) {
    return true;
  }
  const record = parsed.data;
  if (typeof record.operator === "string" && Array.isArray(record.conditions)) {
    const results = record.conditions.map((item: unknown) =>
      matchesFilter(email, item)
    );
    if (record.operator === "OR") {
      return results.some(Boolean);
    }
    return record.operator === "NOT"
      ? !results.some(Boolean)
      : results.every(Boolean);
  }
  const checks: Readonly<Record<string, (value: unknown) => boolean>> = {
    after: (value) => typeof value === "string" && email.receivedAt >= value,
    before: (value) => typeof value === "string" && email.receivedAt < value,
    cc: () => false,
    from: (value) =>
      contains(email.from.email, value) || contains(email.from.name, value),
    inMailbox: (value) =>
      typeof value === "string" && email.mailboxIds[value] === true,
    to: (value) => contains(JANE, value),
  };
  return Object.entries(record).every(
    ([key, value]) => checks[key]?.(value) ?? true
  );
};

const QueryArgs = z.object({
  filter: z.object({ inMailbox: z.string().optional() }).catch({}),
  limit: z.number().catch(50),
  position: z.number().catch(0),
});
const SetArgs = z.object({
  create: z.record(z.string(), Args).catch({}),
  destroy: Ids,
  update: z.record(z.string(), Args).catch({}),
});
const BodyRequest = z.object({ fetchHTMLBodyValues: z.literal(true) });
const CardSetArgs = z.object({
  create: z.record(z.string(), Args).catch({}),
  destroy: Ids,
  update: z.record(z.string(), Args).catch({}),
});
const AccountArgs = z.object({ accountId: z.string().catch(ACCOUNT) });
const ChangesArgs = z.object({ sinceState: z.string().catch("1") });
const MailboxIds = z.record(z.string(), z.literal(true));

/** RFC 8620 §3.7 result reference paths, e.g. "/ids" or "/list/(star)/threadId". */
const evaluatePath = (value: unknown, path: string): unknown => {
  const [head, ...rest] = path.split("/").filter((part) => part !== "");
  if (head === undefined) {
    return value;
  }
  const items = List.safeParse(value);
  if (head === "*" && items.success) {
    return items.data.flatMap((item) => {
      const result = evaluatePath(item, rest.join("/"));
      const list = List.safeParse(result);
      // Nulls are skipped (Stalwart 1.0, references/jsptr.rs).
      if (list.success) {
        return list.data;
      }
      return result === null || result === undefined ? [] : [result];
    });
  }
  const parsed = Args.safeParse(value);
  return evaluatePath(
    parsed.success ? parsed.data[head] : undefined,
    rest.join("/")
  );
};

export interface FakeJmap {
  /** Every `Email/set` update the app sent, in order. */
  readonly updates: Json[];
  /** Every email the app created with `Email/set` (drafts), in order. */
  readonly created: Json[];
  readonly emails: FakeEmail[];
  /** Full emails (with bodies) of these ids fail with `serverFail`. */
  readonly failBodies: Set<string>;
  /** While set, requests for full emails wait for it. */
  bodiesGate: Promise<void> | null;
  /** While set, uploads wait for it (to see the progress bar). */
  uploadsGate: Promise<void> | null;
  /** Uploads answer 500 (after the gate, if any). */
  uploadsFail: boolean;
  /** The server can't be reached: every request fails like offline. */
  down: boolean;
  /** The event source answers 503: no push (the app polls instead). */
  pushDown: boolean;
  /** Principal/query lists PRINCIPALS (otherwise the calendar fake: none). */
  directory: boolean;
  /** Principal/query is `forbidden`, as Stalwart 1.0 by default. */
  directoryClosed: boolean;
  /** The session announces FileNode (off: like a server without files). */
  fileNode: boolean;
  /** The session announces `emailpush` (Stalwart 1.0). */
  emailPush: boolean;
  /** Files and folders (FileNode/*). */
  readonly files: FakeFile[];
  /** Sieve scripts and their contents (blobs by id), like Stalwart. */
  readonly scripts: FakeScript[];
  readonly blobs: Map<string, string>;
  vacation: FakeVacation;
  /** Every `VacationResponse/set` patch, in order. */
  readonly vacationUpdates: Json[];
  /** Emails of the shared account (same ids as ours, other content). */
  readonly sharedEmails: FakeEmail[];
  readonly cards: FakeCard[];
  /** Cards of the address book Bob shares with us. */
  readonly sharedCards: FakeCard[];
  /** Every `ContactCard/set` the app sent. */
  readonly cardSets: Json[];
  /** Who our address book is shared with (principal id → rights). */
  readonly bookShares: Record<string, Json>;
  /** Every `AddressBook/set` the app sent. */
  readonly bookSets: Json[];
  /** Every `Mailbox/set` the app sent (sharing). */
  readonly mailboxSets: Json[];
  /** Every `Blob/copy` the app sent. */
  readonly blobCopies: Json[];
  /** accountId of every method call, in order. */
  readonly accountsUsed: string[];
  /**
   * Mail delivered during the test (`deliver`): each one moves the Email
   * state on by one, and `Email/changes` lists it as created.
   */
  readonly arrivals: string[];
  /** Calendars, events and what the app wrote (fake-calendar.ts). */
  readonly calendar: FakeCalendar;
  /** The session announces MDN (read receipts); off by default. */
  mdn: boolean;
  /**
   * The session announces FUTURERELEASE (held sends: undo, scheduled);
   * off by default, as most tests expect sends to go out at once.
   */
  futureRelease: boolean;
  /**
   * The server is the bridge with `[submission] send_while_signed_out`:
   * its capability says `sendWhenSignedOut` and submissions carry the
   * vendor property.
   */
  sendWhenSignedOut: boolean;
  /**
   * The latest HOLDUNTIL accepted (ISO), like Stalwart 1.0.1's real 7 days
   * under the 30 advertised; later ones are refused with its wording.
   */
  releaseMax: string | null;
  /** Submissions with delivery status (EmailSubmission/query, /get). */
  readonly submissions: FakeSubmission[];
  /** Every `MDN/send` the app sent. */
  readonly mdnSends: Json[];
  /** Data types the next event-source connection says changed. */
  pushChanges: string[];
  /** Email/query refuses `text` conditions (`unsupportedFilter`). */
  refuseSearches: boolean;
  /**
   * How a mail send (`EmailSubmission/set` with `onSuccessUpdateEmail`)
   * ends: "ok" (sent and filed), "notFiled" (sent, the implicit Email/set
   * fails) or "unknown" (the bridge's `outcome_unknown`: `notCreated`
   * `forbiddenToSend`, the submission row kept).
   */
  submitOutcome: "ok" | "notFiled" | "unknown";
  /** Every mail send (`EmailSubmission/set` with `onSuccessUpdateEmail`). */
  readonly mailSends: Json[];
  /** The user's identities (Identity/get); one, without Reply-To or Bcc. */
  readonly identities: FakeIdentity[];
}

const SHARED_MAILBOXES: readonly FakeMailbox[] = [
  { id: "team", name: "Team inbox", role: null, sortOrder: 0 },
  // Read only (myRights): nothing that changes it is offered.
  {
    id: "reports",
    name: "Reports",
    rights: {
      mayAddItems: false,
      mayCreateChild: false,
      mayDelete: false,
      mayReadItems: true,
      mayRemoveItems: false,
      mayRename: false,
      maySetKeywords: false,
      maySetSeen: false,
      mayShare: false,
      maySubmit: false,
    },
    role: null,
    sortOrder: 1,
  },
];

export const sharedEmails = (): FakeEmail[] => [
  {
    from: { email: "bob@example.com", name: "Bob Pérez" },
    html: "<p>This lives in Bob's shared folder.</p>",
    // Same id as Carla's email in our inbox.
    id: "e1",
    keywords: {},
    mailboxIds: { team: true },
    receivedAt: "2026-10-23T08:00:00Z",
    subject: "Hello from the shared folder",
  },
  {
    attachment: "budget.pdf",
    from: { email: "bob@example.com", name: "Bob Pérez" },
    html: '<p>Figures attached.</p><p><img src="cid:chart@bob" alt="Chart"></p>',
    id: "s2",
    inlineCid: "chart@bob",
    keywords: {},
    mailboxIds: { reports: true },
    receivedAt: "2026-10-22T08:00:00Z",
    subject: "Monthly report from Bob",
  },
];

const PRINCIPALS = [
  { email: "jane@example.com", id: "pa", name: "Jane", type: "individual" },
  { email: "bob@example.com", id: "pb", name: "Bob Pérez", type: "individual" },
  { email: "eve@example.com", id: "pe", name: "Eve Núñez", type: "individual" },
];

/**
 * Only found by searching its name, like a Nextcloud group through the
 * bridge (no address, so the directory misses it).
 */
const SEARCH_ONLY = [{ email: null, id: "pg", name: "Staff", type: "group" }];

const PrincipalFilter = z
  .object({ email: z.string().optional(), text: z.string().optional() })
  .catch({});

/** `Principal/query`: everyone, or (with a filter) as the bridge does. */
const principalIds = (args: Json): string[] => {
  const { email, text } = PrincipalFilter.parse(args.filter);
  if (email !== undefined) {
    return PRINCIPALS.filter((p) => p.email === email.toLowerCase()).map(
      (p) => p.id
    );
  }
  if (text !== undefined) {
    const wanted = text.toLowerCase();
    return [...PRINCIPALS, ...SEARCH_ONLY]
      .filter((p) =>
        `${p.name} ${p.email ?? ""}`.toLowerCase().includes(wanted)
      )
      .map((p) => p.id);
  }
  return PRINCIPALS.map((p) => p.id);
};

const isShared = (args: Json): boolean =>
  AccountArgs.parse(args).accountId === SHARED_ACCOUNT;

const READ = {
  mayAddItems: false,
  mayCreateChild: false,
  mayDelete: false,
  mayReadItems: true,
  mayRemoveItems: false,
  mayRename: false,
  maySetKeywords: false,
  maySetSeen: false,
  mayShare: false,
  maySubmit: false,
};

/** Projects is shared with Bob; shared accounts' folders show null. */
const shareWithOf = (box: FakeMailbox, shared: boolean) => {
  if (shared) {
    return null;
  }
  return box.id === "projects" ? { pb: READ } : {};
};

/** Adds a script to the fake account (as if another client wrote it). */
export const addScript = (
  state: FakeJmap,
  name: string,
  content: string,
  isActive: boolean
): void => {
  const blobId = `blob-${String(state.blobs.size + 1)}`;
  state.blobs.set(blobId, content);
  if (isActive) {
    for (const script of state.scripts) {
      script.isActive = false;
    }
  }
  state.scripts.push({
    blobId,
    id: `s${String(state.scripts.length + 1)}`,
    isActive,
    name,
  });
};

const mailboxObject = (
  box: FakeMailbox,
  all: readonly FakeEmail[],
  shared = false
) => {
  const inBox = all.filter((email) => email.mailboxIds[box.id] === true);
  const unread = inBox.filter((email) => email.keywords.$seen !== true).length;
  return {
    id: box.id,
    isSubscribed: true,
    myRights: box.rights ?? {
      mayAddItems: true,
      mayCreateChild: true,
      mayDelete: box.role === null,
      mayReadItems: true,
      mayRemoveItems: true,
      mayRename: box.role === null,
      maySetKeywords: true,
      maySetSeen: true,
      mayShare: !shared,
      maySubmit: !shared,
    },
    name: box.name,
    parentId: null,
    role: box.role,
    shareWith: shareWithOf(box, shared),
    sortOrder: box.sortOrder,
    totalEmails: inBox.length,
    totalThreads: inBox.length,
    unreadEmails: unread,
    unreadThreads: unread,
  };
};

const emailObject = (email: FakeEmail) => ({
  attachments: [
    ...(email.attachment === undefined
      ? []
      : [
          {
            blobId: `b-${email.id}-att`,
            cid: null,
            disposition: "attachment",
            name: email.attachment,
            partId: "2",
            size: 48_213,
            type: "application/pdf",
          },
        ]),
    ...(email.inlineCid === undefined
      ? []
      : [
          {
            blobId: `b-${email.id}-img`,
            cid: `<${email.inlineCid}>`,
            disposition: "inline",
            name: "photo.png",
            partId: "3",
            size: PIXEL_PNG.length,
            type: "image/png",
          },
        ]),
    ...(email.calendarBlob === undefined
      ? []
      : [
          {
            blobId: email.calendarBlob,
            disposition: null,
            name: null,
            partId: "3",
            size: 900,
            type: "text/calendar",
          },
        ]),
  ],
  blobId: `b-${email.id}`,
  bcc: null,
  bodyValues: {
    "1": { isTruncated: false, value: email.html },
    ...(email.plainFooter === undefined
      ? {}
      : { "4": { isTruncated: false, value: email.plainFooter } }),
  },
  cc: null,
  from: [email.from],
  hasAttachment: email.attachment !== undefined,
  "header:Authentication-Results:asText:all":
    email.authResults === undefined ? [] : [email.authResults],
  "header:DKIM-Signature:asText:all": [],
  "header:List-Unsubscribe-Post:asText": null,
  "header:List-Id:asText": email.listId ?? null,
  "header:List-Unsubscribe:asURLs": null,
  "header:Disposition-Notification-To:asAddresses":
    email.readReceiptTo === undefined
      ? null
      : [{ email: email.readReceiptTo, name: null }],
  "header:Return-Path": email.returnPath ?? null,
  headers: [],
  htmlBody: [
    { partId: "1", size: email.html.length, type: "text/html" },
    ...(email.plainFooter === undefined
      ? []
      : [
          {
            partId: "4",
            size: email.plainFooter.length,
            type: "text/plain",
          },
        ]),
  ],
  id: email.id,
  inReplyTo: null,
  keywords: email.keywords,
  mailboxIds: email.mailboxIds,
  messageId: [`${email.id}@example.com`],
  preview: email.html
    .replaceAll(/<[^>]+>/gu, " ")
    .trim()
    .slice(0, 120),
  receivedAt: email.receivedAt,
  references: null,
  replyTo: null,
  sentAt: email.receivedAt,
  size: 2048,
  subject: email.subject,
  textBody: [{ partId: "1", size: email.html.length, type: "text/html" }],
  threadId: `t-${email.id}`,
  to: [{ email: "jane@example.com", name: "Jane" }],
});

/** Applies one `Email/set` patch (mailboxIds and keywords/… only). */
const applyPatch = (email: FakeEmail, patch: Json): void => {
  for (const [key, value] of Object.entries(patch)) {
    if (key === "mailboxIds") {
      email.mailboxIds = MailboxIds.parse(value);
    } else if (key.startsWith("mailboxIds/")) {
      const mailbox = key.slice("mailboxIds/".length);
      const others = Object.entries(email.mailboxIds).filter(
        ([id]) => id !== mailbox
      );
      email.mailboxIds = Object.fromEntries(
        value === true ? [...others, [mailbox, true]] : others
      );
    } else if (key.startsWith("keywords/")) {
      const keyword = key.slice("keywords/".length);
      const others = Object.entries(email.keywords).filter(
        ([name]) => name !== keyword
      );
      email.keywords = Object.fromEntries(
        value === true ? [...others, [keyword, true]] : others
      );
    }
  }
};

/** The Email state: "1", plus one per delivered email. */
const emailStateOf = (state: FakeJmap): string =>
  String(1 + state.arrivals.length);

/** New mail arrives (inbox): the next push says EmailDelivery changed. */
export const deliver = (state: FakeJmap, email: FakeEmail): void => {
  state.emails.unshift(email);
  state.arrivals.push(email.id);
};

const emailChanges = (state: FakeJmap, args: Json) => {
  const { sinceState } = ChangesArgs.parse(args);
  const since = Number(sinceState);
  if (!Number.isInteger(since) || since < 1) {
    return null;
  }
  return {
    accountId: ACCOUNT,
    created: state.arrivals.slice(since - 1),
    destroyed: [],
    hasMoreChanges: false,
    newState: emailStateOf(state),
    oldState: sinceState,
    updated: [],
  };
};

const noChanges = (args: Json) => {
  const { sinceState } = ChangesArgs.parse(args);
  return {
    accountId: ACCOUNT,
    created: [],
    destroyed: [],
    hasMoreChanges: false,
    newState: sinceState,
    oldState: sinceState,
    updated: [],
  };
};

export interface FakeIdentity {
  readonly id: string;
  readonly name: string;
  readonly email: string;
  /** RFC 8621 §6; null when unset, as Stalwart writes it. */
  readonly replyTo: readonly { email: string; name: string | null }[] | null;
  readonly bcc: readonly { email: string; name: string | null }[] | null;
  readonly htmlSignature: string;
  readonly textSignature: string;
  readonly mayDelete: boolean;
}

const IDENTITY: FakeIdentity = {
  bcc: null,
  email: "jane@example.com",
  htmlSignature: "<p>Jane from Nebula</p>",
  id: "i1",
  mayDelete: false,
  name: "Jane",
  replyTo: null,
  textSignature: "",
};

type Method = (state: FakeJmap, args: Json) => Json;

/** The emails of the account a call is for. */
const emailsFor = (state: FakeJmap, args: Json): FakeEmail[] =>
  isShared(args) ? state.sharedEmails : state.emails;

const METHODS: Readonly<Record<string, Method>> = {
  // RFC 8620 §6.3: the copy of blob X is "copy-X" in our account.
  "Blob/copy": (state, args) => {
    state.blobCopies.push(args);
    const blobIds = Ids.parse(args.blobIds);
    return {
      accountId: ACCOUNT,
      copied: Object.fromEntries(blobIds.map((id) => [id, `copy-${id}`])),
      fromAccountId: String(args.fromAccountId),
      notCopied: null,
    };
  },
  "AddressBook/get": (state, args) => ({
    accountId: AccountArgs.parse(args).accountId,
    list: isShared(args)
      ? [
          {
            description: null,
            id: "bbook",
            isDefault: true,
            isSubscribed: true,
            // Stalwart 1.0: mayWrite includes deleting cards, mayDelete
            // is the book itself, and maySetMetadata is always listed.
            myRights: {
              mayDelete: false,
              mayRead: true,
              maySetMetadata: false,
              mayShare: false,
              mayWrite: false,
            },
            name: "Team contacts",
            // Like Stalwart: other accounts' books show null.
            shareWith: null,
            sortOrder: 0,
          },
        ]
      : [
          {
            description: null,
            id: "book",
            isDefault: true,
            isSubscribed: true,
            myRights: {
              mayDelete: true,
              mayRead: true,
              maySetMetadata: true,
              mayShare: true,
              mayWrite: true,
            },
            name: "Personal",
            shareWith: state.bookShares,
            sortOrder: 0,
          },
        ],
    notFound: [],
    state: "1",
  }),
  "AddressBook/set": (state, args) => {
    const { update } = CardSetArgs.parse(args);
    state.bookSets.push(args);
    for (const patch of Object.values(update)) {
      for (const [path, rights] of Object.entries(patch)) {
        const principal = path.replace(/^shareWith\//u, "");
        if (rights === null) {
          // oxlint-disable-next-line typescript/no-dynamic-delete -- a fake server's map.
          delete state.bookShares[principal];
        } else {
          state.bookShares[principal] = Args.parse(rights);
        }
      }
    }
    return {
      accountId: ACCOUNT,
      newState: "2",
      oldState: "1",
      updated: Object.fromEntries(Object.keys(update).map((id) => [id, null])),
    };
  },
  "ContactCard/get": (state, args) => {
    const ids = Ids.parse(args.ids);
    return {
      accountId: AccountArgs.parse(args).accountId,
      list: (isShared(args) ? state.sharedCards : state.cards).filter((card) =>
        ids.includes(card.id)
      ),
      notFound: [],
      state: "1",
    };
  },
  "ContactCard/query": (state, args) => {
    const { limit, position } = QueryArgs.parse(args);
    const ids = (isShared(args) ? state.sharedCards : state.cards).map(
      (card) => card.id
    );
    return {
      accountId: ACCOUNT,
      canCalculateChanges: false,
      ids: ids.slice(position, position + limit),
      position,
      queryState: "1",
      total: ids.length,
    };
  },
  "ContactCard/set": (state, args) => {
    const { create, destroy, update } = CardSetArgs.parse(args);
    state.cardSets.push(args);
    const created: Record<string, { id: string }> = {};
    for (const [key, card] of Object.entries(create)) {
      const id = `new${state.cards.length + 1}`;
      state.cards.push({ ...card, id });
      created[key] = { id };
    }
    for (const [id, patch] of Object.entries(update)) {
      const index = state.cards.findIndex((card) => card.id === id);
      const card = state.cards[index];
      if (card !== undefined) {
        state.cards[index] = { ...card, ...patch, id };
      }
    }
    const kept = state.cards.filter((card) => !destroy.includes(card.id));
    state.cards.splice(0, state.cards.length, ...kept);
    return {
      accountId: ACCOUNT,
      created,
      destroyed: destroy,
      newState: "2",
      oldState: "1",
      updated: Object.fromEntries(Object.keys(update).map((id) => [id, null])),
    };
  },
  "Email/changes": (state, args) =>
    emailChanges(state, args) ?? noChanges(args),
  "Email/get": (state, args) => {
    const ids = Ids.parse(args.ids);
    return {
      accountId: AccountArgs.parse(args).accountId,
      list: emailsFor(state, args)
        .filter((email) => ids.includes(email.id))
        .map(emailObject),
      notFound: [],
      state: emailStateOf(state),
    };
  },
  "Email/query": (state, args) => {
    if (refusedFilter(args.filter, state.refuseSearches)) {
      throw new FakeMethodError(
        "unsupportedFilter",
        "Filter too complex or not supported."
      );
    }
    const { limit, position } = QueryArgs.parse(args);
    const ids = emailsFor(state, args)
      .filter((email) => matchesFilter(email, args.filter))
      .map((email) => email.id);
    return {
      accountId: AccountArgs.parse(args).accountId,
      canCalculateChanges: false,
      ids: ids.slice(position, position + limit),
      position,
      queryState: "1",
      total: ids.length,
    };
  },
  "Email/set": (state, args) => {
    const { create, destroy, update } = SetArgs.parse(args);
    state.calendar.emails.push(...Object.values(create));
    state.updates.push(update);
    for (const [id, patch] of Object.entries(update)) {
      const email = state.emails.find((item) => item.id === id);
      if (email !== undefined) {
        applyPatch(email, patch);
      }
    }
    // Created emails (drafts) are only recorded, not listed.
    const created = Object.entries(create).map(([key, email]) => {
      state.created.push(email);
      const id = `d${state.created.length}`;
      return [key, { blobId: `b-${id}`, id, size: 1, threadId: `t-${id}` }];
    });
    return {
      accountId: ACCOUNT,
      newState: "2",
      oldState: "1",
      ...(created.length > 0 ? { created: Object.fromEntries(created) } : {}),
      ...(destroy.length > 0 ? { destroyed: destroy } : {}),
      updated: Object.fromEntries(Object.keys(update).map((id) => [id, null])),
    };
  },
  "EmailSubmission/get": (state, args) => {
    const ids = args.ids === null ? null : Ids.parse(args.ids);
    return {
      accountId: ACCOUNT,
      list: state.submissions.filter(
        (submission) => ids === null || ids.includes(submission.id)
      ),
      notFound: [],
      state: "1",
    };
  },
  "EmailSubmission/query": (state, args) => {
    const filter = z
      .object({
        emailIds: z.array(z.string()).optional(),
        undoStatus: z.string().optional(),
      })
      .catch({})
      .parse(args.filter);
    const ids = state.submissions
      .filter(
        (submission) =>
          (filter.emailIds === undefined ||
            filter.emailIds.includes(submission.emailId)) &&
          (filter.undoStatus === undefined ||
            submission.undoStatus === filter.undoStatus)
      )
      .toSorted((a, b) => a.sendAt.localeCompare(b.sendAt))
      .map((submission) => submission.id);
    return {
      accountId: ACCOUNT,
      canCalculateChanges: false,
      ids,
      position: 0,
      queryState: "1",
      total: ids.length,
    };
  },
  "Identity/changes": (_state, args) => noChanges(args),
  // Only the properties asked for, like Stalwart (RFC 8620 §5.1).
  "Identity/get": (state, args) => {
    const { properties } = z
      .object({ properties: z.array(z.string()).nullish() })
      .parse(args);
    return {
      accountId: ACCOUNT,
      list: state.identities.map((identity) =>
        properties === null || properties === undefined
          ? identity
          : Object.fromEntries(
              Object.entries(identity).filter(
                ([key]) => key === "id" || properties.includes(key)
              )
            )
      ),
      notFound: [],
      state: "1",
    };
  },
  "Mailbox/changes": (_state, args) => noChanges(args),
  // Only by role (the service worker looks for the inbox).
  "Mailbox/query": (_state, args) => {
    const role = z.object({ role: z.string() }).safeParse(args.filter);
    return {
      accountId: ACCOUNT,
      canCalculateChanges: false,
      ids: MAILBOXES.filter(
        (box) => !role.success || box.role === role.data.role
      ).map((box) => box.id),
      position: 0,
      queryState: "1",
    };
  },
  "Mailbox/get": (state, args) => ({
    accountId: AccountArgs.parse(args).accountId,
    list: isShared(args)
      ? SHARED_MAILBOXES.map((box) =>
          mailboxObject(box, state.sharedEmails, true)
        )
      : MAILBOXES.map((box) => mailboxObject(box, state.emails)),
    notFound: [],
    state: "1",
  }),
  "Mailbox/set": (state, args) => {
    const { update } = SetArgs.parse(args);
    state.mailboxSets.push(args);
    return {
      accountId: ACCOUNT,
      newState: "2",
      oldState: "1",
      updated: Object.fromEntries(Object.keys(update).map((id) => [id, null])),
    };
  },
  "Principal/get": (_state, args) => {
    const ids = Ids.parse(args.ids);
    return {
      accountId: ACCOUNT,
      list: [...PRINCIPALS, ...SEARCH_ONLY].filter((principal) =>
        ids.includes(principal.id)
      ),
      notFound: [],
      state: "1",
    };
  },
  "Principal/query": (_state, args) => {
    const ids = principalIds(args);
    return {
      accountId: ACCOUNT,
      canCalculateChanges: false,
      ids,
      position: 0,
      queryState: "1",
      total: ids.length,
    };
  },
  "Quota/get": () => ({
    accountId: ACCOUNT,
    // 90 % used: the sidebar warns.
    list: [
      {
        hardLimit: 1_000_000_000,
        id: "q",
        name: "jane@example.com",
        resourceType: "octets",
        scope: "account",
        types: ["Email"],
        used: 900_000_000,
      },
    ],
    notFound: [],
    state: "1",
  }),
  "Thread/changes": (_state, args) => noChanges(args),
  "Thread/get": (_state, args) => ({
    accountId: ACCOUNT,
    list: Ids.parse(args.ids).map((id) => ({ emailIds: [id.slice(2)], id })),
    notFound: [],
    state: "1",
  }),
};

/** A full email (the reader's `Email/get` with bodies). */
const isBodyRequest = (name: string, args: Json): boolean =>
  name === "Email/get" && BodyRequest.safeParse(args).success;

const isFailingBody = (state: FakeJmap, name: string, args: Json): boolean =>
  isBodyRequest(name, args) &&
  Ids.parse(args.ids).some((id) => state.failBodies.has(id));
// Like Stalwart: one active script; the vacation response is a script
// named "vacation" and turning it on deactivates the others.
const VacationPatch = z.object({
  fromDate: z.string().nullable().optional(),
  htmlBody: z.string().nullable().optional(),
  isEnabled: z.boolean().optional(),
  subject: z.string().nullable().optional(),
  textBody: z.string().nullable().optional(),
  toDate: z.string().nullable().optional(),
});
const ScriptSetArgs = z.object({
  create: z
    .record(z.string(), z.object({ blobId: z.string(), name: z.string() }))
    .optional(),
  destroy: z.array(z.string()).optional(),
  onSuccessActivateScript: z.string().nullable().optional(),
  onSuccessDeactivateScript: z.boolean().optional(),
  update: z
    .record(
      z.string(),
      z.object({ blobId: z.string().optional(), name: z.string().optional() })
    )
    .optional(),
});

const activate = (state: FakeJmap, id: string | null): void => {
  for (const script of state.scripts) {
    script.isActive = script.id === id;
  }
  state.vacation.isEnabled = state.scripts.some(
    (script) => script.name === "vacation" && script.isActive
  );
};

/** A stand-in for the server's check: "!!" is a syntax error. */
const validate = (content: string | undefined): Json | null => {
  if (content === undefined) {
    return { description: "Blob not found.", type: "invalidScript" };
  }
  const line = content.split("\n").findIndex((text) => text.includes("!!"));
  return line === -1
    ? null
    : {
        description: `Unexpected token at line ${String(line + 1)}, column 1.`,
        type: "invalidScript",
      };
};

const FileFilter = z.object({
  isTopLevel: z.boolean().optional(),
  name: z.string().optional(),
  parentId: z.string().optional(),
});
// Stalwart 1.0 (file/set.rs, file/node.rs): `@type` is required and an
// unknown key (the old `type`) is refused with `invalidProperties`.
const FileCreate = z.strictObject({
  "@type": z.enum(["Directory", "File"]),
  blobId: z.string().optional(),
  mediaType: z.string().optional(),
  name: z.string(),
  parentId: z.string().nullable(),
});

/** FileNode (draft-ietf-jmap-filenode), as checked against Stalwart. */
const FILE_METHODS: Readonly<Record<string, Method>> = {
  "FileNode/get": (state, args) => {
    const ids = Ids.parse(args.ids);
    return {
      accountId: ACCOUNT,
      list: state.files.filter((file) => ids.includes(file.id)),
      notFound: ids.filter((id) => !state.files.some((file) => file.id === id)),
      state: "1",
    };
  },
  "FileNode/query": (state, args) => {
    const filter = FileFilter.parse(args.filter ?? {});
    return {
      accountId: ACCOUNT,
      canCalculateChanges: false,
      ids: state.files
        .filter(
          (file) =>
            (filter.isTopLevel !== true || file.parentId === null) &&
            (filter.name === undefined || file.name === filter.name) &&
            (filter.parentId === undefined || file.parentId === filter.parentId)
        )
        .map((file) => file.id),
      position: 0,
      queryState: "1",
    };
  },
  "FileNode/set": (state, args) => {
    const created: Record<string, Json> = {};
    const notCreated: Record<string, Json> = {};
    for (const [key, raw] of Object.entries(Args.parse(args.create ?? {}))) {
      const parsed = FileCreate.safeParse(raw);
      if (!parsed.success) {
        notCreated[key] = {
          description: parsed.error.message,
          type: "invalidProperties",
        };
        continue;
      }
      const create = parsed.data;
      const id = `f${String(state.files.length + 1)}`;
      // `#dir`: a folder created earlier in the same request.
      const parentId =
        create.parentId?.startsWith("#") === true
          ? (state.files.findLast((file) => file["@type"] === "Directory")
              ?.id ?? null)
          : create.parentId;
      state.files.push({
        "@type": create["@type"],
        blobId: create.blobId ?? null,
        id,
        mediaType: create.mediaType ?? null,
        name: create.name,
        parentId,
      });
      created[key] = { id };
    }
    const updated: Record<string, null> = {};
    for (const [id, raw] of Object.entries(Args.parse(args.update ?? {}))) {
      const file = state.files.find((item) => item.id === id);
      const blobId = z.string().safeParse(Args.parse(raw).blobId);
      if (file !== undefined && blobId.success) {
        file.blobId = blobId.data;
        updated[id] = null;
      }
    }
    return {
      accountId: ACCOUNT,
      created,
      newState: "2",
      notCreated,
      oldState: "1",
      updated,
    };
  },
};

/** The JSON of the synced preferences file, if there is one. */
export const syncedPreferences = (state: FakeJmap): Json | null => {
  const file = state.files.find((item) => item.name === "preferences.json");
  const text =
    file === undefined || file.blobId === null
      ? undefined
      : state.blobs.get(file.blobId);
  return text === undefined ? null : Args.parse(JSON.parse(text));
};

/** Puts a synced preferences file on the fake server (another device). */
export const putSyncedPreferences = (state: FakeJmap, doc: Json): void => {
  const blobId = `blob-${String(state.blobs.size + 1)}`;
  state.blobs.set(blobId, JSON.stringify(doc));
  state.files.push(
    {
      "@type": "Directory",
      blobId: null,
      id: "fdir",
      mediaType: null,
      name: ".nebula-mail",
      parentId: null,
    },
    {
      "@type": "File",
      blobId,
      id: "fprefs",
      mediaType: "application/json",
      name: "preferences.json",
      parentId: "fdir",
    }
  );
};

/** Like Stalwart: the active script can't be destroyed. */
const destroyScripts = (state: FakeJmap, ids: readonly string[]) => {
  const destroyed: string[] = [];
  const notDestroyed: Record<string, Json> = {};
  for (const id of ids) {
    const index = state.scripts.findIndex((item) => item.id === id);
    if (index === -1) {
      notDestroyed[id] = { type: "notFound" };
    } else if (state.scripts[index]?.isActive === true) {
      notDestroyed[id] = { type: "scriptIsActive" };
    } else {
      state.scripts.splice(index, 1);
      destroyed.push(id);
    }
  }
  return { destroyed, notDestroyed };
};

const SIEVE_METHODS: Readonly<Record<string, Method>> = {
  "SieveScript/get": (state) => ({
    accountId: ACCOUNT,
    list: state.scripts.map((script) => ({ ...script })),
    notFound: [],
    state: "1",
  }),
  "SieveScript/set": (state, args) => {
    const parsed = ScriptSetArgs.parse(args);
    const created: Record<string, Json> = {};
    const notCreated: Record<string, Json> = {};
    const updated: Record<string, null> = {};
    const notUpdated: Record<string, Json> = {};
    for (const [key, create] of Object.entries(parsed.create ?? {})) {
      const error = validate(state.blobs.get(create.blobId));
      if (error === null) {
        const id = `s${String(state.scripts.length + 1)}`;
        state.scripts.push({
          blobId: create.blobId,
          id,
          isActive: false,
          name: create.name,
        });
        created[key] = { blobId: create.blobId, id, isActive: false };
      } else {
        notCreated[key] = error;
      }
    }
    for (const [id, patch] of Object.entries(parsed.update ?? {})) {
      const script = state.scripts.find((item) => item.id === id);
      const error =
        patch.blobId === undefined
          ? null
          : validate(state.blobs.get(patch.blobId));
      if (script === undefined || error !== null) {
        notUpdated[id] = error ?? { type: "notFound" };
      } else {
        script.blobId = patch.blobId ?? script.blobId;
        script.name = patch.name ?? script.name;
        updated[id] = null;
      }
    }
    const { destroyed, notDestroyed } = destroyScripts(
      state,
      parsed.destroy ?? []
    );
    const target = parsed.onSuccessActivateScript;
    if (target !== undefined && target !== null) {
      const id = target.startsWith("#")
        ? z.string().catch("").parse(created[target.slice(1)]?.id)
        : target;
      activate(state, id);
    } else if (parsed.onSuccessDeactivateScript === true) {
      activate(state, null);
    }
    return {
      accountId: ACCOUNT,
      created,
      destroyed,
      newState: "2",
      notCreated,
      notDestroyed,
      notUpdated,
      oldState: "1",
      updated,
    };
  },
  "SieveScript/validate": (state, args) => ({
    accountId: ACCOUNT,
    error: validate(state.blobs.get(String(args.blobId))),
  }),
  "VacationResponse/get": (state) => ({
    accountId: ACCOUNT,
    list: [{ id: "singleton", ...state.vacation }],
    notFound: [],
    state: "1",
  }),
  "VacationResponse/set": (state, args) => {
    const update = z.object({ singleton: VacationPatch }).parse(args.update);
    state.vacationUpdates.push(update.singleton);
    const { isEnabled, ...rest } = update.singleton;
    for (const [key, value] of Object.entries(rest)) {
      if (value !== undefined) {
        state.vacation = { ...state.vacation, [key]: value };
      }
    }
    if (isEnabled !== undefined) {
      let script = state.scripts.find((item) => item.name === "vacation");
      if (script === undefined) {
        addScript(state, "vacation", "# vacation", false);
        script = state.scripts.at(-1);
      }
      if (isEnabled) {
        activate(state, script?.id ?? null);
      } else if (script?.isActive === true) {
        activate(state, null);
      }
    }
    return {
      accountId: ACCOUNT,
      newState: "2",
      oldState: "1",
      updated: { singleton: null },
    };
  },
};

const MdnSendArgs = z.object({
  onSuccessUpdateEmail: z.record(z.string(), Args).catch({}),
  send: z.record(z.string(), z.object({ forEmailId: z.string() }).loose()),
});

/**
 * RFC 9007 `MDN/send`: records it and applies `onSuccessUpdateEmail`
 * (usually `$mdnsent`); `$mdnsent` already set → `mdnAlreadySent`. The
 * second value is the implicit `Email/set` response.
 */
const mdnSend = (state: FakeJmap, args: Json): [Json, Json] => {
  state.mdnSends.push(args);
  const parsed = MdnSendArgs.parse(args);
  const sent: Record<string, Json> = {};
  const notSent: Record<string, Json> = {};
  const updated: Record<string, null> = {};
  for (const [key, mdn] of Object.entries(parsed.send)) {
    const email = state.emails.find((item) => item.id === mdn.forEmailId);
    if (email === undefined) {
      notSent[key] = { type: "notFound" };
    } else if (email.keywords.$mdnsent === true) {
      notSent[key] = { type: "mdnAlreadySent" };
    } else {
      sent[key] = { finalRecipient: "rfc822;jane@example.com" };
      const patch = parsed.onSuccessUpdateEmail[`#${key}`];
      if (patch !== undefined) {
        applyPatch(email, patch);
        updated[email.id] = null;
      }
    }
  }
  return [
    { accountId: ACCOUNT, notSent, sent },
    { accountId: ACCOUNT, newState: "2", oldState: "1", updated },
  ];
};

const SubmitArgs = z.object({
  create: z
    .record(
      z.string(),
      z.object({
        emailId: z.string(),
        envelope: z
          .object({
            mailFrom: z.object({
              email: z.string(),
              parameters: z.record(z.string(), z.string()).nullish(),
            }),
            rcptTo: z.array(z.object({ email: z.string() })),
          })
          .nullish(),
        identityId: z.string().optional(),
        "https://helixnebula.tech/jmap/bridge/sendWhenSignedOut": z
          .boolean()
          .optional(),
      })
    )
    .catch({}),
  onSuccessUpdateEmail: z
    .record(z.string(), z.record(z.string(), z.unknown()))
    .optional(),
  update: z
    .record(
      z.string(),
      z.object({
        undoStatus: z.string().optional(),
        "https:~1~1helixnebula.tech~1jmap~1bridge~1sendWhenSignedOut": z
          .boolean()
          .optional(),
      })
    )
    .catch({}),
});
type SubmitCreate = z.infer<typeof SubmitArgs>["create"][string];

/**
 * When the server releases it (RFC 4865, Stalwart's answers): HOLDUNTIL
 * or HOLDFOR keep it `pending`; refused like Stalwart 1.0.1 when past or
 * after `state.releaseMax`.
 */
const holdOf = (
  state: FakeJmap,
  create: SubmitCreate
):
  | { readonly sendAt: string; readonly pending: boolean }
  | { readonly refused: string } => {
  const parameters = create.envelope?.mailFrom.parameters ?? {};
  const now = Date.now();
  const until = parameters.HOLDUNTIL;
  const holdFor = parameters.HOLDFOR;
  if (until !== undefined) {
    const at = Date.parse(until);
    if (Number.isNaN(at) || at <= now) {
      return { refused: "HOLDUNTIL must be a date and time in the future." };
    }
    if (state.releaseMax !== null && at > Date.parse(state.releaseMax)) {
      return {
        refused: `Requested release time exceeds maximum of ${state.releaseMax}.`,
      };
    }
    return { pending: true, sendAt: until };
  }
  if (holdFor !== undefined) {
    return {
      pending: true,
      sendAt: new Date(now + Number(holdFor) * 1000).toISOString(),
    };
  }
  return { pending: false, sendAt: new Date(now).toISOString() };
};

/** A held draft becomes a listed email (Sent), so the app can find it. */
const listHeldEmail = (
  state: FakeJmap,
  emailId: string,
  patch: Json | undefined
): void => {
  const existing = state.emails.find((email) => email.id === emailId);
  if (existing !== undefined) {
    if (patch !== undefined) {
      applyPatch(existing, patch);
    }
    return;
  }
  const created = z
    .object({
      bodyValues: z
        .record(z.string(), z.object({ value: z.string() }))
        .catch({}),
      from: z.array(
        z.object({ email: z.string(), name: z.string().nullish() })
      ),
      subject: z.string().catch(""),
    })
    .safeParse(state.created[Number(emailId.slice(1)) - 1]);
  if (!created.success) {
    return;
  }
  const from = created.data.from[0] ?? { email: "", name: null };
  const email: FakeEmail = {
    from: { email: from.email, name: from.name ?? "" },
    html:
      created.data.bodyValues.html?.value ??
      created.data.bodyValues.text?.value ??
      "",
    id: emailId,
    keywords: { $draft: true, $seen: true },
    mailboxIds: { drafts: true },
    receivedAt: new Date().toISOString(),
    subject: created.data.subject,
  };
  if (patch !== undefined) {
    applyPatch(email, patch);
  }
  state.emails.push(email);
};

const submissionRow = (
  state: FakeJmap,
  create: SubmitCreate,
  emailId: string,
  hold: { readonly sendAt: string; readonly pending: boolean }
): FakeSubmission => ({
  deliveryStatus: null,
  emailId,
  envelope:
    create.envelope === undefined || create.envelope === null
      ? null
      : {
          mailFrom: {
            email: create.envelope.mailFrom.email,
            parameters: create.envelope.mailFrom.parameters ?? null,
          },
          rcptTo: create.envelope.rcptTo.map((rcpt) => ({
            email: rcpt.email,
            parameters: null,
          })),
        },
  id: `sub-${state.submissions.length + 1}`,
  identityId: create.identityId ?? IDENTITY.id,
  sendAt: hold.sendAt,
  undoStatus: hold.pending ? "pending" : "final",
  ...(state.sendWhenSignedOut
    ? {
        "https://helixnebula.tech/jmap/bridge/sendWhenSignedOut":
          hold.pending &&
          create["https://helixnebula.tech/jmap/bridge/sendWhenSignedOut"] ===
            true,
      }
    : {}),
});

/** An EmailSubmission/set that cancels, or creates a held submission. */
const isHoldOrCancel = (args: Json): boolean => {
  const { create, update } = args as {
    readonly create?: Record<string, unknown>;
    readonly update?: Record<string, unknown>;
  };
  if (update !== undefined && Object.keys(update).length > 0) {
    return true;
  }
  return Object.values(create ?? {}).some((value) => {
    const text = JSON.stringify(value);
    return text.includes('"HOLDUNTIL"') || text.includes('"HOLDFOR"');
  });
};

/**
 * EmailSubmission/set without `onSuccessUpdateEmail`: cancelling a held
 * one (undo, «Cancelar envío») and submitting an email again («Cambiar
 * hora»).
 */
const submissionSet = (state: FakeJmap, args: Json): Json => {
  state.mailSends.push(args);
  const parsed = SubmitArgs.parse(args);
  const updated: Record<string, null> = {};
  const notUpdated: Record<string, Json> = {};
  for (const [id, patch] of Object.entries(parsed.update)) {
    const submission = state.submissions.find((item) => item.id === id);
    const flag =
      patch["https:~1~1helixnebula.tech~1jmap~1bridge~1sendWhenSignedOut"];
    if (
      submission?.undoStatus === "pending" &&
      patch.undoStatus === "canceled"
    ) {
      submission.undoStatus = "canceled";
      updated[id] = null;
    } else if (
      submission?.undoStatus === "pending" &&
      flag !== undefined &&
      state.sendWhenSignedOut
    ) {
      // Like the bridge: only while pending.
      submission["https://helixnebula.tech/jmap/bridge/sendWhenSignedOut"] =
        flag;
      updated[id] = null;
    } else {
      notUpdated[id] = { type: "cannotUnsend" };
    }
  }
  const created: Json = {};
  const notCreated: Json = {};
  for (const [key, create] of Object.entries(parsed.create)) {
    const hold = holdOf(state, create);
    if ("refused" in hold) {
      notCreated[key] = {
        description: `Server rejected MAIL-FROM: 501 5.5.4 ${hold.refused}`,
        type: "forbiddenMailFrom",
      };
      continue;
    }
    const row = submissionRow(state, create, create.emailId, hold);
    state.submissions.push(row);
    created[key] = {
      id: row.id,
      sendAt: row.sendAt,
      undoStatus: row.undoStatus,
    };
  }
  return {
    accountId: ACCOUNT,
    created,
    newState: "2",
    notCreated,
    notUpdated,
    oldState: "1",
    updated,
  };
};

/** The id behind `#creationId`, from an earlier Email/set's `created`. */
const resolveCreated = (
  id: string,
  earlier: readonly [string, Json, string][]
): string => {
  if (!id.startsWith("#")) {
    return id;
  }
  const key = id.slice(1);
  for (const [name, response] of earlier) {
    const created = z
      .object({ created: z.record(z.string(), z.object({ id: z.string() })) })
      .safeParse(response);
    if (name === "Email/set" && created.success) {
      const found = created.data.created[key];
      if (found !== undefined) {
        return found.id;
      }
    }
  }
  return id;
};

/** A mail send, ending as `state.submitOutcome` says (with the implicit call). */
const mailSubmit = (
  state: FakeJmap,
  args: Json,
  callId: string,
  earlier: readonly [string, Json, string][]
): [string, Json, string][] => {
  state.mailSends.push(args);
  const created: Json = {};
  const notCreated: Json = {};
  const emailIds: string[] = [];
  const parsed = SubmitArgs.parse(args);
  for (const [key, submission] of Object.entries(parsed.create)) {
    const emailId = resolveCreated(submission.emailId, earlier);
    const hold = holdOf(state, submission);
    if ("refused" in hold) {
      // Refused before anything is queued: no row (Stalwart).
      notCreated[key] = {
        description: `Server rejected MAIL-FROM: 501 5.5.4 ${hold.refused}`,
        type: "forbiddenMailFrom",
      };
      continue;
    }
    const row = submissionRow(state, submission, emailId, hold);
    const { id, sendAt } = row;
    // Like the bridge: the row stays (empty SMTP reply) when unknown.
    state.submissions.push(row);
    if (state.submitOutcome === "unknown") {
      notCreated[key] = {
        description:
          "The message may have been sent; check before sending again.",
        type: "forbiddenToSend",
      };
    } else {
      created[key] = { id, sendAt, undoStatus: row.undoStatus };
      emailIds.push(emailId);
      if (hold.pending && state.submitOutcome === "ok") {
        listHeldEmail(state, emailId, parsed.onSuccessUpdateEmail?.[`#${key}`]);
      }
    }
  }
  const base = { accountId: ACCOUNT, newState: "2", oldState: "1" };
  const responses: [string, Json, string][] = [
    ["EmailSubmission/set", { ...base, created, notCreated }, callId],
  ];
  if (emailIds.length > 0) {
    responses.push([
      "Email/set",
      state.submitOutcome === "notFiled"
        ? {
            ...base,
            notUpdated: Object.fromEntries(
              emailIds.map((id) => [id, { type: "serverFail" }])
            ),
          }
        : {
            ...base,
            updated: Object.fromEntries(emailIds.map((id) => [id, null])),
          },
      callId,
    ]);
  }
  return responses;
};

const ALL_METHODS: Readonly<Record<string, Method>> = {
  ...METHODS,
  ...SIEVE_METHODS,
  ...FILE_METHODS,
};

/**
 * The mail side of EmailSubmission/set: a send (`onSuccessUpdateEmail`), a
 * cancel or a «Cambiar hora» (held with HOLDUNTIL/HOLDFOR). Null for the
 * rest: the calendar's own iTIP mail (fake-calendar.ts records it).
 */
const mailSubmission = (
  state: FakeJmap,
  args: Json,
  callId: string,
  responses: readonly [string, Json, string][]
): [string, Json, string][] | null => {
  if (args.onSuccessUpdateEmail !== undefined) {
    return mailSubmit(state, args, callId, responses);
  }
  return isHoldOrCancel(args)
    ? [["EmailSubmission/set", submissionSet(state, args), callId]]
    : null;
};

export const handleApi = (state: FakeJmap, body: unknown): Json => {
  const responses: [string, Json, string][] = [];
  for (const [name, rawArgs, callId] of Body.parse(body).methodCalls) {
    const args: Json = {};
    for (const [key, value] of Object.entries(rawArgs)) {
      if (key.startsWith("#")) {
        const ref = Reference.parse(value);
        const earlier = responses.find(
          (response) => response[2] === ref.resultOf
        );
        args[key.slice(1)] = evaluatePath(earlier?.[1], ref.path);
      } else {
        args[key] = value;
      }
    }
    state.accountsUsed.push(AccountArgs.parse(args).accountId);
    if (state.directoryClosed && name === "Principal/query") {
      responses.push([
        "error",
        {
          description: "The administrator has disabled directory queries.",
          type: "forbidden",
        },
        callId,
      ]);
      continue;
    }
    const mail =
      name === "EmailSubmission/set"
        ? mailSubmission(state, args, callId, responses)
        : null;
    if (mail !== null) {
      responses.push(...mail);
      continue;
    }
    const method = ALL_METHODS[name];
    // The calendar fake answers Principal/query with nobody (free/busy);
    // `directory` lists the people instead (sharing tests).
    const calendarMethod =
      state.directory && name === "Principal/query"
        ? undefined
        : CALENDAR_METHODS[name];
    if (calendarMethod !== undefined) {
      try {
        responses.push([name, calendarMethod(state.calendar, args), callId]);
      } catch (error) {
        if (!(error instanceof FakeMethodError)) {
          throw error;
        }
        responses.push([
          "error",
          { description: error.message, type: error.type },
          callId,
        ]);
      }
    } else if (name === "MDN/send") {
      const [sent, emailSet] = mdnSend(state, args);
      responses.push([name, sent, callId], ["Email/set", emailSet, callId]);
    } else if (method === undefined) {
      responses.push(["error", { type: "unknownMethod" }, callId]);
    } else if (isFailingBody(state, name, args)) {
      responses.push(["error", { type: "serverFail" }, callId]);
    } else {
      try {
        responses.push([name, method(state, args), callId]);
      } catch (error) {
        if (!(error instanceof FakeMethodError)) {
          throw error;
        }
        responses.push([
          "error",
          { description: error.message, type: error.type },
          callId,
        ]);
      }
    }
  }
  return { methodResponses: responses, sessionState: "s1" };
};

const asksForBodies = (body: unknown): boolean =>
  Body.safeParse(body).data?.methodCalls.some(([name, args]) =>
    isBodyRequest(name, args)
  ) ?? false;

/** An email's inline image (also copied to our account) or a card photo. */
const isImageBlob = (
  state: FakeJmap,
  pathname: string,
  blobId: string
): boolean =>
  blobId === CARD_PHOTO_BLOB ||
  [...state.emails, ...state.sharedEmails].some(
    (email) =>
      pathname.includes(`/b-${email.id}-img/`) ||
      pathname.includes(`/copy-b-${email.id}-img/`)
  );

const CORS = {
  "access-control-allow-headers": "authorization, content-type",
  "access-control-allow-methods": "GET, POST, OPTIONS",
  "access-control-allow-origin": "*",
};

/** Routes every request to the fake JMAP origin through the fake server. */
/** Each stream only gets what it asked for: the app keeps two open. */
const takePushChanges = (state: FakeJmap, types: string | null): string[] => {
  const asked = (types ?? "").split(",");
  const changed = state.pushChanges.filter(
    (type) => asked.includes(type) || asked.includes("*")
  );
  state.pushChanges = state.pushChanges.filter(
    (type) => !changed.includes(type)
  );
  return changed;
};

/** The session document for the current state (`/.well-known/jmap`). */
const sessionOf = (state: FakeJmap) => {
  const base = session(
    state.fileNode,
    state.mdn,
    state.emailPush,
    state.futureRelease
  );
  const own = base.accounts[ACCOUNT];
  // Nextcloud through the bridge: what its server offers.
  const accounts =
    state.calendar.nextcloud === true
      ? {
          ...base.accounts,
          [ACCOUNT]: {
            ...own,
            accountCapabilities: {
              ...own.accountCapabilities,
              "https://helixnebula.tech/jmap/bridge": {
                calendarPublishing: true,
                calendarSubscriptions: true,
                calendarTrash: true,
              },
            },
          },
        }
      : base.accounts;
  // The bridge's capability: with scheduled mail after sign-out, or
  // plain for the calendar extras.
  let bridge: Json | null = null;
  if (state.sendWhenSignedOut) {
    bridge = { sendWhenSignedOut: true };
  } else if (state.calendar.bridge === true) {
    bridge = {};
  }
  return bridge === null
    ? base
    : {
        ...base,
        accounts,
        capabilities: {
          ...base.capabilities,
          "https://helixnebula.tech/jmap/bridge": bridge,
        },
      };
};

/**
 * The event source: no push in the tests but calendar alerts and the
 * changes a test asked for, in a short stream the client reconnects to;
 * a 503 with `pushDown`.
 */
const eventSource = (
  state: FakeJmap,
  url: URL
): Parameters<Route["fulfill"]>[0] => {
  if (state.pushDown) {
    return { headers: CORS, status: 503 };
  }
  const changed = takePushChanges(state, url.searchParams.get("types"));
  const stateEvent =
    changed.length === 0
      ? ""
      : `event: state\ndata: ${JSON.stringify({
          "@type": "StateChange",
          changed: {
            [ACCOUNT]: Object.fromEntries(
              changed.map((type) => [type, `${type}-${Date.now()}`])
            ),
          },
        })}\n\n`;
  return {
    body:
      stateEvent +
      eventSourceBody(state.calendar, url.searchParams.get("types") ?? ""),
    contentType: "text/event-stream",
    headers: CORS,
  };
};

export const installFakeJmap = async (
  page: Page,
  options: {
    /** Also for the service worker (its requests go by the context). */
    readonly serviceWorker?: boolean;
  } = {}
): Promise<FakeJmap> => {
  const state: FakeJmap = {
    accountsUsed: [],
    arrivals: [],
    blobCopies: [],
    blobs: new Map(),
    bookSets: [],
    bookShares: {},
    bodiesGate: null,
    uploadsGate: null,
    uploadsFail: false,
    directory: false,
    directoryClosed: false,
    calendar: fakeCalendar(),
    cardSets: [],
    cards: cards(),
    created: [],
    down: false,
    pushDown: false,
    emails: emails(),
    failBodies: new Set(),
    emailPush: false,
    fileNode: false,
    files: [],
    mailboxSets: [],
    mdn: false,
    futureRelease: false,
    sendWhenSignedOut: false,
    releaseMax: null,
    mdnSends: [],
    pushChanges: [],
    refuseSearches: false,
    submitOutcome: "ok",
    mailSends: [],
    identities: [IDENTITY],
    scripts: [],
    sharedCards: sharedCards(),
    sharedEmails: sharedEmails(),
    submissions: [],
    updates: [],
    vacation: {
      fromDate: null,
      htmlBody: null,
      isEnabled: false,
      subject: null,
      textBody: null,
      toDate: null,
    },
    vacationUpdates: [],
  };
  const handle = async (route: Route): Promise<void> => {
    const request = route.request();
    const url = new URL(request.url());
    if (state.down) {
      await route.abort("internetdisconnected");
    } else if (request.method() === "OPTIONS") {
      await route.fulfill({ headers: CORS, status: 204 });
    } else if (url.pathname === "/.well-known/jmap") {
      await route.fulfill({ headers: CORS, json: sessionOf(state) });
    } else if (url.pathname === "/jmap/" && request.method() === "POST") {
      const body: unknown = JSON.parse(request.postData() ?? "null");
      if (state.bodiesGate !== null && asksForBodies(body)) {
        await state.bodiesGate;
      }
      await route.fulfill({ headers: CORS, json: handleApi(state, body) });
    } else if (url.pathname.startsWith("/jmap/upload/")) {
      if (state.uploadsGate !== null) {
        await state.uploadsGate;
      }
      if (state.uploadsFail) {
        await route.fulfill({ headers: CORS, status: 500 });
        return;
      }
      const blobId = `blob-${String(state.blobs.size + 1)}`;
      state.blobs.set(blobId, request.postData() ?? "");
      await route.fulfill({
        headers: CORS,
        json: {
          accountId: ACCOUNT,
          blobId,
          size: 0,
          type: "application/sieve",
        },
      });
    } else if (url.pathname.startsWith("/jmap/download/")) {
      // Blobs are uploaded Sieve scripts, the emails' inline images or a
      // contact's photo.
      const blobId = url.pathname.split("/")[4] ?? "";
      const content = state.blobs.get(blobId);
      const image = isImageBlob(state, url.pathname, blobId);
      if (content !== undefined) {
        await route.fulfill({
          body: content,
          contentType: "application/sieve",
          headers: CORS,
        });
      } else if (image) {
        await route.fulfill({
          body: PIXEL_PNG,
          contentType: "image/png",
          headers: CORS,
        });
      } else {
        await route.fulfill({ headers: CORS, status: 404 });
      }
    } else if (url.pathname.startsWith("/jmap/eventsource")) {
      await route.fulfill(eventSource(state, url));
    } else {
      await route.fulfill({ headers: CORS, status: 404 });
    }
  };
  await page.route(`${JMAP_ORIGIN}/**`, handle);
  if (options.serviceWorker === true) {
    await page.context().route(`${JMAP_ORIGIN}/**`, handle);
  }
  return state;
};
