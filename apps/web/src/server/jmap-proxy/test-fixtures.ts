// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Test fixtures of the JMAP proxy. Only imported by tests.

const PUBLIC = "https://mail.example.com";

/** What Stalwart sends: its public name, whatever name it was asked on. */
export const stalwartSession = (
  overrides: Record<string, unknown> = {}
): Record<string, unknown> => ({
  accounts: { c: { name: "alice@example.com" } },
  apiUrl: `${PUBLIC}/jmap/`,
  capabilities: {
    "urn:ietf:params:jmap:core": {
      maxSizeRequest: 10_000_000,
      maxSizeUpload: 50_000_000,
    },
    "urn:ietf:params:jmap:mail": {},
    "urn:ietf:params:jmap:websocket": {
      supportsPush: true,
      url: "wss://mail.example.com/jmap/ws",
    },
  },
  downloadUrl: `${PUBLIC}/jmap/download/{accountId}/{blobId}/{name}?accept={type}`,
  eventSourceUrl: `${PUBLIC}/jmap/eventsource/?types={types}&closeafter={closeafter}&ping={ping}`,
  primaryAccounts: { "urn:ietf:params:jmap:mail": "c" },
  state: "s1",
  uploadUrl: `${PUBLIC}/jmap/upload/{accountId}/`,
  username: "alice@example.com",
  ...overrides,
});
