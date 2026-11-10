// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Web Push subscriptions (RFC 8620 §7.2; VAPID: RFC 8292 and
// draft-ietf-jmap-webpush-vapid). The server POSTs push objects to `url`
// (encrypted with `keys`, RFC 8291): first a PushVerification whose code
// the client sends back, then StateChange (and, for calendars,
// CalendarAlert) objects for `types`. PushSubscription has no accountId:
// it belongs to the credentials. Checked against Stalwart 1.0 (2026-10-26).

import { z } from "zod";

import { CAPABILITIES } from "./capabilities";
import { parseOrThrow } from "./errors";
import { AccountId, CalendarEventId } from "./ids";
import { getResponseSchema, setResponseSchema } from "./standard";

/** Session capability with the server's VAPID public key. */
export const WEBPUSH_VAPID = "urn:ietf:params:jmap:webpush-vapid";

/**
 * Stalwart 1.0: delivered mail inside the push (`emailPush` of a
 * subscription; an `EmailPush` object with the emails instead of the
 * StateChange of that delivery). Session and account capability.
 */
export const EMAIL_PUSH = "urn:ietf:params:jmap:emailpush";

/**
 * Per account (`emailPush` of a subscription, Stalwart 1.0): which Email
 * properties go in the push, for which emails (an `Email/query` filter)
 * and its Web Push `Urgency`. What doesn't fit in one push (4 KiB) is
 * left out, property by property.
 */
export interface EmailPushConfig {
  readonly properties: readonly string[];
  readonly filter?: Readonly<Record<string, unknown>>;
  readonly urgency?: "very-low" | "low" | "normal" | "high";
}

const PushSubscriptionId = z.string().min(1).max(255);

export const PushSubscriptionSchema = z.looseObject({
  deviceClientId: z.string(),
  expires: z.string().nullish(),
  id: PushSubscriptionId,
  /** Type names (null: all). */
  types: z.array(z.string()).nullish(),
  /** Never returned by the server (RFC 8620 §7.2.1): kept for typing. */
  url: z.string().nullish(),
  verificationCode: z.string().nullish(),
});
export type PushSubscription = z.infer<typeof PushSubscriptionSchema>;

export interface PushSubscriptionCreate {
  /** Stable per device and app; lets the client find its subscription. */
  readonly deviceClientId: string;
  /** The browser's push endpoint (PushSubscription.endpoint). */
  readonly url: string;
  /** RFC 8291 keys, base64url (PushSubscription.getKey). */
  readonly keys: { readonly p256dh: string; readonly auth: string } | null;
  readonly expires?: string | null;
  readonly types?: readonly string[] | null;
  /** By account id (`EMAIL_PUSH` servers only). */
  readonly emailPush?: Readonly<Record<string, EmailPushConfig>> | null;
}

export interface PushSubscriptionGetArgs {
  readonly ids?: readonly string[] | null;
  readonly properties?: readonly string[];
}

export interface PushSubscriptionSetArgs {
  readonly create?: Readonly<Record<string, PushSubscriptionCreate>>;
  readonly update?: Readonly<
    Record<
      string,
      {
        readonly verificationCode?: string;
        readonly types?: readonly string[] | null;
        readonly expires?: string | null;
      }
    >
  >;
  readonly destroy?: readonly string[];
}

/** Sent to the push URL right after creating a subscription. */
export const PushVerificationSchema = z.looseObject({
  "@type": z.literal("PushVerification"),
  pushSubscriptionId: z.string(),
  verificationCode: z.string(),
});
export type PushVerification = z.infer<typeof PushVerificationSchema>;

/**
 * A display alert of an event is due (draft-ietf-jmap-calendars §9):
 * over EventSource (`event: calendarAlert`) or Web Push, when `types`
 * include "CalendarAlert". `recurrenceId` names the occurrence (Stalwart:
 * an RFC 3339 UTC time).
 */
export const CalendarAlertSchema = z.looseObject({
  "@type": z.literal("CalendarAlert"),
  accountId: AccountId,
  alertId: z.string().min(1).max(255),
  calendarEventId: CalendarEventId,
  recurrenceId: z.string().nullish(),
  uid: z.string().nullish(),
});
export type CalendarAlert = z.infer<typeof CalendarAlertSchema>;

interface Spec<A, R> {
  readonly args: A;
  readonly response: R;
}

type PushGetResponse = z.infer<typeof GetSchema>;
type PushSetResponse = z.infer<typeof SetSchema>;

/** Merged into `MethodMap` (methods.ts). */
export interface PushSubscriptionMethodMap {
  "PushSubscription/get": Spec<PushSubscriptionGetArgs, PushGetResponse>;
  "PushSubscription/set": Spec<PushSubscriptionSetArgs, PushSetResponse>;
}

const nullable = <T extends z.ZodType>(schema: T) =>
  schema.nullish().transform((value) => value ?? null);

// No accountId or state for this type (RFC 8620 §7.2).
const GetSchema = getResponseSchema(PushSubscriptionSchema, PushSubscriptionId)
  .omit({ accountId: true, state: true })
  .extend({ state: nullable(z.string()).optional() });

const SetSchema = setResponseSchema(
  PushSubscriptionSchema.partial().extend({ id: PushSubscriptionId }),
  PushSubscriptionId,
  PushSubscriptionSchema.partial()
)
  .omit({ accountId: true, newState: true, oldState: true })
  .extend({
    newState: nullable(z.string()).optional(),
    oldState: nullable(z.string()).optional(),
  });

/** Merged into `METHODS` (methods.ts). */
export const PUSH_SUBSCRIPTION_METHODS: {
  readonly [M in keyof PushSubscriptionMethodMap]: {
    readonly using: readonly string[];
    readonly parse: (
      args: Readonly<Record<string, unknown>>,
      raw: Record<string, unknown>
    ) => PushSubscriptionMethodMap[M]["response"];
  };
} = {
  "PushSubscription/get": {
    parse: (_args, raw) => parseOrThrow(GetSchema, raw),
    using: [CAPABILITIES.core],
  },
  "PushSubscription/set": {
    parse: (_args, raw) => parseOrThrow(SetSchema, raw),
    using: [CAPABILITIES.core],
  },
};
