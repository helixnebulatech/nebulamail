// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Scheduled mail sent even after signing out: only on the bridge, when
// its session says `sendWhenSignedOut` (`[submission]
// send_while_signed_out`). Each scheduled submission carries the bridge's
// vendor property `sendWhenSignedOut`; while one waits, the bridge keeps
// the user's password (encrypted) after they sign out, until it is sent.
//
// The default for new scheduled messages is a per-device choice in
// Settings (off): it is not sensitive, just a preference. The «Programados»
// view shows and changes it per message.

import type { EmailSubmissionId, JmapClient } from "@nebulamail/jmap-client";
import {
  CAPABILITIES,
  SEND_WHEN_SIGNED_OUT,
  SEND_WHEN_SIGNED_OUT_PATCH,
  sendsWhenSignedOut,
} from "@nebulamail/jmap-client";

import { readLocalFlag } from "../lib/use-local-flag";
import type { ScheduledEmail } from "./schedule";

/** The local flag: new scheduled messages go out after sign-out too. */
export const SEND_SIGNED_OUT_FLAG = "wm-send-signed-out";

/** Whether the server lets scheduled messages go out after sign-out. */
export const canSendSignedOut = (client: JmapClient): boolean =>
  sendsWhenSignedOut(client.session);

/**
 * What a new scheduled submission adds to its create (the vendor property
 * with the setting's value), and whether the request must use the bridge
 * capability; nothing where the server doesn't have it.
 */
export const signedOutCreate = (
  client: JmapClient,
  on: boolean = readLocalFlag(SEND_SIGNED_OUT_FLAG, false)
): { readonly [SEND_WHEN_SIGNED_OUT]?: boolean } =>
  canSendSignedOut(client) ? { [SEND_WHEN_SIGNED_OUT]: on } : {};

/** Thrown when the server won't change it (sent meanwhile, or refused). */
export class SignedOutChangeError extends Error {
  readonly type: string;
  constructor(type: string, description?: string | null) {
    super(description ?? type);
    this.name = "SignedOutChangeError";
    this.type = type;
  }
}

/** Changes whether one pending submission goes out after sign-out. */
export const setSendWhenSignedOut = async (
  client: JmapClient,
  submissionId: EmailSubmissionId,
  on: boolean
): Promise<void> => {
  const accountId = client.primaryAccountId();
  const [result] = await client.request((b) => {
    b.using(CAPABILITIES.bridge);
    return [
      b.call("EmailSubmission/set", {
        accountId,
        update: { [submissionId]: { [SEND_WHEN_SIGNED_OUT_PATCH]: on } },
      }),
    ] as const;
  });
  if (result.updated[submissionId] !== undefined) {
    return;
  }
  const error = result.notUpdated[submissionId];
  throw new SignedOutChangeError(
    error?.type ?? "unknown",
    error?.description ?? null
  );
};

/**
 * The scheduled messages (`HOLDUNTIL`, not undo send's few seconds) that
 * would not go out if the user signed out now: what the logout warns about.
 */
export const waitingForSignIn = (
  list: readonly ScheduledEmail[]
): ScheduledEmail[] =>
  list.filter((item) => item.holdUntil && !item.sendWhenSignedOut);
