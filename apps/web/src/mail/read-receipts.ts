// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Read receipts (MDN, RFC 8098, sent with JMAP `MDN/send`, RFC 9007).
// A receipt tells the sender when you opened the email, so it is a privacy
// choice: "never / ask / always", "ask" by default. Only
// offered when the server has `urn:ietf:params:jmap:mdn`.
//
// Whatever the choice, `$mdnsent` (RFC 3503) is set once the user sent
// one or said no: no client asks again for that email. "always" sends on
// opening, but degrades to asking (RFC 8098 §2.1) when:
// - the email is in Junk;
// - it asks for more than one address;
// - the address isn't the Return-Path (someone could ask for receipts to
//   go to a third party, or to learn that this address is read).
// Our own messages, drafts and other users' shared folders never ask.

import type { EmailId, IdentityId, JmapClient } from "@nebulamail/jmap-client";
import {
  CAPABILITIES,
  hasCapability,
  keywordPatch,
} from "@nebulamail/jmap-client";
import { z } from "zod";

import type { MailAccount } from "./account-scope";
import { getClient } from "./client";
import { sameAddress } from "./recipients";

export const READ_RECEIPT_MODES = ["never", "ask", "always"] as const;
export type ReadReceiptMode = (typeof READ_RECEIPT_MODES)[number];
export const ReadReceiptModeSchema = z.enum(READ_RECEIPT_MODES);
export const DEFAULT_READ_RECEIPTS: ReadReceiptMode = "ask";
/** Where the device keeps the choice (localStorage; synced). */
export const READ_RECEIPTS_KEY = "wm-read-receipts";

/** The keyword that says "already answered" (RFC 3503, IANA). */
export const MDN_SENT = "$mdnsent";

/** The bridge sends to 5 addresses at most; more is not a real request. */
const MAX_ADDRESSES = 5;

/** What an open email says about a receipt (from api.ts `getEmail`). */
export interface ReceiptRequest {
  /** Disposition-Notification-To addresses. */
  readonly to: readonly string[];
  /** The Return-Path address, if the server kept one. */
  readonly returnPath: string | null;
  /** `$mdnsent`: answered already (here or in another client). */
  readonly answered: boolean;
}

export interface ReceiptContext {
  readonly mode: ReadReceiptMode;
  /** The session has `urn:ietf:params:jmap:mdn`. */
  readonly supported: boolean;
  /** The user's own account (not a folder someone shares). */
  readonly ownAccount: boolean;
  readonly inJunk: boolean;
  /** We wrote it (Sent, our replies) or it is a draft. */
  readonly ours: boolean;
}

export type ReceiptAction = "none" | "ask" | "send";

/** Whether the receipt could go out without asking (RFC 8098 §2.1). */
export const safeToSendAlone = (request: ReceiptRequest): boolean => {
  const [only] = request.to;
  return (
    request.to.length === 1 &&
    only !== undefined &&
    request.returnPath !== null &&
    sameAddress(only, request.returnPath)
  );
};

export const receiptAction = (
  request: ReceiptRequest | null,
  context: ReceiptContext
): ReceiptAction => {
  if (
    request === null ||
    request.answered ||
    request.to.length === 0 ||
    request.to.length > MAX_ADDRESSES ||
    !context.supported ||
    !context.ownAccount ||
    context.ours ||
    context.mode === "never"
  ) {
    return "none";
  }
  if (
    context.mode === "always" &&
    !context.inJunk &&
    safeToSendAlone(request)
  ) {
    return "send";
  }
  return "ask";
};

/**
 * The address inside a Return-Path (`<bob@example.com>`, maybe with
 * comments or spaces). `<>` (bounces) or nonsense: null.
 */
export const returnPathAddress = (value: string | null): string | null => {
  if (value === null) {
    return null;
  }
  const match = /<(?<address>[^<>\s]*)>/u.exec(value);
  const address = (match?.groups?.address ?? value).trim();
  return /^[^\s@<>()]+@[^\s@<>()]+$/u.test(address) ? address : null;
};

const Addresses = z
  .array(z.object({ email: z.string().nullish() }))
  .nullable()
  .catch(null);
const Text = z.string().nullable().catch(null);

/**
 * Email/get properties for `readReceiptRequest`. Return-Path is an RFC
 * 5322 field: only the raw form is allowed for it (RFC 8621 §4.1.2;
 * `:asText` makes the bridge, and the RFC, reject the whole call).
 */
export const RECEIPT_PROPERTIES = [
  "header:Disposition-Notification-To:asAddresses",
  "header:Return-Path",
] as const;

/** Reads RECEIPT_PROPERTIES (and the keywords) of an Email/get result. */
export const readReceiptRequest = (
  email: Readonly<Record<string, unknown>>,
  keywords: Readonly<Record<string, boolean | undefined>>
): ReceiptRequest | null => {
  const to = (
    Addresses.parse(email["header:Disposition-Notification-To:asAddresses"]) ??
    []
  ).flatMap((address) =>
    address.email === null ||
    address.email === undefined ||
    address.email.trim() === ""
      ? []
      : [address.email.trim()]
  );
  if (to.length === 0) {
    return null;
  }
  return {
    answered: keywords[MDN_SENT] === true,
    returnPath: returnPathAddress(Text.parse(email["header:Return-Path"])),
    to,
  };
};

/** Whether the session can send receipts (own account). */
export const supportsReceipts = (client: JmapClient): boolean =>
  hasCapability(client.session, CAPABILITIES.mdn, client.primaryAccountId());

/** The identity that answers: the one the email was sent to, else the first. */
const pickIdentity = async (
  client: JmapClient,
  recipients: readonly string[]
): Promise<IdentityId> => {
  const [result] = await client.request((b) => [
    b.call("Identity/get", {
      accountId: client.primaryAccountId(),
      properties: ["email"],
    }),
  ]);
  const match = result.list.find((identity) =>
    recipients.some((address) => sameAddress(address, identity.email))
  );
  const identity = match ?? result.list[0];
  if (identity === undefined) {
    throw new Error("No identity to send the receipt from");
  }
  return identity.id;
};

/** Header text: one line, no controls (the server checks it too). */
const oneLine = (text: string, max: number): string =>
  text
    .replaceAll(/\p{Cc}+/gu, " ")
    .replaceAll(/\s+/gu, " ")
    .trim()
    .slice(0, max);

export interface ReceiptText {
  readonly subject: string;
  readonly body: string;
}

/**
 * Sends the receipt and sets `$mdnsent` in the same request. `automatic`:
 * sent because of "always" (RFC 8098 `mdn-sent-automatically`). The user
 * displayed the email either way (`manual-action`).
 */
export const sendReadReceipt = async (
  emailId: EmailId,
  recipients: readonly string[],
  text: ReceiptText,
  automatic: boolean,
  account: MailAccount = null
): Promise<void> => {
  if (account !== null) {
    throw new Error("Receipts are only sent from the user's own account");
  }
  const client = await getClient();
  const identityId = await pickIdentity(client, recipients);
  const subject = oneLine(text.subject, 900);
  const [result] = await client.request((b) => [
    b.call("MDN/send", {
      accountId: client.primaryAccountId(),
      identityId,
      onSuccessUpdateEmail: { "#receipt": keywordPatch({ [MDN_SENT]: true }) },
      send: {
        receipt: {
          disposition: {
            actionMode: "manual-action",
            sendingMode: automatic
              ? "mdn-sent-automatically"
              : "mdn-sent-manually",
            type: "displayed",
          },
          forEmailId: emailId,
          includeOriginalMessage: false,
          ...(subject === "" ? {} : { subject }),
          textBody: text.body,
        },
      },
    }),
  ]);
  const refused = result.notSent.receipt;
  // Another client (or tab) answered first: that's what we wanted.
  if (refused !== undefined && refused.type !== "mdnAlreadySent") {
    throw new Error(`The receipt was not sent: ${refused.type}`);
  }
};

/** "Ignore": sends nothing, and no client asks again (`$mdnsent`). */
export const ignoreReadReceipt = async (emailId: EmailId): Promise<void> => {
  const client = await getClient();
  await client.request((b) => [
    b.call("Email/set", {
      accountId: client.primaryAccountId(),
      update: { [emailId]: keywordPatch({ [MDN_SENT]: true }) },
    }),
  ]);
};
