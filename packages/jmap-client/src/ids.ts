// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Branded identifiers ("parse, don't validate"). Every schema is also the
// way to build a value: `EmailId.parse(raw)` checks the RFC 8620 §1.2 Id
// syntax once and the type system remembers it afterwards, so a MailboxId can
// never be passed where an EmailId is expected.

// Each id is declared twice on purpose: a zod schema (value) and its branded
// type, with the same name, so `EmailId.parse(x)` returns an `EmailId`.
// TypeScript keeps values and types in separate namespaces; the core
// no-redeclare rule doesn't know that.
/* oxlint-disable no-redeclare */

import { z } from "zod";

// RFC 8620 §1.2: 1-255 octets from the URL-safe base64 alphabet.
const idString = z
  .string()
  .min(1)
  .max(255)
  .regex(/^[\w-]+$/u, "Not a valid JMAP Id");

export const AccountId = idString.brand<"AccountId">();
export type AccountId = z.infer<typeof AccountId>;

export const BlobId = idString.brand<"BlobId">();
export type BlobId = z.infer<typeof BlobId>;

export const EmailId = idString.brand<"EmailId">();
export type EmailId = z.infer<typeof EmailId>;

export const EmailSubmissionId = idString.brand<"EmailSubmissionId">();
export type EmailSubmissionId = z.infer<typeof EmailSubmissionId>;

export const IdentityId = idString.brand<"IdentityId">();
export type IdentityId = z.infer<typeof IdentityId>;

export const MailboxId = idString.brand<"MailboxId">();
export type MailboxId = z.infer<typeof MailboxId>;

export const ThreadId = idString.brand<"ThreadId">();
export type ThreadId = z.infer<typeof ThreadId>;

export const ContactCardId = idString.brand<"ContactCardId">();
export type ContactCardId = z.infer<typeof ContactCardId>;

export const AddressBookId = idString.brand<"AddressBookId">();
export type AddressBookId = z.infer<typeof AddressBookId>;

export const QuotaId = idString.brand<"QuotaId">();
export type QuotaId = z.infer<typeof QuotaId>;

export const PrincipalId = idString.brand<"PrincipalId">();
export type PrincipalId = z.infer<typeof PrincipalId>;

export const VacationResponseId = z.literal("singleton");
export type VacationResponseId = z.infer<typeof VacationResponseId>;

// States are opaque strings chosen by the server (no syntax rules).
export const State = z.string().min(1).brand<"State">();
export type State = z.infer<typeof State>;

// Client-chosen keys in `create` maps; referenced elsewhere as `#key`.
export const CreationId = idString.brand<"CreationId">();
export type CreationId = z.infer<typeof CreationId>;

// JMAP for Calendars (calendars.ts). Occurrence ids of an expanded
// recurrence (`expandRecurrences`) are CalendarEventIds too.
export const CalendarId = idString.brand<"CalendarId">();
export type CalendarId = z.infer<typeof CalendarId>;

export const CalendarEventId = idString.brand<"CalendarEventId">();
export type CalendarEventId = z.infer<typeof CalendarEventId>;
