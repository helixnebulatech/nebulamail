// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Writing and sending: drafts, send with undo, identities.
//
// Undo send uses RFC 4865 FUTURERELEASE (`HOLDFOR`) when the server
// advertises it (Stalwart and the bridge do): the server holds the message and "undo"
// cancels the submission there, so closing the tab during the delay still
// sends it. Without FUTURERELEASE the message goes out at once and the UI
// doesn't offer undo. Checked against Stalwart: a held submission is
// `pending`, cancelling it stops delivery, and the email stays in Sent, so
// undo also moves it back to Drafts.
//
// A send has three outcomes (`SendOutcome`), and the UI can't mix them up:
// - "sent": the submission exists and the email was filed in Sent;
// - "sentNotFiled": the submission exists but the implicit Email/set that
//   files it failed (it's still a draft). It is not patched again: the
//   bridge already adds the copy to Sent itself when that patch fails
// so a second patch would make two copies;
// - "unknown": the server didn't confirm (bridge `outcome_unknown`: no
//   `created.send`, but the EmailSubmission row stays with an empty SMTP
//   reply), the answer was lost after the request went out (network, 5xx,
//   not JSON), or we couldn't check. Never offered to send again.
// Only `SendFailedError` means nothing went out. The email id is always
// the one of the draft created in the same request, never taken from the
// implicit Email/set.
//
// Scheduled send (schedule.ts) is the same request with `HOLDUNTIL`
// instead of `HOLDFOR`: the email leaves Drafts for Sent at once, like
// undo send, and cancelling puts it back.

import type {
  EmailAddress,
  EmailBodyPartCreate,
  EmailCreate,
  EmailId,
  EmailSubmissionId,
  IdentityId,
  JmapClient,
  MailboxId,
} from "@nebulamail/jmap-client";
import {
  CAPABILITIES,
  EmailId as EmailIdSchema,
  JmapError,
  submissionCapability,
} from "@nebulamail/jmap-client";
import { withBodyStyle } from "@nebulamail/mail-render";
import type { ComposeBodyStyle } from "@nebulamail/mail-render";

import type { DraftAttachment } from "./attachments";
import { attachmentsInUse, editableBody, htmlToText } from "./body";
import { getClient } from "./client";
import { submissionsOf } from "./delivery";
import type { MailboxRoles } from "./mailbox-roles";
import {
  freshRolesIfChanged,
  mailboxRoles,
  mayBeStaleMailbox,
  roleId,
} from "./mailbox-roles";
import { DraftRefusedError, draftRefusal } from "./refusal";
import type { ScheduledEmail } from "./schedule";
import {
  ScheduleError,
  cancelPending,
  checkSendAt,
  hasPending,
  holdUntil,
  learnMaximum,
  limitFor,
  maximumInRefusal,
  reschedule,
  scheduleFailure,
} from "./schedule";
import { SendFailedError } from "./send-failed";
import { signedOutCreate } from "./send-signed-out";

export interface Identity {
  readonly id: IdentityId;
  readonly name: string;
  readonly email: string;
  /** Sanitized HTML for the editor (from htmlSignature or textSignature). */
  readonly signature: string | null;
  /**
   * The identity's `textSignature`, for plain-text messages (without it,
   * the HTML one converted, body.ts `plainSignature`).
   */
  readonly textSignature?: string | null;
  /**
   * RFC 8621 §6: the Reply-To to set on emails from this identity (empty:
   * none). Missing in copies saved before 2026-11-08.
   */
  readonly replyTo?: readonly Recipient[];
  /**
   * RFC 8621 §6: the Bcc to add to new emails from this identity; the
   * user can remove it (recipients.ts `swapIdentityBcc`).
   */
  readonly bcc?: readonly Recipient[];
}

export interface Recipient {
  readonly name?: string;
  readonly email: string;
}

export interface Draft {
  /**
   * Who sends it. null: the identities aren't known yet (the app opened
   * without a connection and none were saved on this device); the draft
   * stays on the device until they are.
   */
  readonly identity: Identity | null;
  readonly to: readonly Recipient[];
  readonly cc: readonly Recipient[];
  readonly bcc: readonly Recipient[];
  readonly subject: string;
  /** The editor's HTML; the text/plain alternative is generated from it. */
  readonly html: string;
  /**
   * Plain-text mode: the body as typed. The message then has only a
   * text/plain part (no HTML, no inline images) and `html` is ignored.
   */
  readonly plainText?: string;
  /**
   * The default font and size the HTML is sent in (Settings › Writing):
   * the HTML part is wrapped in a `<div>` with them.
   */
  readonly bodyStyle?: ComposeBodyStyle;
  /** Uploaded attachments (see attachments.ts). */
  readonly attachments: readonly DraftAttachment[];
  /**
   * Ask for a read receipt (`Disposition-Notification-To`, RFC 8098) to
   * the sender's address. Off unless the user turns it on.
   */
  readonly readReceipt?: boolean;
  /** Replies: Message-IDs of the email being answered (RFC 5322 threading). */
  readonly inReplyTo?: readonly string[];
  readonly references?: readonly string[];
}

export interface SentEmail {
  readonly emailId: EmailId;
  readonly submissionId: EmailSubmissionId;
  /** When the server will release it; undo is possible until then. */
  readonly sendAt: string;
  readonly undoable: boolean;
  /** Scheduled for later (`HOLDUNTIL`), not just held for undo. */
  readonly scheduled?: true;
}

export type SendOutcome =
  /** Sent and filed in Sent. */
  | { readonly kind: "sent"; readonly sent: SentEmail }
  /** Sent (the submission exists), but not filed: still a draft in Drafts. */
  | { readonly kind: "sentNotFiled"; readonly sent: SentEmail }
  /**
   * The server didn't confirm it, or the answer was lost: it may have gone
   * out. Never offer to send it again. `emailId`: the draft on the server,
   * null if we can't tell which one (or whether there is one).
   */
  | { readonly kind: "unknown"; readonly emailId: EmailId | null };

/** Undo window offered by default (5–30 s, configurable). */
export const DEFAULT_UNDO_SECONDS = 10;

// A pragmatic check (RFC 5322 is far looser): one @, no spaces or angle
// brackets, a dot inside the domain. The server validates for real.
// Done in pieces, not one regex: `x+\.x+` with a class that holds the dot
// backtracks quadratically, and a hostile `mailto:` with 80,000 dots hung
// the tab (like CVE-2023-45150 in Nextcloud Mail).
const PART = /^[^\s@<>()",;:]+$/u;
/** RFC 5321 §4.5.3.1.3: a path is at most 256 octets with its `<>`. */
const MAX_EMAIL = 254;

export const isValidEmail = (email: string): boolean => {
  const trimmed = email.trim();
  if (trimmed.length > MAX_EMAIL) {
    return false;
  }
  const at = trimmed.indexOf("@");
  const local = trimmed.slice(0, at);
  const domain = trimmed.slice(at + 1);
  return (
    at > 0 &&
    PART.test(local) &&
    PART.test(domain) &&
    domain.slice(1, -1).includes(".")
  );
};

const toAddresses = (recipients: readonly Recipient[]): EmailAddress[] =>
  recipients.map((recipient) => ({
    email: recipient.email.trim(),
    name:
      recipient.name?.trim() === "" ? null : (recipient.name?.trim() ?? null),
  }));

export const allRecipients = (draft: Draft): readonly Recipient[] => [
  ...draft.to,
  ...draft.cc,
  ...draft.bcc,
];

/** A draft that knows who sends it: it can go to the server. */
export type ReadyDraft = Draft & { readonly identity: Identity };

export const isReady = (draft: Draft): draft is ReadyDraft =>
  draft.identity !== null;

/** Problems that block sending, as stable codes for the UI to translate. */
export type DraftProblem =
  | "no-recipients"
  | "invalid-recipient"
  | "no-identity";

export const checkDraft = (draft: Draft): DraftProblem | null => {
  if (draft.identity === null) {
    return "no-identity";
  }
  const recipients = allRecipients(draft);
  if (recipients.length === 0) {
    return "no-recipients";
  }
  return recipients.every((recipient) => isValidEmail(recipient.email))
    ? null
    : "invalid-recipient";
};

const fileParts = (
  attachments: readonly DraftAttachment[]
): EmailBodyPartCreate[] =>
  attachments.map((attachment) => ({
    blobId: attachment.blobId,
    disposition: "attachment",
    name: attachment.name,
    type: attachment.type,
  }));

/**
 * The body parts. Without inline images the server builds the MIME tree
 * (multipart/alternative, then mixed with the files). With them, the tree
 * is given (`bodyStructure`): mixed[related[alternative[text, html],
 * images by Content-ID], files], so clients show them in place. Stalwart
 * left to itself puts them in multipart/mixed (checked 2026-10-24).
 */
const bodyParts = (
  attachments: readonly DraftAttachment[],
  plain: boolean
): Pick<
  EmailCreate,
  "attachments" | "bodyStructure" | "htmlBody" | "textBody"
> => {
  const files = attachments.filter(
    (attachment) => attachment.cid === undefined
  );
  if (plain) {
    // Only text/plain: no HTML part, so no inline images either.
    return {
      textBody: [{ partId: "text", type: "text/plain" }],
      ...(files.length === 0 ? {} : { attachments: fileParts(files) }),
    };
  }
  const inline = attachments.flatMap((attachment) =>
    attachment.cid === undefined
      ? []
      : [
          {
            blobId: attachment.blobId,
            cid: attachment.cid,
            disposition: "inline",
            name: attachment.name,
            type: attachment.type,
          },
        ]
  );
  if (inline.length === 0) {
    return {
      htmlBody: [{ partId: "html", type: "text/html" }],
      textBody: [{ partId: "text", type: "text/plain" }],
      ...(files.length === 0 ? {} : { attachments: fileParts(files) }),
    };
  }
  const related: EmailBodyPartCreate = {
    subParts: [
      {
        subParts: [
          { partId: "text", type: "text/plain" },
          { partId: "html", type: "text/html" },
        ],
        type: "multipart/alternative",
      },
      ...inline,
    ],
    type: "multipart/related",
  };
  return {
    bodyStructure:
      files.length === 0
        ? related
        : { subParts: [related, ...fileParts(files)], type: "multipart/mixed" },
  };
};

/**
 * The JMAP Email for a draft: HTML (in the draft's default font and size)
 * plus its plain-text alternative and the attachments (bodyParts); in
 * plain-text mode only the text. `text` is `draftText(draft)`, passed in
 * so this stays pure. Empty header fields are left out: Stalwart rejects
 * `null` for them on create ("invalidProperties: bcc").
 */
export const draftEmail = (
  draft: ReadyDraft,
  draftsId: MailboxId,
  text: string
): EmailCreate => ({
  ...bodyParts(draft.attachments, draft.plainText !== undefined),
  bodyValues:
    draft.plainText === undefined
      ? {
          html: {
            value:
              draft.bodyStyle === undefined
                ? draft.html
                : withBodyStyle(draft.html, draft.bodyStyle),
          },
          text: { value: text },
        }
      : { text: { value: text } },
  from: [{ email: draft.identity.email, name: draft.identity.name || null }],
  keywords: { $draft: true, $seen: true },
  mailboxIds: { [draftsId]: true },
  subject: draft.subject,
  to: toAddresses(draft.to),
  ...(draft.cc.length > 0 ? { cc: toAddresses(draft.cc) } : {}),
  ...(draft.bcc.length > 0 ? { bcc: toAddresses(draft.bcc) } : {}),
  // RFC 8621 §6: the identity's Reply-To (its Bcc is in `draft.bcc`).
  ...((draft.identity.replyTo ?? []).length > 0
    ? { replyTo: toAddresses(draft.identity.replyTo ?? []) }
    : {}),
  ...(draft.inReplyTo === undefined || draft.inReplyTo.length === 0
    ? {}
    : { inReplyTo: draft.inReplyTo }),
  ...(draft.references === undefined || draft.references.length === 0
    ? {}
    : { references: draft.references }),
  ...(draft.readReceipt === true
    ? {
        "header:Disposition-Notification-To:asAddresses": [
          {
            email: draft.identity.email,
            name: draft.identity.name || null,
          },
        ],
      }
    : {}),
});

const withClient = async <T>(
  run: (
    client: JmapClient,
    accountId: ReturnType<JmapClient["primaryAccountId"]>
  ) => Promise<T>
): Promise<T> => {
  const client = await getClient();
  return await run(client, client.primaryAccountId());
};

/** The identity's signature for the editor, sanitized. */
export const signatureHtml = (
  html: string | undefined,
  text: string | undefined
): string | null => {
  if ((html ?? "").trim() === "" && (text ?? "").trim() === "") {
    return null;
  }
  return editableBody(html ?? null, text ?? null);
};

/** An identity's address list (null or missing: none) as recipients. */
const fromAddresses = (
  addresses: readonly EmailAddress[] | null | undefined
): Recipient[] =>
  (addresses ?? []).flatMap((address) => {
    const email = address.email.trim();
    const name = (address.name ?? "").trim();
    return email === "" ? [] : [name === "" ? { email } : { email, name }];
  });

export const listIdentities = async (): Promise<Identity[]> =>
  await withClient(async (client, accountId) => {
    const [result] = await client.request((b) => [
      b.call("Identity/get", {
        accountId,
        properties: [
          "name",
          "email",
          "htmlSignature",
          "textSignature",
          "replyTo",
          "bcc",
        ],
      }),
    ]);
    return result.list.map((identity) => ({
      // Stalwart and the bridge write null when there are none.
      bcc: fromAddresses(identity.bcc),
      email: identity.email,
      id: identity.id,
      name: identity.name,
      replyTo: fromAddresses(identity.replyTo),
      signature: signatureHtml(identity.htmlSignature, identity.textSignature),
      textSignature:
        (identity.textSignature ?? "").trim() === ""
          ? null
          : (identity.textSignature ?? null),
    }));
  });

/** Without identities the server can't take the draft (see Draft). */
export class NoIdentityError extends Error {
  override readonly name = "NoIdentityError";
}

/**
 * The text/plain part: what was typed in plain-text mode (with the final
 * line break of a text file), else generated from the HTML.
 */
export const draftText = (draft: Draft): string => {
  if (draft.plainText === undefined) {
    return htmlToText(draft.html);
  }
  const text = draft.plainText.replaceAll("\r\n", "\n").trimEnd();
  return text === "" ? "" : `${text}\n`;
};

/** The draft as it goes to the server: inline images only if still shown. */
const forServer = (draft: Draft): ReadyDraft => {
  if (!isReady(draft)) {
    throw new NoIdentityError("The identities are not known yet");
  }
  return {
    ...draft,
    // Plain text shows no image: none of the inline ones go.
    attachments: attachmentsInUse(
      draft.attachments,
      draft.plainText === undefined ? draft.html : ""
    ),
  };
};

/**
 * Saves a draft. JMAP emails are immutable, so each save creates a new one
 * and destroys the previous version (`previous`). Returns the new id.
 */
export const saveDraft = async (
  draft: Draft,
  previous?: EmailId
): Promise<EmailId> => {
  const ready = forServer(draft);
  return await withClient(async (client, accountId) => {
    const save = async (roles: MailboxRoles) => {
      const draftsId = roleId(roles, "drafts");
      const [answer] = await client.request((b) => [
        b.call("Email/set", {
          accountId,
          create: { draft: draftEmail(ready, draftsId, draftText(ready)) },
          ...(previous === undefined ? {} : { destroy: [previous] }),
        }),
      ]);
      return answer;
    };
    const roles = await mailboxRoles(client, accountId);
    let result = await save(roles);
    if (
      (result.created?.draft ?? null) === null &&
      mayBeStaleMailbox(result.notCreated?.draft)
    ) {
      // Drafts may have been replaced since we remembered it: retry once.
      // A failed lookup leaves the server's refusal as the answer.
      const fresh = await freshRolesIfChanged(client, accountId, roles).catch(
        () => null
      );
      if (fresh !== null) {
        result = await save(fresh);
      }
    }
    const created = result.created?.draft;
    if (created === undefined || created === null) {
      throw (
        draftRefusal(result.notCreated) ??
        new Error("The server did not save the draft")
      );
    }
    return EmailIdSchema.parse(created.id);
  });
};

/** Deletes a saved draft (the composer's "Discard"). */
export const discardDraft = async (id: EmailId): Promise<void> => {
  await withClient(async (client, accountId) => {
    await client.request((b) => [
      b.call("Email/set", { accountId, destroy: [id] }),
    ]);
  });
};

/** Seconds the server can hold a message (FUTURERELEASE), or 0. */
const maxHoldSeconds = (client: JmapClient): number => {
  const capability = submissionCapability(
    client.session,
    client.primaryAccountId()
  );
  const supported =
    capability?.submissionExtensions?.FUTURERELEASE !== undefined;
  return supported ? (capability?.maxDelayedSend ?? 0) : 0;
};

/**
 * The `mailFrom` parameters: HOLDUNTIL for a scheduled send (after
 * checking the time against the server's limit), else HOLDFOR for undo
 * when the server can hold, else none.
 */
const holdParameters = (
  client: JmapClient,
  undoSeconds: number,
  sendAt: Date | undefined
): Readonly<Record<string, string>> | null => {
  if (sendAt !== undefined) {
    const limit = limitFor(client);
    const problem =
      limit === null ? "invalid" : checkSendAt(sendAt, new Date(), limit);
    if (problem !== null) {
      throw new ScheduleError({ kind: "time", problem });
    }
    return { HOLDUNTIL: holdUntil(sendAt) };
  }
  const hold = Math.min(
    Math.max(0, Math.floor(undoSeconds)),
    maxHoldSeconds(client)
  );
  return hold > 0 ? { HOLDFOR: String(hold) } : null;
};

/**
 * Errors of an EmailSubmission/set method call that say the server did
 * nothing (RFC 8620 §3.6.2, RFC 8621 §7.5, Stalwart's limits).
 */
const NOTHING_DONE = new Set([
  "accountNotFound",
  "accountNotSupportedByMethod",
  "accountReadOnly",
  "forbidden",
  "invalidArguments",
  "rateLimit",
  "requestTooLarge",
  "tooLarge",
  "unknownMethod",
]);

/**
 * Whether a failed request may still have run on the server: the answer
 * was lost or broken after it went out. A 4xx (or a problem document under
 * 500) means the server refused the whole request.
 */
const mayHaveRun = (error: unknown): boolean => {
  if (!(error instanceof JmapError)) {
    return true;
  }
  const { detail } = error;
  switch (detail.kind) {
    case "network":
    case "aborted":
    case "invalidResponse": {
      return true;
    }
    case "http":
    case "request": {
      return detail.status >= 500;
    }
    // Before the request went out, or refused before running it.
    case "auth":
    case "unauthorized":
    case "limit":
    case "usage":
    case "invalidUrl":
    case "crossOrigin":
    case "method": {
      return false;
    }
    default: {
      return detail satisfies never;
    }
  }
};

const unknown = (emailId: EmailId | null): SendOutcome => ({
  emailId,
  kind: "unknown",
});

interface SendRequest {
  readonly client: JmapClient;
  readonly accountId: ReturnType<JmapClient["primaryAccountId"]>;
  readonly ready: ReadyDraft;
  readonly previousDraft: EmailId | undefined;
  readonly draftsId: MailboxId;
  readonly sentId: MailboxId;
  /** `mailFrom` parameters: HOLDFOR (undo) or HOLDUNTIL (scheduled). */
  readonly hold: Readonly<Record<string, string>> | null;
  /**
   * A scheduled send on the bridge: the vendor property that says whether
   * it goes out after sign-out (send-signed-out.ts); empty otherwise.
   */
  readonly signedOut: ReturnType<typeof signedOutCreate>;
}

/**
 * The draft and its submission in one request. Throws `SendFailedError`
 * if the request failed before going out or was refused as a whole; null
 * if it went out and the answer was lost (it may have been sent).
 */
const sendRequest = async ({
  accountId,
  client,
  draftsId,
  hold,
  previousDraft,
  ready,
  sentId,
  signedOut,
}: SendRequest) => {
  let requested = false;
  try {
    return await client.requestSettled(
      (b) => {
        if (Object.keys(signedOut).length > 0) {
          b.using(CAPABILITIES.bridge);
        }
        const email = b.call("Email/set", {
          accountId,
          create: { draft: draftEmail(ready, draftsId, draftText(ready)) },
          ...(previousDraft === undefined ? {} : { destroy: [previousDraft] }),
        });
        const submit = b.call("EmailSubmission/set", {
          accountId,
          create: {
            send: {
              ...signedOut,
              emailId: "#draft",
              // The envelope is explicit so Bcc recipients get it and the
              // HOLDFOR or HOLDUNTIL parameter can be set.
              envelope: {
                mailFrom: {
                  email: ready.identity.email,
                  parameters: hold,
                },
                rcptTo: allRecipients(ready).map((recipient) => ({
                  email: recipient.email.trim(),
                })),
              },
              identityId: ready.identity.id,
            },
          },
          onSuccessUpdateEmail: {
            "#send": {
              "keywords/$draft": null,
              [`mailboxIds/${draftsId}`]: null,
              [`mailboxIds/${sentId}`]: true,
            },
          },
        });
        return [email, submit] as const;
      },
      {
        onSend: () => {
          requested = true;
        },
      }
    );
  } catch (error) {
    // Before it went out (session refresh, size limit, token) or refused
    // as a whole (4xx): nothing was done. Otherwise we can't know.
    if (!requested || !mayHaveRun(error)) {
      throw new SendFailedError(error, null);
    }
    return null;
  }
};

type SendResults = NonNullable<Awaited<ReturnType<typeof sendRequest>>>;

/**
 * The draft this request created; null if we can't tell. Throws when the
 * server surely created nothing (then "#draft" couldn't be sent either).
 */
const createdDraftId = (saved: SendResults[0]): EmailId | null => {
  if (!saved.ok) {
    // A method error: Email/set did nothing. Anything else (an answer
    // that doesn't parse, none at all) may hide a draft that was sent.
    if (saved.error.detail.kind === "method") {
      throw new SendFailedError(saved.error, null);
    }
    return null;
  }
  const refused = draftRefusal(saved.value.notCreated);
  if (refused !== null) {
    throw new SendFailedError(refused, null);
  }
  const created = saved.value.created?.draft;
  return created === undefined || created === null
    ? null
    : EmailIdSchema.parse(created.id);
};

/**
 * The draft was refused, maybe for stale mailbox ids, and so nothing was
 * submitted (the submission's "#draft" reference had nothing to point to,
 * RFC 8620 §5.3).
 */
const refusedForMailbox = ([saved, submission]: SendResults): boolean =>
  saved.ok &&
  (saved.value.created?.draft ?? null) === null &&
  mayBeStaleMailbox(saved.value.notCreated?.draft) &&
  (!submission.ok || (submission.value.created?.send ?? null) === null);

/** Whether the implicit Email/set filed the draft in Sent. */
const wasFiled = (
  submission: Extract<SendResults[1], { ok: true }>["value"],
  draftId: EmailId
): boolean => {
  const { emailSet, emailSetFailed } = submission;
  // No implicit answer at all (RFC 8621 requires one): not filed.
  const updated = emailSet?.updated;
  return (
    emailSetFailed !== true &&
    updated !== undefined &&
    updated !== null &&
    Object.hasOwn(updated, draftId)
  );
};

/**
 * No submission was created: did it surely fail? Asks the server for
 * submissions of the draft (created by this very request, so any is this
 * attempt's: the bridge keeps the row, with an empty SMTP reply, when the
 * outcome is unknown, and forgets it when the send surely failed).
 */
const notSentOutcome = async (
  client: JmapClient,
  accountId: ReturnType<JmapClient["primaryAccountId"]>,
  submission: SendResults[1],
  draftId: EmailId
): Promise<SendOutcome> => {
  let cause: unknown = new Error("The server did not accept the message");
  if (submission.ok) {
    const refusal = submission.value.notCreated?.send;
    if (refusal !== undefined) {
      cause = new DraftRefusedError(refusal.type, refusal.description);
      // A scheduled time past the server's real limit (see schedule.ts).
      const maximum = maximumInRefusal(refusal.description);
      if (maximum !== null) {
        learnMaximum(accountId, maximum);
      }
    }
  } else {
    const { detail } = submission.error;
    if (detail.kind === "method" && NOTHING_DONE.has(detail.type)) {
      throw new SendFailedError(submission.error, draftId);
    }
    cause = submission.error;
  }
  try {
    const found = await submissionsOf(client, accountId, [draftId]);
    if (found.some((row) => row.undoStatus !== "canceled")) {
      return unknown(draftId);
    }
  } catch {
    // Can't check: never invite to send it again without knowing.
    return unknown(draftId);
  }
  throw new SendFailedError(cause, draftId);
};

/**
 * Sends a draft: saves it (replacing `previousDraft`) and submits it in
 * one request. Throws `SendFailedError` only when nothing went out; see
 * the top of this file for the outcomes. If the new draft was created but
 * `previousDraft` couldn't be destroyed (`notDestroyed`), both stay in
 * Drafts: rare, and better than losing one.
 */
export const sendEmail = async (
  draft: Draft,
  {
    previousDraft,
    sendAt,
    undoSeconds = DEFAULT_UNDO_SECONDS,
  }: {
    readonly previousDraft?: EmailId;
    readonly undoSeconds?: number;
    /**
     * Send later (scheduled): the server holds it until then. Checked
     * against the server's limit first (`ScheduleError`, kind "time").
     */
    readonly sendAt?: Date;
  } = {}
): Promise<SendOutcome> => {
  const problem = checkDraft(draft);
  if (problem !== null) {
    throw new Error(`Draft can't be sent: ${problem}`);
  }
  const ready = forServer(draft);
  return await withClient(async (client, accountId) => {
    // Checked first: a bad time needs no round trip.
    const hold = holdParameters(client, undoSeconds, sendAt);
    const send = async (roles: MailboxRoles) =>
      await sendRequest({
        accountId,
        client,
        draftsId: roleId(roles, "drafts"),
        hold,
        previousDraft,
        ready,
        sentId: roleId(roles, "sent"),
        // Only a scheduled send: undo send's few seconds don't need it.
        signedOut: sendAt === undefined ? {} : signedOutCreate(client),
      });
    const roles = await mailboxRoles(client, accountId);
    let results = await send(roles);
    if (results !== null && refusedForMailbox(results)) {
      // Nothing went out (no draft, so "#draft" couldn't be submitted):
      // with Drafts or Sent replaced since we remembered them, retry once.
      const fresh = await freshRolesIfChanged(client, accountId, roles).catch(
        () => null
      );
      if (fresh !== null) {
        results = await send(fresh);
      }
    }
    if (results === null) {
      return unknown(null);
    }
    const [saved, submission] = results;
    const draftId = createdDraftId(saved);
    const created = submission.ok ? submission.value.created?.send : undefined;
    if (created === undefined || created === null) {
      return draftId === null
        ? unknown(null)
        : await notSentOutcome(client, accountId, submission, draftId);
    }
    if (draftId === null || !submission.ok) {
      return unknown(null);
    }
    const sent: SentEmail = {
      emailId: draftId,
      sendAt: created.sendAt ?? new Date().toISOString(),
      submissionId: created.id,
      undoable: hold !== null,
      ...(sendAt === undefined ? {} : { scheduled: true as const }),
    };
    return {
      kind: wasFiled(submission.value, draftId) ? "sent" : "sentNotFiled",
      sent,
    };
  });
};

/** Puts a sent (cancelled) email back in Drafts, editable again. */
const returnToDrafts = async (
  client: JmapClient,
  accountId: ReturnType<JmapClient["primaryAccountId"]>,
  emailId: EmailId,
  known?: Promise<MailboxRoles>
): Promise<void> => {
  const move = async (roles: MailboxRoles) => {
    const [answer] = await client.request((b) => [
      b.call("Email/set", {
        accountId,
        update: {
          [emailId]: {
            "keywords/$draft": true,
            [`mailboxIds/${roleId(roles, "drafts")}`]: true,
            [`mailboxIds/${roleId(roles, "sent")}`]: null,
          },
        },
      }),
    ]);
    return answer;
  };
  const roles = await (known ?? mailboxRoles(client, accountId));
  const result = await move(roles);
  if (mayBeStaleMailbox(result.notUpdated?.[emailId])) {
    const fresh = await freshRolesIfChanged(client, accountId, roles).catch(
      () => null
    );
    if (fresh !== null) {
      await move(fresh);
    }
  }
};

/**
 * Cancels a held message (undo send, or a scheduled one) and puts it back
 * in Drafts, editable again.
 */
export const undoSend = async (
  sent: Pick<SentEmail, "emailId" | "submissionId">
): Promise<void> => {
  await withClient(async (client, accountId) => {
    // The mailboxes are looked up while the cancel runs. The Email/set
    // that moves it waits for the cancel, never in the same request: if
    // it was already sent, the email must stay in Sent.
    const roles = mailboxRoles(client, accountId);
    // Not left unhandled when the cancel throws first.
    roles.catch(() => null);
    // ScheduleError "alreadySent" or "alreadyCanceled" if it can't.
    await cancelPending(client, sent.submissionId);
    await returnToDrafts(client, accountId, sent.emailId, roles);
  });
};

/**
 * A scheduled message at another time (schedule.ts `reschedule`). If the
 * new submission fails after the old one was cancelled, the email goes
 * back to Drafts (so it is never lost nor sent twice) and the
 * `ScheduleError` (kind "refused" or "interrupted") is thrown. After an
 * "interrupted" one the new submission may still have been made (the
 * answer was lost): then it stays in Sent, scheduled.
 */
export const rescheduleEmail = async (
  scheduled: ScheduledEmail,
  at: Date
): Promise<void> => {
  try {
    await reschedule(scheduled, at);
  } catch (error) {
    const kind = scheduleFailure(error)?.kind;
    if (kind !== "refused" && kind !== "interrupted") {
      throw error;
    }
    await withClient(async (client, accountId) => {
      if (
        kind === "interrupted" &&
        (await hasPending(client, scheduled.emailId))
      ) {
        return;
      }
      await returnToDrafts(client, accountId, scheduled.emailId);
    });
    throw error;
  }
};
