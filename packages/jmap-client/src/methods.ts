// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// The method registry: method name → capability, argument type, response
// type and runtime response parser. Adding a method means adding one entry to
// `MethodMap` and one to `METHODS`; the compiler enforces that both exist.

import { z } from "zod";

import { CALENDAR_METHODS } from "./calendars";
import type { CalendarMethodMap } from "./calendars";
import { CAPABILITIES } from "./capabilities";
import {
  AddressBookSchema,
  CONTACT_CARD_FILL_INS,
  ContactCardParseResponseSchema,
  ContactCardSchema,
} from "./contacts";
import type {
  AddressBook,
  AddressBookGetArgs,
  AddressBookProperty,
  AddressBookSetArgs,
  ContactCard,
  ContactCardGetArgs,
  ContactCardParseArgs,
  ContactCardParseResponse,
  ContactCardProperty,
  ContactCardQueryArgs,
  ContactCardSetArgs,
} from "./contacts";
import { JmapError, parseOrThrow } from "./errors";
import { FILENODE_METHODS } from "./filenode";
import type { FileNodeMethodMap } from "./filenode";
import {
  AccountId,
  AddressBookId,
  BlobId,
  ContactCardId,
  EmailId,
  EmailSubmissionId,
  IdentityId,
  MailboxId,
  PrincipalId,
  QuotaId,
  State,
  ThreadId,
  VacationResponseId,
} from "./ids";
import {
  DEFAULT_EMAIL_PROPERTIES,
  EmailSchema,
  EmailSubmissionSchema,
  IdentitySchema,
  MailboxSchema,
  SearchSnippetSchema,
  ThreadSchema,
  VacationResponseSchema,
} from "./mail";
import type {
  DefaultEmailProperty,
  Email,
  EmailGetArgs,
  EmailImportArgs,
  EmailQueryArgs,
  EmailQueryChangesArgs,
  EmailSetArgs,
  EmailSubmission,
  EmailSubmissionGetArgs,
  EmailSubmissionQueryArgs,
  EmailSubmissionSetArgs,
  Identity,
  IdentityGetArgs,
  IdentitySetArgs,
  Mailbox,
  MailboxGetArgs,
  MailboxSetArgs,
  SearchSnippet,
  SearchSnippetGetArgs,
  Thread,
  ThreadGetArgs,
  VacationResponse,
  VacationResponseGetArgs,
  VacationResponseSetArgs,
} from "./mail";
import { MDN_METHODS } from "./mdn";
import type { MdnMethodMap } from "./mdn";
import { PrincipalSchema } from "./principals";
import type {
  Principal,
  PrincipalGetArgs,
  PrincipalProperty,
  PrincipalQueryArgs,
} from "./principals";
import { PUSH_SUBSCRIPTION_METHODS } from "./push-subscription";
import type { PushSubscriptionMethodMap } from "./push-subscription";
import { QuotaSchema } from "./quota";
import type { Quota, QuotaGetArgs, QuotaProperty } from "./quota";
import { SIEVE_METHODS } from "./sieve";
import type { SieveMethodMap } from "./sieve";
import {
  SetErrorSchema,
  UnsignedInt,
  changesResponseSchema,
  getResponseSchema,
  queryChangesResponseSchema,
  queryResponseSchema,
  setResponseSchema,
} from "./standard";
import type {
  ChangesArgs,
  ChangesResponse,
  GetResponse,
  Lenient,
  QueryChangesResponse,
  QueryResponse,
  SetError,
  SetResponse,
} from "./standard";

// ---------------------------------------------------------------- types

/** `properties` selection: requested keys (always + `id`) of `T`. */
export type Selected<T, P> = Pick<T, Extract<P | "id", keyof T>> &
  Readonly<Record<Extract<P, `header:${string}`>, unknown>>;

/** Narrows a /get list item to the requested `properties`, if given. */
type Selection<T, A, Default extends keyof T = keyof T> = A extends {
  readonly properties: readonly (infer P)[];
}
  ? Selected<T, P>
  : Pick<T, Default>;

export interface MailboxChangesResponse extends ChangesResponse<MailboxId> {
  /** Non-null if only counts changed (RFC 8621 §2.2). */
  readonly updatedProperties: string[] | null;
}

export interface EmailImportResponse {
  readonly accountId: AccountId;
  readonly oldState: State | null;
  /** null when the server left it out (see SetResponse). */
  readonly newState: State | null;
  readonly created: Partial<
    Record<
      string,
      {
        readonly id: EmailId;
        readonly blobId: BlobId;
        readonly threadId: ThreadId;
        readonly size: number;
      }
    >
  >;
  readonly notCreated: Partial<Record<string, SetError>>;
}

/** RFC 8620 §6.3: copies blobs from one account into another. */
export interface BlobCopyArgs {
  readonly fromAccountId: AccountId;
  readonly accountId: AccountId;
  readonly blobIds: readonly BlobId[];
}

export interface BlobCopyResponse {
  readonly fromAccountId: AccountId;
  readonly accountId: AccountId;
  /** Original blob id → its id in `accountId`. */
  readonly copied: Partial<Record<string, BlobId>>;
  readonly notCopied: Partial<Record<string, SetError>>;
}

export interface SearchSnippetGetResponse {
  readonly accountId: AccountId;
  readonly list: SearchSnippet[];
  readonly notFound: EmailId[];
}

export interface EmailSubmissionSetResponse extends SetResponse<
  EmailSubmission,
  EmailSubmissionId
> {
  /** Implicit `Email/set` from `onSuccessUpdateEmail`/`DestroyEmail`. */
  readonly emailSet?: SetResponse<Email, EmailId>;
  /**
   * The implicit `Email/set` came back as `["error", …]` or didn't match
   * RFC 8621 (`emailSet` is then absent). The submissions themselves are
   * still reported: a send that went out must never look like a failure.
   */
  readonly emailSetFailed?: true;
}

interface Spec<A, R> {
  readonly args: A;
  readonly response: R;
}

/** Method name → arguments and (widest) response. */
export interface MethodMap
  extends
    SieveMethodMap,
    CalendarMethodMap,
    FileNodeMethodMap,
    MdnMethodMap,
    PushSubscriptionMethodMap {
  "AddressBook/get": Spec<
    AddressBookGetArgs,
    GetResponse<Lenient<AddressBook>, AddressBookId>
  >;
  "AddressBook/set": Spec<
    AddressBookSetArgs,
    SetResponse<AddressBook, AddressBookId>
  >;
  "ContactCard/get": Spec<
    ContactCardGetArgs,
    GetResponse<Lenient<ContactCard>, ContactCardId>
  >;
  "ContactCard/parse": Spec<ContactCardParseArgs, ContactCardParseResponse>;
  "ContactCard/query": Spec<ContactCardQueryArgs, QueryResponse<ContactCardId>>;
  "ContactCard/set": Spec<
    ContactCardSetArgs,
    SetResponse<ContactCard, ContactCardId>
  >;
  "Blob/copy": Spec<BlobCopyArgs, BlobCopyResponse>;
  "Core/echo": Spec<Readonly<Record<string, unknown>>, Record<string, unknown>>;
  "Email/changes": Spec<ChangesArgs, ChangesResponse<EmailId>>;
  "Email/get": Spec<EmailGetArgs, GetResponse<Lenient<Email>, EmailId>>;
  "Email/import": Spec<EmailImportArgs, EmailImportResponse>;
  "Email/query": Spec<EmailQueryArgs, QueryResponse<EmailId>>;
  "Email/queryChanges": Spec<
    EmailQueryChangesArgs,
    QueryChangesResponse<EmailId>
  >;
  "Email/set": Spec<EmailSetArgs, SetResponse<Email, EmailId>>;
  "EmailSubmission/get": Spec<
    EmailSubmissionGetArgs,
    GetResponse<Lenient<EmailSubmission>, EmailSubmissionId>
  >;
  "EmailSubmission/query": Spec<
    EmailSubmissionQueryArgs,
    QueryResponse<EmailSubmissionId>
  >;
  "EmailSubmission/set": Spec<
    EmailSubmissionSetArgs,
    EmailSubmissionSetResponse
  >;
  "Identity/changes": Spec<ChangesArgs, ChangesResponse<IdentityId>>;
  "Identity/get": Spec<
    IdentityGetArgs,
    GetResponse<Lenient<Identity>, IdentityId>
  >;
  "Identity/set": Spec<IdentitySetArgs, SetResponse<Identity, IdentityId>>;
  "Mailbox/changes": Spec<ChangesArgs, MailboxChangesResponse>;
  "Mailbox/get": Spec<MailboxGetArgs, GetResponse<Lenient<Mailbox>, MailboxId>>;
  "Mailbox/set": Spec<MailboxSetArgs, SetResponse<Mailbox, MailboxId>>;
  "Principal/get": Spec<
    PrincipalGetArgs,
    GetResponse<Lenient<Principal>, PrincipalId>
  >;
  "Principal/query": Spec<PrincipalQueryArgs, QueryResponse<PrincipalId>>;
  "Quota/get": Spec<QuotaGetArgs, GetResponse<Lenient<Quota>, QuotaId>>;
  "SearchSnippet/get": Spec<SearchSnippetGetArgs, SearchSnippetGetResponse>;
  "Thread/changes": Spec<ChangesArgs, ChangesResponse<ThreadId>>;
  "Thread/get": Spec<ThreadGetArgs, GetResponse<Lenient<Thread>, ThreadId>>;
  "VacationResponse/get": Spec<
    VacationResponseGetArgs,
    GetResponse<Lenient<VacationResponse>, VacationResponseId>
  >;
  "VacationResponse/set": Spec<
    VacationResponseSetArgs,
    SetResponse<VacationResponse, VacationResponseId>
  >;
}

export type MethodName = keyof MethodMap;
export type MethodArgs<M extends MethodName> = MethodMap[M]["args"];

type GetOf<T, Id, A, Default extends keyof T = keyof T> = Omit<
  GetResponse<T, Id>,
  "list"
> & { readonly list: Selection<T, A, Default>[] };

/** The response type of method `M` called with arguments `A`. */
export type ResponseFor<M extends MethodName, A> = M extends "Email/get"
  ? GetOf<Email, EmailId, A, DefaultEmailProperty>
  : M extends "Mailbox/get"
    ? GetOf<Mailbox, MailboxId, A>
    : M extends "Thread/get"
      ? GetOf<Thread, ThreadId, A>
      : M extends "Identity/get"
        ? GetOf<Identity, IdentityId, A>
        : M extends "EmailSubmission/get"
          ? GetOf<EmailSubmission, EmailSubmissionId, A>
          : M extends "VacationResponse/get"
            ? GetOf<VacationResponse, VacationResponseId, A>
            : M extends "ContactCard/get"
              ? GetOf<ContactCard, ContactCardId, A, ContactCardProperty>
              : M extends "AddressBook/get"
                ? GetOf<AddressBook, AddressBookId, A, AddressBookProperty>
                : M extends "Quota/get"
                  ? GetOf<Quota, QuotaId, A, QuotaProperty>
                  : M extends "Principal/get"
                    ? GetOf<Principal, PrincipalId, A, PrincipalProperty>
                    : MethodMap[M]["response"];

// ---------------------------------------------------------------- runtime

/** `[name, arguments, callId]` as received. */
export type Invocation = readonly [string, Record<string, unknown>, string];

export interface MethodSpec<M extends MethodName> {
  readonly using: readonly string[];
  readonly parse: (
    args: Readonly<Record<string, unknown>>,
    raw: Record<string, unknown>,
    extra: readonly Invocation[]
  ) => MethodMap[M]["response"];
}

const requestedProperties = (
  args: Readonly<Record<string, unknown>>,
  defaults: readonly string[]
): readonly string[] => {
  const { properties } = args;
  if (!Array.isArray(properties)) {
    return defaults;
  }
  const names = properties.filter(
    (name): name is string => typeof name === "string"
  );
  return names.includes("id") ? names : [...names, "id"];
};

/**
 * /get with `properties`: parse every item leniently (all properties
 * optional), then require that each requested property is present (its value
 * may be `null`). This is what lets the type system narrow the list items to
 * exactly the requested properties.
 */
type LooseObject<Shape extends z.ZodRawShape> = z.ZodObject<
  Shape,
  z.core.$loose
>;

const parseGet = <Shape extends z.ZodRawShape, I extends z.ZodType>(
  item: LooseObject<Shape>,
  id: I,
  defaults: readonly string[],
  /**
   * Values for properties some servers leave out instead of sending an
   * empty value. Applied before the "every requested property is present"
   * check.
   */
  fillIns: Readonly<Record<string, unknown>> = {}
) => {
  const schema = getResponseSchema(item.partial(), id);
  return (
    args: Readonly<Record<string, unknown>>,
    raw: Record<string, unknown>
  ) => {
    const parsed = parseOrThrow(schema, raw);
    const wanted = requestedProperties(args, defaults);
    for (const entry of parsed.list) {
      for (const [name, value] of Object.entries(fillIns)) {
        if (wanted.includes(name) && !Object.hasOwn(entry, name)) {
          Object.assign(entry, { [name]: value });
        }
      }
    }
    const issues = parsed.list.flatMap((entry, index) =>
      wanted
        .filter((name) => !Object.hasOwn(entry, name))
        .map((name) => `list.${String(index)}.${name}: missing`)
    );
    if (issues.length > 0) {
      throw new JmapError({ issues, kind: "invalidResponse" });
    }
    return parsed;
  };
};

const setSchema = <Shape extends z.ZodRawShape, I extends z.ZodType>(
  item: LooseObject<Shape>,
  id: I
) =>
  setResponseSchema(item.partial().extend({ id }), id).extend({
    updated: z
      .record(z.string(), item.partial().nullable())
      .nullish()
      .transform((record) => record ?? {}),
  });

const parser =
  <T>(schema: z.ZodType<T>) =>
  (_args: unknown, raw: Record<string, unknown>): T =>
    parseOrThrow(schema, raw);

const keysOf = (schema: z.ZodObject): readonly string[] =>
  Object.keys(schema.shape);

const EmailSetSchema = setSchema(EmailSchema, EmailId);

const MailboxChangesSchema = changesResponseSchema(MailboxId).extend({
  updatedProperties: z
    .array(z.string())
    .nullish()
    .transform((names) => names ?? null),
});

const EmailImportSchema = z.looseObject({
  accountId: AccountId,
  created: z
    .record(
      z.string(),
      z.looseObject({
        blobId: BlobId,
        id: EmailId,
        size: UnsignedInt,
        threadId: ThreadId,
      })
    )
    .nullish()
    .transform((record) => record ?? {}),
  // As in /set (standard.ts): Stalwart 1.0.1 leaves it out when it
  // refused everything, and the refusals must still be read.
  newState: State.nullish().transform((state) => state ?? null),
  notCreated: z
    .record(z.string(), SetErrorSchema)
    .nullish()
    .transform((record) => record ?? {}),
  oldState: State.nullish().transform((state) => state ?? null),
});

const BlobCopySchema = z.looseObject({
  accountId: AccountId,
  copied: z
    .record(z.string(), BlobId)
    .nullish()
    .transform((record) => record ?? {}),
  fromAccountId: AccountId,
  notCopied: z
    .record(z.string(), SetErrorSchema)
    .nullish()
    .transform((record) => record ?? {}),
});

const SearchSnippetGetSchema = z.looseObject({
  accountId: AccountId,
  list: z.array(SearchSnippetSchema),
  notFound: z
    .array(EmailId)
    .nullish()
    .transform((ids) => ids ?? []),
});

const SubmissionSetSchema = setSchema(EmailSubmissionSchema, EmailSubmissionId);

const parseSubmissionSet = (
  _args: unknown,
  raw: Record<string, unknown>,
  extra: readonly Invocation[]
): EmailSubmissionSetResponse => {
  const response = parseOrThrow(SubmissionSetSchema, raw);
  // Same callId (RFC 8620 §3.6.2): the implicit call, or its error.
  const implicit = extra.find(
    ([name]) => name === "Email/set" || name === "error"
  );
  if (implicit === undefined) {
    return response;
  }
  const emailSet =
    implicit[0] === "Email/set" ? EmailSetSchema.safeParse(implicit[1]) : null;
  return emailSet?.success === true
    ? { ...response, emailSet: emailSet.data }
    : { ...response, emailSetFailed: true };
};

const {
  contacts,
  contactsParse,
  core,
  mail,
  principals,
  quota,
  submission,
  vacationResponse,
} = CAPABILITIES;
const MAIL = [mail];
const SUBMISSION = [mail, submission];

export const METHODS: { readonly [M in MethodName]: MethodSpec<M> } = {
  ...CALENDAR_METHODS,
  ...PUSH_SUBSCRIPTION_METHODS,
  ...MDN_METHODS,
  "AddressBook/get": {
    parse: parseGet(
      AddressBookSchema,
      AddressBookId,
      keysOf(AddressBookSchema)
    ),
    using: [contacts],
  },
  "AddressBook/set": {
    parse: parser(setSchema(AddressBookSchema, AddressBookId)),
    using: [contacts],
  },
  "ContactCard/get": {
    // Stalwart leaves out properties a card doesn't have, even when they
    // are asked for by name (RFC 8620: null).
    parse: parseGet(
      ContactCardSchema,
      ContactCardId,
      keysOf(ContactCardSchema),
      CONTACT_CARD_FILL_INS
    ),
    using: [contacts],
  },
  "ContactCard/parse": {
    parse: parser(ContactCardParseResponseSchema),
    using: [contacts, contactsParse],
  },
  "ContactCard/query": {
    parse: parser(queryResponseSchema(ContactCardId)),
    using: [contacts],
  },
  "ContactCard/set": {
    parse: parser(setSchema(ContactCardSchema, ContactCardId)),
    using: [contacts],
  },
  "Blob/copy": { parse: parser(BlobCopySchema), using: [core] },
  "Core/echo": { parse: (_args, raw) => raw, using: [core] },
  "Email/changes": {
    parse: parser(changesResponseSchema(EmailId)),
    using: MAIL,
  },
  "Email/get": {
    // `preview` is "" for an empty body (RFC 8621); fill it in if a
    // server leaves it out.
    parse: parseGet(EmailSchema, EmailId, DEFAULT_EMAIL_PROPERTIES, {
      preview: "",
    }),
    using: MAIL,
  },
  "Email/import": { parse: parser(EmailImportSchema), using: MAIL },
  "Email/query": { parse: parser(queryResponseSchema(EmailId)), using: MAIL },
  "Email/queryChanges": {
    parse: parser(queryChangesResponseSchema(EmailId)),
    using: MAIL,
  },
  "Email/set": { parse: parser(EmailSetSchema), using: MAIL },
  "EmailSubmission/get": {
    parse: parseGet(
      EmailSubmissionSchema,
      EmailSubmissionId,
      keysOf(EmailSubmissionSchema)
    ),
    using: SUBMISSION,
  },
  "EmailSubmission/query": {
    parse: parser(queryResponseSchema(EmailSubmissionId)),
    using: SUBMISSION,
  },
  "EmailSubmission/set": { parse: parseSubmissionSet, using: SUBMISSION },
  "Identity/changes": {
    parse: parser(changesResponseSchema(IdentityId)),
    using: SUBMISSION,
  },
  "Identity/get": {
    parse: parseGet(IdentitySchema, IdentityId, keysOf(IdentitySchema)),
    using: SUBMISSION,
  },
  "Identity/set": {
    parse: parser(setSchema(IdentitySchema, IdentityId)),
    using: SUBMISSION,
  },
  "Mailbox/changes": { parse: parser(MailboxChangesSchema), using: MAIL },
  "Mailbox/get": {
    // `shareWith` only exists with mail:share (add it with `b.using`).
    parse: parseGet(MailboxSchema, MailboxId, keysOf(MailboxSchema), {
      shareWith: null,
    }),
    using: MAIL,
  },
  "Mailbox/set": {
    parse: parser(setSchema(MailboxSchema, MailboxId)),
    using: MAIL,
  },
  "Principal/get": {
    parse: parseGet(PrincipalSchema, PrincipalId, keysOf(PrincipalSchema), {
      description: null,
      email: null,
      timeZone: null,
    }),
    using: [principals],
  },
  "Principal/query": {
    parse: parser(queryResponseSchema(PrincipalId)),
    using: [principals],
  },
  "Quota/get": {
    parse: parseGet(QuotaSchema, QuotaId, keysOf(QuotaSchema), {
      description: null,
      softLimit: null,
      warnLimit: null,
    }),
    // RFC 9425 §4.1: a Quota only names (and is only shown with) the types
    // of capabilities in `using`; the web looks for `Email`.
    using: [quota, mail],
  },
  "SearchSnippet/get": { parse: parser(SearchSnippetGetSchema), using: MAIL },
  ...SIEVE_METHODS,
  ...FILENODE_METHODS,
  "Thread/changes": {
    parse: parser(changesResponseSchema(ThreadId)),
    using: MAIL,
  },
  "Thread/get": {
    parse: parseGet(ThreadSchema, ThreadId, keysOf(ThreadSchema)),
    using: MAIL,
  },
  "VacationResponse/get": {
    parse: parseGet(
      VacationResponseSchema,
      VacationResponseId,
      keysOf(VacationResponseSchema)
    ),
    using: [vacationResponse],
  },
  "VacationResponse/set": {
    parse: parser(setSchema(VacationResponseSchema, VacationResponseId)),
    using: [vacationResponse],
  },
};
