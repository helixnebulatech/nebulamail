// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import type { EmailId } from "@nebulamail/jmap-client";

/**
 * A send that surely didn't go out (compose.ts `sendEmail`). `cause`: why.
 * `draftId`: the draft this attempt saved on the server, which the
 * composer keeps editing (null if none was created).
 */
export class SendFailedError extends Error {
  readonly draftId: EmailId | null;

  constructor(cause: unknown, draftId: EmailId | null) {
    super("The message was not sent", { cause });
    this.name = "SendFailedError";
    this.draftId = draftId;
  }
}
