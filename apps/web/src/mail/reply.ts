// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Reply, reply all and forward → the initial state of a compose window.
// The original is quoted as HTML: its sanitized HTML (the
// same pipeline as the reader), or its text when it has no HTML part.
// Labels come translated.

import { DEFAULT_LIMITS } from "@nebulamail/mail-render";

import type { EmailDetail, Person } from "./api";
import type { DraftAttachment } from "./attachments";
import { editableBody, textToComposeHtml } from "./body";
import type { Recipient } from "./compose";
import { mergeRecipients, sameAddress } from "./recipients";

export type ReplyMode = "reply" | "replyAll" | "forward";

/** What a compose window starts with (all optional: a blank message). */
export interface ComposeInit {
  readonly to?: readonly Recipient[];
  readonly cc?: readonly Recipient[];
  readonly bcc?: readonly Recipient[];
  readonly subject?: string;
  /** Plain-text body (e.g. from a mailto: link). */
  readonly text?: string;
  /** Body as sanitized HTML; wins over `text`. */
  readonly html?: string;
  /** Quoted original (sanitized HTML), after the body and signature. */
  readonly quoteHtml?: string;
  /**
   * The body is a whole saved message (reopened draft, undone send): use it
   * as it is, without adding the signature again.
   */
  readonly restored?: boolean;
  /** Already on the server (forwarded or reopened): referenced by blobId. */
  readonly attachments?: readonly DraftAttachment[];
  readonly inReplyTo?: readonly string[];
  readonly references?: readonly string[];
  /** A restored draft that asked for a read receipt. */
  readonly readReceipt?: boolean;
  /** Send from this address if it is one of the user's identities. */
  readonly identityEmail?: string;
  /**
   * Start in plain-text mode (body.ts `initialPlainText`): a reopened
   * text-only draft, or a reply to a text-only email.
   */
  readonly plainText?: boolean;
}

/**
 * Turns an email body into sanitized editor HTML (see body.ts), keeping
 * `cid:` images for these Content-IDs.
 */
export type ToEditable = (
  html: string | null,
  text: string | null,
  inlineCids: readonly string[]
) => string;

/**
 * The original's inline images travel with the new message as inline
 * parts, by blobId (nothing is uploaded again). Unused ones (no longer in
 * the body) are left out when it is saved (body.ts, attachmentsInUse).
 */
const inlineAttachments = (email: EmailDetail): DraftAttachment[] =>
  email.inlineParts.map((part) => ({
    blobId: part.blobId,
    cid: part.cid,
    name: part.name,
    size: part.size,
    type: part.type,
    // From a shared folder: copied into our account when the reply opens.
    ...(part.accountId === undefined ? {} : { accountId: part.accountId }),
  }));

export interface QuoteLabels {
  /** "On 23 Oct 2026, 9:41, Ana wrote:" */
  readonly wrote: (date: string, sender: string) => string;
  /** The block above a forwarded message. */
  readonly forwarded: (fields: {
    readonly from: string;
    readonly date: string;
    readonly subject: string;
    readonly to: string;
  }) => string;
  readonly formatDate: (iso: string) => string;
  /** Instead of the body of a forward too long to edit: it goes attached. */
  readonly forwardedAttached: string;
  /** After a quote cut short (the original was too long). */
  readonly quoteTruncated: string;
}

/**
 * The body can go into the editor as it is: the server sent all of it
 * (`truncated`: cut at MAX_BODY_BYTES) and it isn't over the editor's
 * limit, past which it would become plain text.
 */
export const isEditableBody = (email: EmailDetail): boolean =>
  !email.truncated && (email.html?.length ?? 0) <= DEFAULT_LIMITS.maxHtmlLength;

/** The whole original as a `.eml` attachment (a forward too long to edit). */
const originalAsFile = (email: EmailDetail): DraftAttachment => ({
  blobId: email.blobId,
  name: `${email.subject.trim() === "" ? "message" : email.subject.trim().slice(0, 100)}.eml`,
  size: email.size,
  type: "message/rfc822",
  // From a shared folder: copied into our account when the forward opens.
  ...(email.accountId === null ? {} : { accountId: email.accountId }),
});

const REPLY_PREFIX = /^\s*(?:re|aw|sv|antw|r)\s*:/iu;
const FORWARD_PREFIX = /^\s*(?:fwd?|rv|wg|tr|enc)\s*:/iu;

export const replySubject = (subject: string, mode: ReplyMode): string => {
  const trimmed = subject.trim();
  if (mode === "forward") {
    return FORWARD_PREFIX.test(trimmed) ? trimmed : `Fwd: ${trimmed}`;
  }
  return REPLY_PREFIX.test(trimmed) ? trimmed : `Re: ${trimmed}`;
};

const toRecipient = (person: Person): Recipient =>
  person.name === null || person.name === ""
    ? { email: person.email }
    : { email: person.email, name: person.name };

export const personLabel = (person: Person): string =>
  person.name === null || person.name === ""
    ? person.email
    : `${person.name} <${person.email}>`;

const withoutOwn = (
  people: readonly Person[],
  own: readonly string[]
): Recipient[] =>
  people
    .filter((person) => !own.some((email) => sameAddress(email, person.email)))
    .map(toRecipient);

/** The identity the original was sent to, so the reply comes from it. */
const receivingIdentity = (
  email: EmailDetail,
  own: readonly string[]
): string | undefined =>
  own.find((address) =>
    [...email.to, ...email.cc].some((person) =>
      sameAddress(person.email, address)
    )
  );

const forwardHeader = (
  email: EmailDetail,
  labels: QuoteLabels,
  date: string,
  senderLabel: string
): string =>
  textToComposeHtml(
    labels.forwarded({
      date,
      from: senderLabel,
      subject: email.subject,
      to: email.to.map(personLabel).join(", "),
    })
  );

/**
 * RFC 5322 §3.6.4: the reply's References are the parent's References
 * followed by its Message-ID; if the parent has no References but an
 * In-Reply-To with exactly one id, that id instead, then its Message-ID.
 * With none of the three, nothing (no References field).
 */
export const replyReferences = (
  parent: Pick<EmailDetail, "references" | "inReplyTo" | "messageId">
): string[] => {
  if (parent.references.length > 0) {
    return [...parent.references, ...parent.messageId];
  }
  const single = parent.inReplyTo.length === 1 ? parent.inReplyTo : [];
  return [...single, ...parent.messageId];
};

/**
 * A forward of an email too long to edit: the whole original goes as one
 * `message/rfc822` attachment, with its files and images inside, so they
 * aren't added again (they would go twice).
 */
const forwardAttached = (
  email: EmailDetail,
  labels: QuoteLabels,
  date: string,
  senderLabel: string,
  own: readonly string[]
): ComposeInit => {
  const identityEmail = receivingIdentity(email, own);
  return {
    ...(identityEmail === undefined ? {} : { identityEmail }),
    attachments: [originalAsFile(email)],
    quoteHtml: `<div data-quote>${forwardHeader(email, labels, date, senderLabel)}<p></p>${textToComposeHtml(labels.forwardedAttached)}</div>`,
    subject: replySubject(email.subject, "forward"),
  };
};

export const buildReply = (
  email: EmailDetail,
  mode: ReplyMode,
  own: readonly string[],
  labels: QuoteLabels,
  toEditable: ToEditable = editableBody
): ComposeInit => {
  const date = labels.formatDate(email.sentAt ?? email.receivedAt);
  const [sender] = email.from;
  const senderLabel = sender === undefined ? "" : personLabel(sender);
  const editable = isEditableBody(email);
  if (mode === "forward" && !editable) {
    return forwardAttached(email, labels, date, senderLabel, own);
  }
  const quoted = toEditable(
    email.html,
    email.text ?? email.preview,
    email.inlineParts.map((part) => part.cid)
  );
  // A reply may quote part of it, saying so.
  const original = editable
    ? quoted
    : `${quoted}${textToComposeHtml(labels.quoteTruncated)}`;
  const inline = inlineAttachments(email);
  const identityEmail = receivingIdentity(email, own);
  const identity = {
    ...(identityEmail === undefined ? {} : { identityEmail }),
    // Text answers text: an email without HTML gets a plain-text reply.
    ...(email.html === null ? { plainText: true } : {}),
  };

  if (mode === "forward") {
    const header = forwardHeader(email, labels, date, senderLabel);
    return {
      ...identity,
      // A forward carries the original's files and inline images (the
      // blobs are already in the account, nothing is uploaded again).
      ...(email.attachments.length + inline.length > 0
        ? { attachments: [...email.attachments, ...inline] }
        : {}),
      quoteHtml: `<div data-quote>${header}<p></p>${original}</div>`,
      subject: replySubject(email.subject, mode),
    };
  }

  const fromMe =
    sender !== undefined &&
    own.some((address) => sameAddress(address, sender.email));
  // Replying to something I sent goes to its recipients (like Gmail).
  const primary = fromMe
    ? email.to.map(toRecipient)
    : (email.replyTo.length > 0 ? email.replyTo : email.from).map(toRecipient);
  const to =
    mode === "replyAll"
      ? mergeRecipients(primary, withoutOwn(email.to, own))
      : primary;
  const cc =
    mode === "replyAll"
      ? withoutOwn(email.cc, own).filter(
          (person) =>
            !to.some((other) => sameAddress(other.email, person.email))
        )
      : [];

  return {
    ...identity,
    // The quote keeps the original's inline images, not its files.
    ...(inline.length > 0 ? { attachments: inline } : {}),
    cc,
    inReplyTo: email.messageId,
    quoteHtml: `<div data-quote>${textToComposeHtml(
      labels.wrote(date, senderLabel)
    )}<blockquote>${original}</blockquote></div>`,
    references: replyReferences(email),
    subject: replySubject(email.subject, mode),
    to,
  };
};

/**
 * A saved draft back in the editor, or why it can't go: one too long to
 * edit would be saved back cut or flattened, over the original.
 */
export type ReopenedDraft =
  | { readonly ok: true; readonly init: ComposeInit }
  | { readonly ok: false; readonly reason: "too-large" };

/** A saved draft back in the editor, as it was. */
export const reopenDraft = (
  email: EmailDetail,
  toEditable: ToEditable = editableBody
): ReopenedDraft => {
  if (!isEditableBody(email)) {
    return { ok: false, reason: "too-large" };
  }
  const [from] = email.from;
  const init: ComposeInit = {
    attachments: [...email.attachments, ...inlineAttachments(email)],
    bcc: email.bcc.map(toRecipient),
    cc: email.cc.map(toRecipient),
    // A draft saved without HTML was written in plain-text mode.
    ...(email.html === null
      ? { plainText: true, text: email.text ?? "" }
      : {
          html: toEditable(
            email.html,
            email.text,
            email.inlineParts.map((part) => part.cid)
          ),
        }),
    restored: true,
    subject: email.subject,
    to: email.to.map(toRecipient),
    ...(from === undefined ? {} : { identityEmail: from.email }),
    ...(email.inReplyTo.length > 0 ? { inReplyTo: email.inReplyTo } : {}),
    ...(email.references.length > 0 ? { references: email.references } : {}),
  };
  return { init, ok: true };
};
