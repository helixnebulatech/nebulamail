// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Message Disposition Notifications over JMAP (RFC 9007): `MDN/send`
// sends a read receipt (RFC 8098) for an email of the account. The usual
// call sets `$mdnsent` on that email in the same request
// (`onSuccessUpdateEmail`), so no client asks again.

import { z } from "zod";

import { CAPABILITIES } from "./capabilities";
import { invalidResponse, parseOrThrow } from "./errors";
import { AccountId, EmailId } from "./ids";
import type { IdentityId } from "./ids";
import { EmailSchema } from "./mail";
import type { Email, EmailPatch } from "./mail";
import { SetErrorSchema, setResponseSchema } from "./standard";
import type { SetError, SetResponse } from "./standard";

/** RFC 8098 §3.2.6. */
export interface MdnDisposition {
  readonly actionMode: "manual-action" | "automatic-action";
  readonly sendingMode: "mdn-sent-manually" | "mdn-sent-automatically";
  readonly type: "deleted" | "dispatched" | "displayed" | "processed";
}

/** An MDN to send (RFC 9007 §2). The server fills in the rest. */
export interface MdnCreate {
  readonly forEmailId: EmailId;
  readonly subject?: string | null;
  readonly textBody?: string | null;
  readonly includeOriginalMessage?: boolean;
  readonly reportingUA?: string | null;
  readonly disposition: MdnDisposition;
  readonly finalRecipient?: string | null;
  readonly extensionFields?: Readonly<Record<string, string>> | null;
}

export interface MdnSendArgs {
  readonly accountId: AccountId;
  readonly identityId: IdentityId;
  readonly send: Readonly<Record<string, MdnCreate>>;
  /** Keys: `#creationId` of `send`. Usually `keywords/$mdnsent: true`. */
  readonly onSuccessUpdateEmail?: Readonly<Record<`#${string}`, EmailPatch>>;
}

export interface MdnSendResponse {
  readonly accountId: AccountId;
  /** Creation id → what the server set (Final-Recipient…). */
  readonly sent: Partial<Record<string, Readonly<Record<string, unknown>>>>;
  readonly notSent: Partial<Record<string, SetError>>;
  /** Implicit `Email/set` from `onSuccessUpdateEmail`. */
  readonly emailSet?: SetResponse<Email, EmailId>;
}

interface Spec<A, R> {
  readonly args: A;
  readonly response: R;
}

/** Merged into the method registry (methods.ts). */
export interface MdnMethodMap {
  "MDN/send": Spec<MdnSendArgs, MdnSendResponse>;
}

const MdnSendSchema = z.looseObject({
  accountId: AccountId,
  notSent: z
    .record(z.string(), SetErrorSchema)
    .nullish()
    .transform((record) => record ?? {}),
  sent: z
    .record(z.string(), z.record(z.string(), z.unknown()).nullable())
    .nullish()
    .transform((record) =>
      Object.fromEntries(
        Object.entries(record ?? {}).map(([key, value]) => [key, value ?? {}])
      )
    ),
});

const EmailSetSchema = setResponseSchema(
  EmailSchema.partial().extend({ id: EmailId }),
  EmailId,
  EmailSchema.partial()
);

type Invocation = readonly [string, Record<string, unknown>, string];

const parseMdnSend = (
  _args: unknown,
  raw: Record<string, unknown>,
  extra: readonly Invocation[]
): MdnSendResponse => {
  const response = parseOrThrow(MdnSendSchema, raw);
  const implicit = extra.find(([name]) => name === "Email/set");
  if (implicit === undefined) {
    return response;
  }
  const emailSet = EmailSetSchema.safeParse(implicit[1]);
  if (!emailSet.success) {
    throw invalidResponse(emailSet.error);
  }
  return { ...response, emailSet: emailSet.data };
};

export const MDN_METHODS = {
  "MDN/send": {
    parse: parseMdnSend,
    using: [CAPABILITIES.mail, CAPABILITIES.mdn],
  },
} as const;
