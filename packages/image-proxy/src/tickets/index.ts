// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// `@nebulamail/image-proxy/tickets`: signing (BFF) and verification (proxy) of
// image tickets. WebCrypto only; runs on any runtime.

export {
  generateTicketKeyEntry,
  imageUrl,
  MAX_TICKET_LENGTH,
  MAX_TICKET_TTL_MS,
  MAX_TICKET_URL_LENGTH,
  parseTicketKeys,
  signTicket,
  TICKET_CLOCK_SKEW_MS,
  TicketError,
  verifyTicket,
  type TicketClaims,
  type TicketFailure,
  type TicketKeys,
  type TicketResult,
} from "./ticket";
export {
  MAX_PREFERENCE_ADDRESSES,
  MAX_PREFERENCE_TTL_MS,
  PREFERENCES_PATH,
  signPreferenceRequest,
  verifyPreferenceRequest,
  type PreferenceRequest,
  type PreferenceVerdict,
} from "./preload-prefs";
