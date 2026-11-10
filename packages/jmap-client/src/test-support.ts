// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Test helpers: a scripted fake `fetch` (no network) and a session document
// modelled on what Stalwart returns.

import { JmapError } from "./errors";
import type { FetchLike } from "./transport";

export const ORIGIN = "https://mail.example.com";
export const SESSION_URL = `${ORIGIN}/.well-known/jmap`;
export const API_URL = `${ORIGIN}/jmap/`;

const CORE_LIMITS = {
  collationAlgorithms: [
    "i;ascii-numeric",
    "i;ascii-casemap",
    "i;unicode-casemap",
  ],
  maxCallsInRequest: 16,
  maxConcurrentRequests: 4,
  maxConcurrentUpload: 4,
  maxObjectsInGet: 500,
  maxObjectsInSet: 500,
  maxSizeRequest: 10_000_000,
  maxSizeUpload: 50_000_000,
};

export const stalwartSession = (
  overrides: Readonly<Record<string, unknown>> = {},
  core: Readonly<Partial<typeof CORE_LIMITS>> = {}
): Record<string, unknown> => ({
  accounts: {
    d: {
      accountCapabilities: {
        "urn:ietf:params:jmap:blob": {
          maxDataSources: 16,
          maxSizeBlobSet: 7_499_488,
          supportedDigestAlgorithms: ["sha", "sha-256", "sha-512"],
          supportedTypeNames: ["Email", "Thread", "SieveScript"],
        },
        "urn:ietf:params:jmap:calendars": {},
        "urn:ietf:params:jmap:contacts": {},
        "urn:ietf:params:jmap:filenode": {},
        "urn:ietf:params:jmap:mail": {
          emailQuerySortOptions: [
            "receivedAt",
            "size",
            "from",
            "to",
            "subject",
            "sentAt",
            "hasKeyword",
            "allInThreadHaveKeyword",
            "someInThreadHaveKeyword",
          ],
          mayCreateTopLevelMailbox: true,
          maxMailboxDepth: 10,
          maxMailboxesPerEmail: null,
          maxSizeAttachmentsPerEmail: 50_000_000,
          maxSizeMailboxName: 255,
        },
        "urn:ietf:params:jmap:principals": {},
        "urn:ietf:params:jmap:quota": {},
        "urn:ietf:params:jmap:sieve": {
          implementation: "Stalwart v1.0.0",
          maxNumberRedirects: 1,
          maxSizeScriptName: 512,
        },
        "urn:ietf:params:jmap:submission": {
          maxDelayedSend: 2_592_000,
          submissionExtensions: {
            FUTURERELEASE: ["2592000", "2026-11-22T00:00:00Z"],
            SIZE: ["104857600"],
          },
        },
        "urn:ietf:params:jmap:vacationresponse": {},
      },
      isPersonal: true,
      isReadOnly: false,
      name: "jane@example.com",
    },
    e: {
      accountCapabilities: {
        "urn:ietf:params:jmap:mail": {},
      },
      isPersonal: false,
      isReadOnly: true,
      name: "shared@example.com",
    },
  },
  apiUrl: API_URL,
  capabilities: {
    "urn:ietf:params:jmap:blob": {},
    "urn:ietf:params:jmap:calendars": {},
    "urn:ietf:params:jmap:contacts": {},
    "urn:ietf:params:jmap:core": { ...CORE_LIMITS, ...core },
    "urn:ietf:params:jmap:filenode": {},
    "urn:ietf:params:jmap:mail": {},
    "urn:ietf:params:jmap:principals": {},
    "urn:ietf:params:jmap:quota": {},
    "urn:ietf:params:jmap:sieve": {},
    "urn:ietf:params:jmap:submission": {},
    "urn:ietf:params:jmap:vacationresponse": {},
    "urn:ietf:params:jmap:websocket": {
      supportsPush: true,
      url: "wss://mail.example.com/jmap/ws",
    },
  },
  downloadUrl: `${ORIGIN}/jmap/download/{accountId}/{blobId}/{name}?accept={type}`,
  eventSourceUrl: `${ORIGIN}/jmap/eventsource/?types={types}&closeafter={closeafter}&ping={ping}`,
  primaryAccounts: {
    "urn:ietf:params:jmap:mail": "d",
    "urn:ietf:params:jmap:submission": "d",
    "urn:ietf:params:jmap:vacationresponse": "d",
  },
  state: "s1",
  uploadUrl: `${ORIGIN}/jmap/upload/{accountId}/`,
  username: "jane@example.com",
  ...overrides,
});

export interface RecordedCall {
  readonly url: string;
  readonly init: RequestInit;
  readonly authorization: string | undefined;
  readonly body: unknown;
}

export type Handler = (
  call: RecordedCall,
  index: number
) => Promise<Response> | Response;

/** A fake fetch that records every call and answers with `handler`. */
export const fakeFetch = (handler: Handler) => {
  const calls: RecordedCall[] = [];
  const fetch: FetchLike = async (input, init) => {
    const headers = new Headers(init.headers);
    const { body } = init;
    const call: RecordedCall = {
      authorization: headers.get("authorization") ?? undefined,
      body: typeof body === "string" ? JSON.parse(body) : body,
      init,
      url: String(input),
    };
    calls.push(call);
    return await handler(call, calls.length - 1);
  };
  return { calls, fetch };
};

/** Answers the session URL with `session`, everything else with `api`. */
export const sessionThen =
  (
    api: Handler,
    session: Record<string, unknown> = stalwartSession()
  ): Handler =>
  async (call, index) =>
    call.url === SESSION_URL ? Response.json(session) : await api(call, index);

/** A JMAP API response body. */
export const methodResponses = (
  responses: readonly (readonly [string, Record<string, unknown>, string])[],
  sessionState = "s1"
): Response => Response.json({ methodResponses: responses, sessionState });

export const tokens = () => {
  const requests: boolean[] = [];
  const getAccessToken = async (request: {
    readonly forceRefresh: boolean;
  }) => {
    requests.push(request.forceRefresh);
    await Promise.resolve();
    return request.forceRefresh ? "fresh-token" : "old-token";
  };
  return { getAccessToken, requests };
};

export const errorOf = (run: () => unknown): JmapError => {
  try {
    run();
  } catch (error) {
    if (error instanceof JmapError) {
      return error;
    }
    throw error;
  }
  throw new Error("expected a JmapError");
};

export const rejection = async (
  promise: Promise<unknown>
): Promise<JmapError> => {
  try {
    await promise;
  } catch (error) {
    if (error instanceof JmapError) {
      return error;
    }
    throw error;
  }
  throw new Error("expected a JmapError");
};
