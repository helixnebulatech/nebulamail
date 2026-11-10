// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Capability URNs. The UI shows or hides features depending on which ones the
// session advertises (see `hasCapability`).

export const CAPABILITIES = {
  core: "urn:ietf:params:jmap:core",
  mail: "urn:ietf:params:jmap:mail",
  submission: "urn:ietf:params:jmap:submission",
  vacationResponse: "urn:ietf:params:jmap:vacationresponse",
  calendars: "urn:ietf:params:jmap:calendars",
  contacts: "urn:ietf:params:jmap:contacts",
  contactsParse: "urn:ietf:params:jmap:contacts:parse",
  sieve: "urn:ietf:params:jmap:sieve",
  quota: "urn:ietf:params:jmap:quota",
  blob: "urn:ietf:params:jmap:blob",
  fileNode: "urn:ietf:params:jmap:filenode",
  websocket: "urn:ietf:params:jmap:websocket",
  principals: "urn:ietf:params:jmap:principals",
  mailShare: "urn:ietf:params:jmap:mail:share",
  /** RFC 9007: `MDN/send` and `MDN/parse`. */
  mdn: "urn:ietf:params:jmap:mdn",
  /**
   * Vendor capability (RFC 8620 §2) of the Nebula Mail bridge: the server
   * is the bridge (bridge/). Its methods:
   * `CalendarEvent/counter` and `CalendarEvent/declineCounter` (calendars.ts).
   * Its session value is `{}`, or `{"sendWhenSignedOut": true}` when held
   * messages may go out after sign-out (`sendsWhenSignedOut`).
   */
  bridge: "https://helixnebula.tech/jmap/bridge",
} as const;

/**
 * Vendor property of `EmailSubmission` (with `CAPABILITIES.bridge`): a
 * held submission goes out even if the user signs out first. Set on create
 * or, while `undoStatus` is `pending`, with `SEND_WHEN_SIGNED_OUT_PATCH`.
 */
export const SEND_WHEN_SIGNED_OUT =
  "https://helixnebula.tech/jmap/bridge/sendWhenSignedOut" as const;

/** `SEND_WHEN_SIGNED_OUT` as a patch key (a JSON pointer: `/` is `~1`). */
export const SEND_WHEN_SIGNED_OUT_PATCH =
  "https:~1~1helixnebula.tech~1jmap~1bridge~1sendWhenSignedOut" as const;

export type Capability = (typeof CAPABILITIES)[keyof typeof CAPABILITIES];
