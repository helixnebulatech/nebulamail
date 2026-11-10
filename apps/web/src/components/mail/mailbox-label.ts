// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { cleanDisplayText } from "@nebulamail/mail-render";

import type { MailboxSummary } from "../../mail/api";
import { specialLabel } from "./sidebar";

/** Display name of a folder: special folders translated, others bidi-safe. */
export const mailboxLabel = (mailbox: MailboxSummary): string =>
  (mailbox.role === null ? null : specialLabel(mailbox.role)) ??
  cleanDisplayText(mailbox.name);
