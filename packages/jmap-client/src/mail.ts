// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// RFC 8621 data types. Object schemas are loose (unknown vendor properties
// are kept) and the TypeScript types are inferred from them, so the type and
// the runtime check can't drift apart.

import { z } from "zod";

import {
  BlobId,
  EmailId,
  EmailSubmissionId,
  IdentityId,
  MailboxId,
  PrincipalId,
  ThreadId,
  VacationResponseId,
} from "./ids";
import type { AccountId, State } from "./ids";
import type { OtherString } from "./other-string";
import { UnsignedInt } from "./standard";
import type {
  Comparator,
  Filter,
  GetArgs,
  QueryArgs,
  QueryChangesArgs,
  SetArgs,
} from "./standard";

// ---------------------------------------------------------------- shared

export const EmailAddressSchema = z.looseObject({
  email: z.string(),
  name: z.string().nullish(),
});
export type EmailAddress = z.infer<typeof EmailAddressSchema>;

export const EmailHeaderSchema = z.looseObject({
  name: z.string(),
  value: z.string(),
});
export type EmailHeader = z.infer<typeof EmailHeaderSchema>;

const Keywords = z.record(z.string(), z.literal(true));
const Addresses = z.array(EmailAddressSchema).nullable();

// ---------------------------------------------------------------- Mailbox

export const MailboxRightsSchema = z.looseObject({
  mayAddItems: z.boolean(),
  mayCreateChild: z.boolean(),
  mayDelete: z.boolean(),
  mayReadItems: z.boolean(),
  mayRemoveItems: z.boolean(),
  mayRename: z.boolean(),
  maySetKeywords: z.boolean(),
  maySetSeen: z.boolean(),
  maySubmit: z.boolean(),
  /** `urn:ietf:params:jmap:mail:share`: may change `shareWith`. */
  mayShare: z.boolean().optional(),
});
export type MailboxRights = z.infer<typeof MailboxRightsSchema>;

/** RFC 8621 §2 (roles from the IANA "IMAP Mailbox Name Attributes"). */
export type MailboxRole =
  | "all"
  | "archive"
  | "drafts"
  | "flagged"
  | "important"
  | "inbox"
  | "junk"
  | "sent"
  | "subscribed"
  | "trash"
  | OtherString;

/** The bridge's name for a folder on the IMAP server. */
export const IMAP_NAME_PROPERTY =
  "https://helixnebula.tech/jmap/bridge/imapName";

export const MailboxSchema = z.looseObject({
  /** Only on the bridge, only when asked for. */
  [IMAP_NAME_PROPERTY]: z.string().optional(),
  id: MailboxId,
  isSubscribed: z.boolean(),
  myRights: MailboxRightsSchema,
  name: z.string(),
  parentId: MailboxId.nullable(),
  role: z.string().nullable(),
  /**
   * `urn:ietf:params:jmap:mail:share`: who else may use it (principal id →
   * rights). Null for mailboxes of other accounts (Stalwart) and left out
   * by servers without sharing.
   */
  shareWith: z.partialRecord(PrincipalId, MailboxRightsSchema).nullish(),
  sortOrder: UnsignedInt,
  totalEmails: UnsignedInt,
  totalThreads: UnsignedInt,
  unreadEmails: UnsignedInt,
  unreadThreads: UnsignedInt,
});
export type Mailbox = z.infer<typeof MailboxSchema>;
export type MailboxProperty = keyof typeof MailboxSchema.shape;

export interface MailboxCreate {
  readonly name: string;
  readonly parentId?: MailboxId | `#${string}` | null;
  readonly role?: MailboxRole | null;
  readonly sortOrder?: number;
  readonly isSubscribed?: boolean;
  readonly shareWith?: Readonly<
    Partial<Record<PrincipalId, MailboxRights>>
  > | null;
}

/** Also `shareWith/<principal id>`: that principal's rights; null removes. */
export type MailboxPatch = Partial<MailboxCreate> &
  Readonly<Record<`shareWith/${string}`, MailboxRights | null>>;

export interface MailboxSetArgs extends SetArgs<
  MailboxId,
  MailboxCreate,
  MailboxPatch
> {
  /** Destroy a non-empty mailbox, removing (or orphan-deleting) its emails. */
  readonly onDestroyRemoveEmails?: boolean;
}

// ---------------------------------------------------------------- Email body

/**
 * RFC 8621 §4.1.4 EmailBodyPart. Which properties are present depends on
 * `bodyProperties`, so all of them are optional here.
 */
export interface EmailBodyPart {
  readonly [vendor: string]: unknown;
  readonly partId?: string | null | undefined;
  readonly blobId?: BlobId | null | undefined;
  readonly size?: number | undefined;
  readonly headers?: EmailHeader[] | undefined;
  readonly name?: string | null | undefined;
  readonly type?: string | undefined;
  readonly charset?: string | null | undefined;
  readonly disposition?: string | null | undefined;
  readonly cid?: string | null | undefined;
  readonly language?: string[] | null | undefined;
  readonly location?: string | null | undefined;
  readonly subParts?: EmailBodyPart[] | null | undefined;
}

export const EmailBodyPartSchema: z.ZodType<EmailBodyPart> = z.lazy(() =>
  z.looseObject({
    blobId: BlobId.nullish(),
    charset: z.string().nullish(),
    cid: z.string().nullish(),
    disposition: z.string().nullish(),
    headers: z.array(EmailHeaderSchema).optional(),
    language: z.array(z.string()).nullish(),
    location: z.string().nullish(),
    name: z.string().nullish(),
    partId: z.string().nullish(),
    size: UnsignedInt.optional(),
    subParts: z.array(EmailBodyPartSchema).nullish(),
    type: z.string().optional(),
  })
);

export type EmailBodyPartProperty =
  | "blobId"
  | "charset"
  | "cid"
  | "disposition"
  | "headers"
  | "language"
  | "location"
  | "name"
  | "partId"
  | "size"
  | "subParts"
  | "type"
  | `header:${string}`;

export const EmailBodyValueSchema = z.looseObject({
  isEncodingProblem: z.boolean().optional(),
  isTruncated: z.boolean().optional(),
  value: z.string(),
});
export type EmailBodyValue = z.infer<typeof EmailBodyValueSchema>;

// ---------------------------------------------------------------- Email

export const EmailSchema = z.looseObject({
  attachments: z.array(EmailBodyPartSchema),
  bcc: Addresses,
  blobId: BlobId,
  bodyStructure: EmailBodyPartSchema,
  bodyValues: z.record(z.string(), EmailBodyValueSchema),
  cc: Addresses,
  from: Addresses,
  hasAttachment: z.boolean(),
  headers: z.array(EmailHeaderSchema),
  htmlBody: z.array(EmailBodyPartSchema),
  id: EmailId,
  inReplyTo: z.array(z.string()).nullable(),
  keywords: Keywords,
  mailboxIds: z.partialRecord(MailboxId, z.literal(true)),
  messageId: z.array(z.string()).nullable(),
  preview: z.string(),
  receivedAt: z.string(),
  references: z.array(z.string()).nullable(),
  replyTo: Addresses,
  sender: Addresses,
  sentAt: z.string().nullable(),
  size: UnsignedInt,
  subject: z.string().nullable(),
  textBody: z.array(EmailBodyPartSchema),
  threadId: ThreadId,
  to: Addresses,
});
export type Email = z.infer<typeof EmailSchema>;
export type EmailProperty = keyof typeof EmailSchema.shape;

/** Dynamic header properties, e.g. `header:List-Unsubscribe:asURLs`. */
export type EmailHeaderProperty = `header:${string}`;

/** Properties returned by `Email/get` when `properties` is omitted. */
export const DEFAULT_EMAIL_PROPERTIES = [
  "id",
  "blobId",
  "threadId",
  "mailboxIds",
  "keywords",
  "size",
  "receivedAt",
  "messageId",
  "inReplyTo",
  "references",
  "sender",
  "from",
  "to",
  "cc",
  "bcc",
  "replyTo",
  "subject",
  "sentAt",
  "hasAttachment",
  "preview",
  "bodyValues",
  "textBody",
  "htmlBody",
  "attachments",
] as const satisfies readonly EmailProperty[];
export type DefaultEmailProperty = (typeof DEFAULT_EMAIL_PROPERTIES)[number];

export interface EmailGetArgs extends GetArgs<
  EmailId,
  EmailProperty | EmailHeaderProperty
> {
  readonly bodyProperties?: readonly EmailBodyPartProperty[];
  readonly fetchTextBodyValues?: boolean;
  readonly fetchHTMLBodyValues?: boolean;
  readonly fetchAllBodyValues?: boolean;
  readonly maxBodyValueBytes?: number;
}

/** RFC 8621 §4.4.1 FilterCondition. Dates are RFC 3339 UTC strings. */
export interface EmailFilterCondition {
  readonly inMailbox?: MailboxId;
  readonly inMailboxOtherThan?: readonly MailboxId[];
  readonly before?: string;
  readonly after?: string;
  readonly minSize?: number;
  readonly maxSize?: number;
  readonly allInThreadHaveKeyword?: string;
  readonly someInThreadHaveKeyword?: string;
  readonly noneInThreadHaveKeyword?: string;
  readonly hasKeyword?: string;
  readonly notKeyword?: string;
  readonly hasAttachment?: boolean;
  readonly text?: string;
  readonly from?: string;
  readonly to?: string;
  readonly cc?: string;
  readonly bcc?: string;
  readonly subject?: string;
  readonly body?: string;
  /** `[name]` (header exists) or `[name, value]` (header contains). */
  readonly header?: readonly [string] | readonly [string, string];
}

export type EmailFilter = Filter<EmailFilterCondition>;

export type EmailSortProperty =
  | "allInThreadHaveKeyword"
  | "from"
  | "hasKeyword"
  | "receivedAt"
  | "sentAt"
  | "size"
  | "someInThreadHaveKeyword"
  | "subject"
  | "to"
  | OtherString;

export interface EmailComparator extends Comparator<EmailSortProperty> {
  /** Required for the `*Keyword` sort properties. */
  readonly keyword?: string;
}

export interface EmailQueryArgs extends QueryArgs<
  EmailFilterCondition,
  EmailComparator
> {
  readonly collapseThreads?: boolean;
}

export interface EmailQueryChangesArgs extends QueryChangesArgs<
  EmailFilterCondition,
  EmailComparator
> {
  readonly collapseThreads?: boolean;
}

/**
 * PatchObject for `Email/set` updates. Prefer the helpers in `patch.ts`
 * (`keywordPatch`, `mailboxPatch`), which escape the path segments.
 */
export interface EmailPatch {
  readonly [path: `keywords/${string}`]: true | null;
  readonly [path: `mailboxIds/${string}`]: true | null;
  readonly keywords?: Readonly<Record<string, true>>;
  readonly mailboxIds?: Readonly<Partial<Record<MailboxId, true>>>;
}

export interface EmailBodyPartCreate {
  readonly partId?: string;
  readonly blobId?: BlobId;
  readonly type?: string;
  readonly charset?: string;
  readonly disposition?: string;
  readonly cid?: string;
  readonly name?: string;
  readonly language?: readonly string[];
  readonly location?: string;
  readonly headers?: readonly EmailHeader[];
  readonly subParts?: readonly EmailBodyPartCreate[];
}

/** RFC 8621 §4.6 Email creation (e.g. a draft). */
export interface EmailCreate {
  readonly [header: `header:${string}`]: unknown;
  readonly mailboxIds: Readonly<
    Partial<Record<MailboxId | `#${string}`, true>>
  >;
  readonly keywords?: Readonly<Record<string, true>>;
  readonly receivedAt?: string;
  readonly messageId?: readonly string[] | null;
  readonly inReplyTo?: readonly string[] | null;
  readonly references?: readonly string[] | null;
  readonly sender?: readonly EmailAddress[] | null;
  readonly from?: readonly EmailAddress[] | null;
  readonly to?: readonly EmailAddress[] | null;
  readonly cc?: readonly EmailAddress[] | null;
  readonly bcc?: readonly EmailAddress[] | null;
  readonly replyTo?: readonly EmailAddress[] | null;
  readonly subject?: string | null;
  readonly sentAt?: string | null;
  readonly bodyStructure?: EmailBodyPartCreate;
  readonly textBody?: readonly EmailBodyPartCreate[];
  readonly htmlBody?: readonly EmailBodyPartCreate[];
  readonly attachments?: readonly EmailBodyPartCreate[];
  readonly bodyValues?: Readonly<
    Record<string, { readonly value: string; readonly isTruncated?: false }>
  >;
}

export type EmailSetArgs = SetArgs<EmailId, EmailCreate, EmailPatch>;

export interface EmailImport {
  readonly blobId: BlobId;
  readonly mailboxIds: Readonly<
    Partial<Record<MailboxId | `#${string}`, true>>
  >;
  readonly keywords?: Readonly<Record<string, true>>;
  readonly receivedAt?: string;
}

export interface EmailImportArgs {
  readonly accountId: AccountId;
  readonly ifInState?: State;
  readonly emails: Readonly<Record<string, EmailImport>>;
}

// ---------------------------------------------------------------- Thread

export const ThreadSchema = z.looseObject({
  emailIds: z.array(EmailId),
  id: ThreadId,
});
export type Thread = z.infer<typeof ThreadSchema>;

// ---------------------------------------------------------------- Identity

export const IdentitySchema = z.looseObject({
  bcc: Addresses,
  email: z.string(),
  htmlSignature: z.string(),
  id: IdentityId,
  mayDelete: z.boolean(),
  name: z.string(),
  replyTo: Addresses,
  textSignature: z.string(),
});
export type Identity = z.infer<typeof IdentitySchema>;
export type IdentityProperty = keyof typeof IdentitySchema.shape;

export interface IdentityCreate {
  readonly email: string;
  readonly name?: string;
  readonly replyTo?: readonly EmailAddress[] | null;
  readonly bcc?: readonly EmailAddress[] | null;
  /** HTML signatures must go through the same sanitizer as emails. */
  readonly htmlSignature?: string;
  readonly textSignature?: string;
}

export type IdentitySetArgs = SetArgs<
  IdentityId,
  IdentityCreate,
  Partial<Omit<IdentityCreate, "email">>
>;

// ---------------------------------------------------------------- EmailSubmission

export const SubmissionAddressSchema = z.looseObject({
  email: z.string(),
  parameters: z.record(z.string(), z.string().nullable()).nullable(),
});
export type SubmissionAddress = z.infer<typeof SubmissionAddressSchema>;

export const EnvelopeSchema = z.looseObject({
  mailFrom: SubmissionAddressSchema,
  rcptTo: z.array(SubmissionAddressSchema),
});
export type Envelope = z.infer<typeof EnvelopeSchema>;

export const DeliveryStatusSchema = z.looseObject({
  delivered: z.string(),
  displayed: z.string(),
  smtpReply: z.string(),
});

export const EmailSubmissionSchema = z.looseObject({
  deliveryStatus: z.record(z.string(), DeliveryStatusSchema).nullable(),
  dsnBlobIds: z.array(BlobId),
  emailId: EmailId,
  envelope: EnvelopeSchema.nullable(),
  id: EmailSubmissionId,
  identityId: IdentityId,
  mdnBlobIds: z.array(BlobId),
  sendAt: z.string(),
  threadId: ThreadId,
  undoStatus: z.string(),
  /** The bridge's vendor property (`SEND_WHEN_SIGNED_OUT`). */
  "https://helixnebula.tech/jmap/bridge/sendWhenSignedOut": z
    .boolean()
    .optional(),
});
export type EmailSubmission = z.infer<typeof EmailSubmissionSchema>;
export type EmailSubmissionProperty = keyof typeof EmailSubmissionSchema.shape;

export interface EmailSubmissionCreate {
  readonly identityId: IdentityId;
  /** An existing id, or `#creationId` of an Email created in this request. */
  readonly emailId: EmailId | `#${string}`;
  /**
   * Omit to let the server derive it from the headers. RFC 4865
   * FUTURERELEASE goes in `mailFrom.parameters` (`HOLDUNTIL`).
   */
  readonly envelope?: {
    readonly mailFrom: {
      readonly email: string;
      readonly parameters?: Readonly<Record<string, string | null>> | null;
    };
    readonly rcptTo: readonly {
      readonly email: string;
      readonly parameters?: Readonly<Record<string, string | null>> | null;
    }[];
  };
  /**
   * The bridge's vendor property (`SEND_WHEN_SIGNED_OUT`, with
   * `CAPABILITIES.bridge`): a held message goes out after sign-out too.
   */
  readonly "https://helixnebula.tech/jmap/bridge/sendWhenSignedOut"?: boolean;
}

/** An update: cancel, or (the bridge) change `SEND_WHEN_SIGNED_OUT`. */
export type EmailSubmissionPatch =
  | { readonly undoStatus: "canceled" }
  | {
      readonly "https:~1~1helixnebula.tech~1jmap~1bridge~1sendWhenSignedOut": boolean;
    };

export interface EmailSubmissionSetArgs extends SetArgs<
  EmailSubmissionId,
  EmailSubmissionCreate,
  EmailSubmissionPatch
> {
  /** Keys: submission id or `#creationId`. Usually moves Drafts → Sent. */
  readonly onSuccessUpdateEmail?: Readonly<Record<string, EmailPatch>>;
  readonly onSuccessDestroyEmail?: readonly (
    | EmailSubmissionId
    | `#${string}`
  )[];
}

/** RFC 8621 §7.3: `EmailSubmission/query` filter. */
export interface EmailSubmissionFilterCondition {
  readonly identityIds?: readonly IdentityId[];
  readonly emailIds?: readonly EmailId[];
  readonly threadIds?: readonly ThreadId[];
  readonly undoStatus?: "pending" | "final" | "canceled";
  readonly before?: string;
  readonly after?: string;
}

export type EmailSubmissionQueryArgs = QueryArgs<
  EmailSubmissionFilterCondition,
  Comparator<"emailId" | "threadId" | "sentAt">
>;

// ---------------------------------------------------------------- SearchSnippet

export const SearchSnippetSchema = z.looseObject({
  emailId: EmailId,
  preview: z.string().nullable(),
  subject: z.string().nullable(),
});
/** `subject`/`preview` contain HTML `<mark>` tags: sanitize before use. */
export type SearchSnippet = z.infer<typeof SearchSnippetSchema>;

export interface SearchSnippetGetArgs {
  readonly accountId: AccountId;
  readonly filter?: EmailFilter | null;
  readonly emailIds: readonly EmailId[];
}

// ---------------------------------------------------------------- VacationResponse

export const VacationResponseSchema = z.looseObject({
  fromDate: z.string().nullable(),
  htmlBody: z.string().nullable(),
  id: VacationResponseId,
  isEnabled: z.boolean(),
  subject: z.string().nullable(),
  textBody: z.string().nullable(),
  toDate: z.string().nullable(),
});
export type VacationResponse = z.infer<typeof VacationResponseSchema>;
export type VacationResponseProperty =
  keyof typeof VacationResponseSchema.shape;

export interface VacationResponsePatch {
  readonly isEnabled?: boolean;
  readonly fromDate?: string | null;
  readonly toDate?: string | null;
  readonly subject?: string | null;
  readonly textBody?: string | null;
  /** Sanitize like any other HTML before showing it back to the user. */
  readonly htmlBody?: string | null;
}

export interface VacationResponseSetArgs {
  readonly accountId: AccountId;
  readonly ifInState?: State;
  readonly update: {
    readonly singleton: VacationResponsePatch;
  };
}

// ---------------------------------------------------------------- arg aliases

export type MailboxGetArgs = GetArgs<MailboxId, MailboxProperty>;
export type ThreadGetArgs = GetArgs<ThreadId, "emailIds" | "id">;
export type IdentityGetArgs = GetArgs<IdentityId, IdentityProperty>;
export type EmailSubmissionGetArgs = GetArgs<
  EmailSubmissionId,
  EmailSubmissionProperty
>;
export interface VacationResponseGetArgs {
  readonly accountId: AccountId;
  readonly ids?: readonly VacationResponseId[] | null;
  readonly properties?: readonly VacationResponseProperty[];
}
