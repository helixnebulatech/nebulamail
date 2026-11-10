// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Which part of an email carries an invitation (RFC 6047 §2.4: a
// text/calendar part, or an attached .ics file). Apart from invitations.ts
// so the reader can tell whether to load the invitation and task cards
// without loading them.

import type { Attachment } from "../mail/api";

/** Bigger .ics parts are not parsed (Stalwart's own limit is 512 KB). */
export const MAX_INVITATION_BYTES = 512 * 1024;

const isCalendarPart = (attachment: Attachment): boolean =>
  attachment.type.toLowerCase() === "text/calendar";

const isIcsFile = (attachment: Attachment): boolean =>
  attachment.type.toLowerCase() === "application/ics" ||
  attachment.name.toLowerCase().endsWith(".ics");

/** The part to read: a text/calendar part first, else an .ics file. */
export const invitationPart = (
  attachments: readonly Attachment[]
): Attachment | null =>
  [
    ...attachments.filter(isCalendarPart),
    ...attachments.filter(isIcsFile),
  ].find((attachment) => attachment.size <= MAX_INVITATION_BYTES) ?? null;
