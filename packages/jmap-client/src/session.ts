// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// The JMAP session resource (RFC 8620 §2): fetching, parsing and the
// capability helpers.

import { z } from "zod";

import { CAPABILITIES } from "./capabilities";
import type { Capability } from "./capabilities";
import { JmapError, parseOrThrow, requestErrorFromBody } from "./errors";
import { AccountId, State } from "./ids";
import type { OtherString } from "./other-string";
import { UnsignedInt } from "./standard";
import { readLimitedText } from "./transport";
import type { Transport } from "./transport";
import { checkTemplate, checkUrl } from "./url";
import type { OriginPolicy } from "./url";

export const CoreCapabilitySchema = z.looseObject({
  collationAlgorithms: z.array(z.string()),
  maxCallsInRequest: UnsignedInt.min(1),
  maxConcurrentRequests: UnsignedInt.min(1),
  maxConcurrentUpload: UnsignedInt.min(1),
  maxObjectsInGet: UnsignedInt.min(1),
  maxObjectsInSet: UnsignedInt.min(1),
  maxSizeRequest: UnsignedInt,
  maxSizeUpload: UnsignedInt,
});
export type CoreCapability = z.infer<typeof CoreCapabilitySchema>;

/** Account-level `urn:ietf:params:jmap:mail` (RFC 8621 §1.3.1). */
export const MailAccountCapabilitySchema = z.looseObject({
  emailQuerySortOptions: z.array(z.string()),
  mayCreateTopLevelMailbox: z.boolean(),
  maxMailboxDepth: UnsignedInt.nullable(),
  maxMailboxesPerEmail: UnsignedInt.nullable(),
  maxSizeAttachmentsPerEmail: UnsignedInt,
  maxSizeMailboxName: UnsignedInt,
});
export type MailAccountCapability = z.infer<typeof MailAccountCapabilitySchema>;

/** Account-level `urn:ietf:params:jmap:submission` (RFC 8621 §1.3.2). */
export const SubmissionAccountCapabilitySchema = z.looseObject({
  maxDelayedSend: UnsignedInt,
  submissionExtensions: z.record(z.string(), z.array(z.string())),
});
export type SubmissionAccountCapability = z.infer<
  typeof SubmissionAccountCapabilitySchema
>;

const AccountSchema = z.looseObject({
  accountCapabilities: z.record(z.string(), z.unknown()),
  isPersonal: z.boolean(),
  isReadOnly: z.boolean(),
  name: z.string(),
});
export type Account = z.infer<typeof AccountSchema>;

const RawSessionSchema = z.looseObject({
  accounts: z.partialRecord(AccountId, AccountSchema),
  apiUrl: z.string(),
  capabilities: z
    .record(z.string(), z.unknown())
    .refine((caps) => Object.hasOwn(caps, CAPABILITIES.core), {
      message: "missing core capability",
    }),
  downloadUrl: z.string(),
  eventSourceUrl: z.string(),
  primaryAccounts: z.record(z.string(), AccountId),
  state: State,
  uploadUrl: z.string(),
  username: z.string(),
});

export interface Session {
  /** All capabilities with their raw (server-defined) values. */
  readonly capabilities: Readonly<Record<string, unknown>>;
  /** `urn:ietf:params:jmap:core`, parsed: the limits the client respects. */
  readonly core: CoreCapability;
  readonly accounts: Readonly<Partial<Record<AccountId, Account>>>;
  readonly primaryAccounts: Readonly<Partial<Record<string, AccountId>>>;
  readonly username: string;
  /** Resolved and origin-checked. */
  readonly apiUrl: URL;
  /** Templates: syntax and origin checked; expanded per use. */
  readonly downloadUrl: string;
  readonly uploadUrl: string;
  readonly eventSourceUrl: string;
  readonly state: State;
  /** Where the session was actually fetched from (after redirects). */
  readonly sessionUrl: URL;
}

/** Parses a session document. Throws `JmapError` (never a raw ZodError). */
export const parseSession = (
  raw: unknown,
  sessionUrl: URL,
  policy: OriginPolicy
): Session => {
  const session = parseOrThrow(RawSessionSchema, raw);
  const core = parseOrThrow(
    CoreCapabilitySchema,
    session.capabilities[CAPABILITIES.core]
  );
  return {
    accounts: session.accounts,
    apiUrl: checkUrl(session.apiUrl, sessionUrl, policy),
    capabilities: session.capabilities,
    core,
    downloadUrl: checkTemplate(
      session.downloadUrl,
      ["accountId", "blobId"],
      sessionUrl,
      policy
    ),
    eventSourceUrl: checkTemplate(
      session.eventSourceUrl,
      [],
      sessionUrl,
      policy
    ),
    primaryAccounts: session.primaryAccounts,
    sessionUrl,
    state: session.state,
    uploadUrl: checkTemplate(
      session.uploadUrl,
      ["accountId"],
      sessionUrl,
      policy
    ),
    username: session.username,
  };
};

/**
 * Whether the server (no `accountId`) or a given account supports `cap`.
 * Account capabilities are what matter for data methods (RFC 8620 §2).
 */
export const hasCapability = (
  session: Session,
  capability: Capability | OtherString,
  accountId?: AccountId
): boolean => {
  if (!Object.hasOwn(session.capabilities, capability)) {
    return false;
  }
  if (accountId === undefined) {
    return true;
  }
  const account = session.accounts[accountId];
  return (
    account !== undefined &&
    Object.hasOwn(account.accountCapabilities, capability)
  );
};

/**
 * Whether an account has a capability, looking only at the account.
 * Stalwart announces some extensions (`mail:share`) per account but not
 * in the session's `capabilities`, which `hasCapability` requires.
 */
export const hasAccountCapability = (
  session: Session,
  capability: Capability | OtherString,
  accountId: AccountId
): boolean => {
  const account = session.accounts[accountId];
  return (
    account !== undefined &&
    Object.hasOwn(account.accountCapabilities, capability)
  );
};

/**
 * Whether the server is the bridge and sends held messages after the user
 * signs out (`{"sendWhenSignedOut": true}` in its session capability).
 */
export const sendsWhenSignedOut = (session: Session): boolean => {
  const bridge: unknown = session.capabilities[CAPABILITIES.bridge];
  return (
    typeof bridge === "object" &&
    bridge !== null &&
    "sendWhenSignedOut" in bridge &&
    bridge.sendWhenSignedOut === true
  );
};

const ServerAlertSchema = z.object({
  id: z.string().min(1).max(64),
  receivedAt: z.string(),
  // The bridge cleans and cuts it to 500 characters; some room for safety.
  text: z.string().min(1).max(2000),
});

/**
 * An `[ALERT]` of the account's IMAP server, which "MUST be presented to
 * the user" (RFC 3501 §7.1, RFC 9051 §7.1). Untrusted text: show it as
 * plain text.
 */
export type ServerAlert = z.infer<typeof ServerAlertSchema>;

/**
 * The bridge's alerts of the mail server for an account (`alerts` in its
 * account capability). Invalid entries are skipped; other
 * servers have none.
 */
export const serverAlerts = (
  session: Session,
  accountId: AccountId
): ServerAlert[] => {
  const bridge: unknown =
    session.accounts[accountId]?.accountCapabilities[CAPABILITIES.bridge];
  if (typeof bridge !== "object" || bridge === null || !("alerts" in bridge)) {
    return [];
  }
  const { alerts } = bridge;
  if (!Array.isArray(alerts)) {
    return [];
  }
  return alerts.flatMap((item: unknown) => {
    const parsed = ServerAlertSchema.safeParse(item);
    return parsed.success ? [parsed.data] : [];
  });
};

/** The primary account for a capability (e.g. mail), if any. */
export const primaryAccountId = (
  session: Session,
  capability: Capability | OtherString = CAPABILITIES.mail
): AccountId | undefined => session.primaryAccounts[capability];

/** Parsed account-level mail capability, or `undefined` if absent/invalid. */
export const mailCapability = (
  session: Session,
  accountId: AccountId
): MailAccountCapability | undefined => {
  const raw =
    session.accounts[accountId]?.accountCapabilities[CAPABILITIES.mail];
  const parsed = MailAccountCapabilitySchema.safeParse(raw);
  return parsed.success ? parsed.data : undefined;
};

/** Parsed account-level submission capability, or `undefined`. */
export const submissionCapability = (
  session: Session,
  accountId: AccountId
): SubmissionAccountCapability | undefined => {
  const raw =
    session.accounts[accountId]?.accountCapabilities[CAPABILITIES.submission];
  const parsed = SubmissionAccountCapabilitySchema.safeParse(raw);
  return parsed.success ? parsed.data : undefined;
};

// ---------------------------------------------------------------- fetching

const MAX_REDIRECTS = 3;
// A session document is small; refuse to buffer something huge.
const MAX_SESSION_BYTES = 1024 * 1024;

const isRedirect = (response: Response): boolean =>
  response.type === "opaqueredirect" ||
  (response.status >= 300 && response.status < 400);

const probeRedirect = async (
  fetchUnauthenticated: (url: URL, signal?: AbortSignal) => Promise<Response>,
  url: URL,
  signal: AbortSignal | undefined
): Promise<string> => {
  try {
    const probe = await fetchUnauthenticated(url, signal);
    await probe.body?.cancel();
    return probe.url === "" ? url.href : probe.url;
  } catch (error) {
    throw new JmapError({ kind: "network" }, { cause: error });
  }
};

type Hop = { readonly session: Session } | { readonly next: string };

/** One request of the discovery: either the session or a redirect target. */
const fetchHop = async (
  transport: Transport,
  fetchUnauthenticated: (url: URL, signal?: AbortSignal) => Promise<Response>,
  url: URL,
  policy: OriginPolicy,
  signal: AbortSignal | undefined
): Promise<Hop> => {
  const response = await transport.send(url, {
    headers: { Accept: "application/json" },
    redirect: "manual",
    signal,
  });
  if (isRedirect(response)) {
    await response.body?.cancel();
    const location = response.headers.get("location");
    return {
      next:
        location ?? (await probeRedirect(fetchUnauthenticated, url, signal)),
    };
  }
  const body = await readLimitedText(response, MAX_SESSION_BYTES);
  if (!response.ok) {
    throw requestErrorFromBody(response.status, body);
  }
  let json: unknown = undefined;
  try {
    json = JSON.parse(body);
  } catch (error) {
    throw new JmapError(
      { issues: ["session is not JSON"], kind: "invalidResponse" },
      { cause: error }
    );
  }
  return { session: parseSession(json, url, policy) };
};

/**
 * Fetches the session. Redirects (common for `/.well-known/jmap`) are not
 * followed by fetch, because fetch would carry the Bearer token along. We
 * look at the target first and only then send the token there, after the
 * origin check. Browsers hide `Location` for manual redirects
 * (`opaqueredirect`), so there we learn the target with an *unauthenticated*
 * request that is allowed to follow redirects.
 */
export const fetchSession = async (
  transport: Transport,
  fetchUnauthenticated: (url: URL, signal?: AbortSignal) => Promise<Response>,
  start: URL,
  policy: OriginPolicy,
  signal?: AbortSignal
): Promise<Session> => {
  let url = checkUrl(start.href, start, policy);
  for (let hop = 0; hop <= MAX_REDIRECTS; hop += 1) {
    // Each hop depends on the previous response: sequential on purpose.
    // oxlint-disable-next-line no-await-in-loop
    const result = await fetchHop(
      transport,
      fetchUnauthenticated,
      url,
      policy,
      signal
    );
    if ("session" in result) {
      return result.session;
    }
    url = checkUrl(result.next, url, policy);
  }
  throw new JmapError({
    actual: MAX_REDIRECTS + 1,
    kind: "limit",
    limit: "maxRedirects",
    max: MAX_REDIRECTS,
  });
};
