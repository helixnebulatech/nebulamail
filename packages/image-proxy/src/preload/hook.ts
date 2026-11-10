// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// `preload` mode: images are fetched when an email arrives,
// so the sender learns only that the email arrived, never when (or
// whether) it is opened. Two ways in:
//
// - `POST /hooks/preload-urls` (always, `url-hook.ts`): the relay, next to
//   Stalwart, reads the email there and sends only its image URLs and the
//   hashed recipients. The proxy never sees the email. Optional.
// - `POST /hooks/stalwart` (only with `fullEmail`): Stalwart's MTA Hook
//   posts the WHOLE email (headers, body, attachments) here. Whoever runs
//   the proxy (a host like Vercel included) can read all incoming mail.
//   Off by default; logs a warning when on.
//
// - Auth: `Authorization: Bearer <IMAGE_PRELOAD_SECRET>` (the hook's or the
//   relay's Bearer token), compared in constant time. Anything else: 401.
// - We never block mail: once authenticated, every answer is
//   `{"action":"accept"}`, also for bodies we cannot read or emails we skip.
// - Same pipeline as the rest of the proxy: URLs are extracted with
//   mail-render's `extractRemoteImageUrls` (the same function and the same
//   kind of parser as the BFF, so the URLs match the tickets exactly),
//   trackers are dropped (also on every redirect hop), every download goes
//   through net-guard with the per-email budget and the global download
//   slots, and only checked raster images are stored.
// - Entries are stored under a shared preload key, `storageKey("*preload",
//   sha256(url))`: at DATA time there is no JMAP account or email id yet.
//   "*" is never in a JMAP id, so it cannot collide with an account. The
//   cache paths look up the account's key first, then this one.
// - Limits: `preload:global` rate limit (before reading the body), a cap
//   on the request body, a small job queue (busy = skipped), the per-email
//   limits of cache mode.
// - Per recipient (`usage.ts`): envelope recipients are hashed
//   (`recipients.ts`); opted-out ones and those over their quota are left
//   out, and an email with nobody left is not preloaded (it then behaves
//   like `cache`). No envelope recipient, or more than
//   `MAX_HOOK_RECIPIENTS`: not preloaded either (fail closed).
// - `POST /preload/preferences` (`handlePreferences`): the BFF's signed
//   opt-out / opt-in (`tickets/preload-prefs.ts`). An opt-out deletes, in
//   the background, the preloaded images only that recipient used. The
//   answer also says whether an address that still preloads is over its
//   quota, and until when (`full`), so the app can warn the user.

import type { ConcurrencyLimiter } from "@nebulamail/net-guard";
import {
  createConcurrencyLimiter,
  QueueFullError,
} from "@nebulamail/net-guard";

import { readLimited, sameSecret } from "../body";
import { keepForMs } from "../cache-policy";
import type {
  FetchImage,
  ImageStorage,
  PreloadConfig,
  ProxyLimits,
  ProxyLogEvent,
  RequestContext,
} from "../config";
import { downloadAll } from "../download";
import { storageKey } from "../storage-key";
import { newEmailId, signEmailToken } from "../tickets/email-token";
import {
  MAX_PREFERENCE_BODY_BYTES,
  verifyPreferenceRequest,
} from "../tickets/preload-prefs";
import type { TicketKeys } from "../tickets/ticket";
import { hashUrl } from "../tickets/ticket";
import {
  ACCEPT,
  acceptWith,
  MAX_HOOK_RECIPIENTS,
  parseHookMessage,
  readHookMessage,
  safeIsTracker,
  tokenModifications,
  tooMuchHtml,
} from "./parse";
import type { HookMessage, ParsedEmail } from "./parse";
import { createRecipientHasher, hashRecipients } from "./recipients";
import type { RecipientHasher } from "./recipients";
import { putRecord } from "./records";
import { isImageUrl, MAX_URL_HOOK_BODY_BYTES, UrlHookBody } from "./url-hook";
import { createUsageIndex } from "./usage";
import type { ChargedImage, OptOutState, UsageIndex } from "./usage";

/** Not a valid JMAP id (`[A-Za-z0-9_-]`), so never an account's key. */
export const PRELOAD_ACCOUNT = "*preload";

export const preloadKey = async (url: string): Promise<string> =>
  await storageKey(PRELOAD_ACCOUNT, await hashUrl(url));

export const MIN_PRELOAD_SECRET_LENGTH = 32;
export const DEFAULT_MAX_MESSAGE_BYTES = 25 * 1024 * 1024;
export {
  ACCEPT,
  MAX_HOOK_RECIPIENTS,
  MAX_TOKEN_HTML_CHARS,
  tokenModifications,
} from "./parse";

export interface PreloadDeps {
  readonly config: PreloadConfig;
  /** Verifies the BFF's preference requests. */
  readonly keys: TicketKeys;
  readonly now: () => number;
  readonly storage: ImageStorage;
  readonly limits: ProxyLimits;
  readonly fetchImage: FetchImage;
  readonly isTracker: (url: string) => boolean;
  readonly slots: ConcurrencyLimiter;
  readonly allowed: (key: string) => Promise<boolean>;
  readonly log: (event: ProxyLogEvent) => void;
}

export interface HookAnswer {
  readonly status: number;
  readonly body: unknown;
}

export interface Preloader {
  /** `POST /hooks/stalwart` is open (the whole email reaches the proxy). */
  readonly fullEmail: boolean;
  /** `POST /hooks/stalwart`: the whole email from Stalwart's MTA Hook. */
  readonly handle: (
    request: Request,
    context: RequestContext
  ) => Promise<HookAnswer>;
  /** `POST /hooks/preload-urls`: image URLs and hashed recipients (relay). */
  readonly handleUrls: (
    request: Request,
    context: RequestContext
  ) => Promise<HookAnswer>;
  /** `POST /preload/preferences` (signed by the BFF). */
  readonly handlePreferences: (
    request: Request,
    context: RequestContext
  ) => Promise<HookAnswer>;
  /** Drop expired quota and reference records (`usage.ts`). */
  readonly sweep: () => Promise<number>;
}

interface Inner extends PreloadDeps {
  readonly usage: UsageIndex;
  readonly hasher: RecipientHasher;
}

const bearer = (header: string | null): string | null => {
  const match = /^Bearer (?<token>\S+)$/u.exec(header ?? "");
  return match?.groups?.token ?? null;
};

/**
 * Some recipient (hashed) takes preloaded images: not opted out, under
 * quota. Only then does the email get a token. Fails closed, like
 * `usage.check`.
 */
const anyRecipientAllowed = async (
  deps: Inner,
  recipients: readonly string[]
): Promise<boolean> => {
  if (recipients.length === 0) {
    return false;
  }
  const { allowed } = await deps.usage.check(recipients);
  return allowed.length > 0;
};

/**
 * Download and store `urls` for these recipients (already hashed), within
 * their quota and opt-outs. Shared by both hooks.
 */
const preloadUrls = async (
  deps: Inner,
  urls: readonly string[],
  recipients: readonly string[]
): Promise<void> => {
  if (urls.length === 0) {
    return;
  }
  const { allowed, full, optedOut } = await deps.usage.check(recipients);
  if (allowed.length === 0) {
    deps.log({
      event: "preload-skipped",
      reason: full > 0 ? "quota" : "opted-out",
    });
    return;
  }
  // Reserve first (`usage.ts`): from here on, another recipient's opt-out
  // cannot delete these entries between the lookup and the charge.
  const all = await Promise.all(
    urls.map(async (url) => ({ url, urlHash: await hashUrl(url) }))
  );
  const reserved = await deps.usage.reserve(
    allowed,
    all.map(({ urlHash }) => urlHash)
  );
  const wanted = all.filter(({ urlHash }) => reserved.has(urlHash));
  const present = await Promise.all(
    wanted.map(async ({ url, urlHash }) => {
      try {
        const stored = await deps.storage.get(await preloadKey(url));
        return { size: stored?.bytes.byteLength ?? null, url, urlHash };
      } catch {
        return { size: null, url, urlHash };
      }
    })
  );
  const charged: ChargedImage[] = [];
  for (const { size, urlHash } of present) {
    if (size !== null) {
      charged.push({ bytes: size, urlHash });
    }
  }
  const missing = present
    .filter(({ size }) => size === null)
    .map(({ url }) => url);
  const outcomes = await downloadAll(missing, {
    allowRedirect: (url) => {
      const tracker = safeIsTracker(deps.isTracker, url);
      if (tracker) {
        deps.log({ event: "tracker-redirect" });
      }
      return !tracker;
    },
    concurrency: deps.limits.concurrency,
    fetchImage: deps.fetchImage,
    maxBytesPerEmail: deps.limits.maxBytesPerEmail,
    maxBytesPerImage: deps.limits.maxBytesPerImage,
    maxRequestsPerEmail: deps.limits.maxRequestsPerEmail,
    slots: deps.slots,
    timeoutMs: deps.limits.batchTimeoutMs,
  });
  let fetched = 0;
  let bytes = 0;
  // Not reserved (state store down): not preloaded, counted as failed.
  let failed = urls.length - wanted.length;
  await Promise.all(
    missing.map(async (url) => {
      const outcome = outcomes.get(url);
      if (outcome?.ok !== true) {
        failed += 1;
        return;
      }
      // RFC 9111: preloaded entries are shared between users, so never
      // `private` or `no-store` ones (cache-policy.ts). Such an
      // image is simply not preloaded: the reader fetches it on open.
      const ttlMs = keepForMs(outcome.image, "shared", {
        fixedTtlMs: deps.storage.fixedTtlMs,
        maxMs: deps.limits.cacheTtlMs,
      });
      if (ttlMs === 0) {
        failed += 1;
        return;
      }
      try {
        await deps.storage.put(await preloadKey(url), outcome.image, ttlMs);
        fetched += 1;
        bytes += outcome.image.bytes.byteLength;
        charged.push({
          bytes: outcome.image.bytes.byteLength,
          urlHash: await hashUrl(url),
        });
      } catch {
        failed += 1;
        deps.log({ event: "storage-error" });
      }
    })
  );
  await deps.usage.charge(allowed, charged);
  deps.log({
    bytes,
    event: "preload",
    failed,
    fetched,
    fromCache: wanted.length - missing.length,
    images: urls.length,
    optedOut,
    overQuota: full,
    recipients: allowed.length,
  });
};

/** Hashed recipients of a parsed email, or null (logged) when it is skipped. */
const emailRecipients = async (
  deps: Inner,
  parsed: ParsedEmail
): Promise<string[] | null> => {
  if (parsed.recipients.length > MAX_HOOK_RECIPIENTS) {
    deps.log({ event: "preload-skipped", reason: "too-many-recipients" });
    return null;
  }
  const recipients = await hashRecipients(deps.hasher, parsed.recipients);
  if (recipients.length === 0) {
    // Without a recipient neither the quota nor an opt-out can be applied.
    deps.log({ event: "preload-skipped", reason: "no-recipient" });
    return null;
  }
  return recipients;
};

const preloadEmail = async (
  deps: Inner,
  parsed: ParsedEmail | null
): Promise<void> => {
  if (parsed === null) {
    deps.log({ event: "preload-skipped", reason: "invalid" });
    return;
  }
  if (parsed.urls.length === 0) {
    if (parsed.unparsable > 0) {
      deps.log({ event: "preload-skipped", reason: "unparsable" });
    }
    return;
  }
  const recipients = await emailRecipients(deps, parsed);
  if (recipients !== null) {
    await preloadUrls(deps, parsed.urls, recipients);
  }
};

/** Delete what was preloaded only for these recipients. Never rejects. */
const purgeAll = async (
  deps: Inner,
  recipients: readonly string[]
): Promise<void> => {
  let deleted = 0;
  for (const recipient of recipients) {
    try {
      // oxlint-disable-next-line no-await-in-loop -- one recipient at a time.
      deleted += await deps.usage.purge(recipient);
    } catch {
      deps.log({ event: "storage-error" });
    }
  }
  deps.log({ deleted, event: "preload-purge" });
};

/** The BFF's signed opt-out / opt-in, or a read of the current state. */
const handlePreferences = async (
  deps: Inner,
  request: Request,
  context: RequestContext
): Promise<HookAnswer> => {
  // Per IP before reading; the shared bucket only once the BFF's signature
  // is valid, so unsigned requests cannot spend it.
  if (
    context.clientAddress !== undefined &&
    !(await deps.allowed(`preload-prefs:ip:${context.clientAddress}`))
  ) {
    return { body: { error: "rate-limited" }, status: 429 };
  }
  const body = await readLimited(request, MAX_PREFERENCE_BODY_BYTES);
  if (!body.ok) {
    return {
      body: { error: body.code },
      status: body.code === "body-too-large" ? 413 : 400,
    };
  }
  const verdict = await verifyPreferenceRequest(
    deps.keys,
    request.headers.get("authorization"),
    body.text,
    deps.now()
  );
  if (!verdict.ok) {
    const status = verdict.reason === "unauthorized" ? 401 : 400;
    deps.log({ event: "rejected", reason: verdict.reason, status });
    return {
      body: {
        error:
          verdict.reason === "unauthorized" ? "unauthorized" : "invalid-body",
      },
      status,
    };
  }
  if (!(await deps.allowed("preload-prefs:global"))) {
    return { body: { error: "rate-limited" }, status: 429 };
  }
  const { addresses, at, optOut } = verdict.request;
  const recipients = await hashRecipients(deps.hasher, addresses);
  if (recipients.length === 0) {
    return { body: { error: "invalid-body" }, status: 400 };
  }
  if (optOut === undefined) {
    const off = await deps.usage.optedOut(recipients);
    return {
      body: {
        full: off ? null : await deps.usage.full(recipients),
        optOut: off,
      },
      status: 200,
    };
  }
  let state: OptOutState;
  try {
    state = await deps.usage.setOptOut(recipients, optOut, at);
  } catch {
    deps.log({ event: "storage-error" });
    return { body: { error: "storage" }, status: 503 };
  }
  deps.log({
    addresses: recipients.length,
    applied: state.optOut === optOut,
    event: "preload-preferences",
    optOut: state.optOut,
  });
  if (state.optedOut.length > 0) {
    // After answering. Only the addresses that are off now (a newer "on"
    // stored for one of them wins). Idempotent: turning it off again (the
    // switch, or any new opt-out request) finishes a purge cut short.
    const purge = purgeAll(deps, state.optedOut);
    context.waitUntil?.(purge);
  }
  // The answer says "off" only if every address is off (as the read does).
  return {
    body: {
      full: state.optOut ? null : await deps.usage.full(recipients),
      optOut: state.optOut,
    },
    status: 200,
  };
};

interface IssuedToken {
  readonly token: string;
  /** Writes the email's record (which images the token may fetch). Never rejects. */
  readonly record: () => Promise<void>;
}

/** A new email id and its token, for an email that is about to be preloaded. */
const issueToken = async (
  deps: Inner,
  urls: readonly string[]
): Promise<IssuedToken> => {
  const id = newEmailId();
  const token = await signEmailToken(deps.keys, id, deps.now());
  const hashes = await Promise.all(urls.map(hashUrl));
  return {
    record: async () => {
      try {
        await putRecord(deps.storage, id, hashes, deps.limits.cacheTtlMs);
      } catch {
        deps.log({ event: "storage-error" });
      }
    },
    token,
  };
};

/**
 * Email tokens on: the email is read now, because
 * the answer carries its token; the record and the downloads go to the
 * background (`background`). An email with no image to preload gets no
 * token, but a forged one is still removed.
 */
const withEmailToken = async (
  deps: Inner,
  hook: HookMessage | null,
  parse: (hook: HookMessage) => Promise<ParsedEmail>,
  background: (work: () => Promise<void>) => void
): Promise<HookAnswer> => {
  if (hook === null) {
    deps.log({ event: "preload-skipped", reason: "invalid" });
    return { body: ACCEPT, status: 200 };
  }
  const withoutToken = (
    later: () => Promise<ParsedEmail> | ParsedEmail
  ): HookAnswer => {
    background(async () => {
      await preloadEmail(deps, await later());
    });
    return {
      body: acceptWith(tokenModifications(hook.headers, null)),
      status: 200,
    };
  };
  // Stalwart waits for this answer: big HTML is parsed afterwards, and
  // that email goes without a token (its images stay blocked in option D).
  if (tooMuchHtml(hook)) {
    deps.log({ event: "preload-token-skipped", reason: "too-large" });
    return withoutToken(async () => await parse(hook));
  }
  const parsed = await parse(hook);
  const eligible =
    parsed.urls.length > 0 &&
    parsed.recipients.length > 0 &&
    parsed.recipients.length <= MAX_HOOK_RECIPIENTS &&
    (await anyRecipientAllowed(
      deps,
      await hashRecipients(deps.hasher, parsed.recipients)
    ));
  if (!eligible) {
    return withoutToken(() => parsed);
  }
  const issued = await issueToken(deps, parsed.urls);
  background(async () => {
    await issued.record();
    await preloadEmail(deps, parsed);
  });
  return {
    body: acceptWith(tokenModifications(parsed.headers, issued.token)),
    status: 200,
  };
};

/** `POST /hooks/preload-urls`: the relay's URLs and hashed recipients. */
const handleUrls = async (
  deps: Inner,
  request: Request,
  context: RequestContext,
  background: (work: () => Promise<void>) => Promise<void>
): Promise<HookAnswer> => {
  if (!(await deps.allowed("preload:global"))) {
    deps.log({ event: "preload-skipped", reason: "rate-limited" });
    return { body: { error: "rate-limited" }, status: 429 };
  }
  const body = await readLimited(request, MAX_URL_HOOK_BODY_BYTES);
  if (!body.ok) {
    return {
      body: { error: body.code },
      status: body.code === "body-too-large" ? 413 : 400,
    };
  }
  let raw: unknown;
  try {
    raw = JSON.parse(body.text);
  } catch {
    raw = undefined;
  }
  const parsed = UrlHookBody.safeParse(raw);
  if (!parsed.success) {
    deps.log({ event: "rejected", reason: "invalid-body", status: 400 });
    return { body: { error: "invalid-body" }, status: 400 };
  }
  // Checked again here: the relay is trusted to send URLs, not to filter them.
  const urls = [
    ...new Set(
      parsed.data.urls.filter(
        (url) => isImageUrl(url) && !safeIsTracker(deps.isTracker, url)
      )
    ),
  ].slice(0, deps.limits.maxImagesPerEmail);
  const recipients = [...new Set(parsed.data.recipients)];
  if (urls.length === 0) {
    return { body: {}, status: 200 };
  }
  if (
    parsed.data.token === true &&
    deps.config.emailTokens === true &&
    (await anyRecipientAllowed(deps, recipients))
  ) {
    const issued = await issueToken(deps, urls);
    // Started here: with no waitUntil (Node) the process keeps it.
    const job = background(async () => {
      await issued.record();
      await preloadUrls(deps, urls, recipients);
    });
    context.waitUntil?.(job);
    return { body: { token: issued.token }, status: 200 };
  }
  const job = background(async () => {
    await preloadUrls(deps, urls, recipients);
  });
  context.waitUntil?.(job);
  return { body: {}, status: 200 };
};

export const createPreloader = (outer: PreloadDeps): Preloader => {
  const { config } = outer;
  if (config.secret.length < MIN_PRELOAD_SECRET_LENGTH) {
    throw new TypeError(
      `image proxy: the preload secret needs ${String(MIN_PRELOAD_SECRET_LENGTH)}+ characters`
    );
  }
  const indexKey = config.indexKey ?? config.secret;
  if (indexKey.length < MIN_PRELOAD_SECRET_LENGTH) {
    throw new TypeError(
      `image proxy: the preload index key needs ${String(MIN_PRELOAD_SECRET_LENGTH)}+ characters`
    );
  }
  const fullEmail = config.fullEmail === true;
  const { extractor } = config;
  if (fullEmail && extractor === undefined) {
    throw new TypeError(
      "image proxy: preload with the full email needs an HTML extractor"
    );
  }
  const deps: Inner = {
    ...outer,
    hasher: createRecipientHasher(indexKey),
    usage: createUsageIndex({
      images: outer.storage,
      now: outer.now,
      onError: () => {
        outer.log({ event: "storage-error" });
      },
      store: config.state,
      ttlMs: outer.limits.cacheTtlMs,
      ...(config.maxBytesPerRecipient === undefined
        ? {}
        : { maxBytes: config.maxBytesPerRecipient }),
      ...(config.maxImagesPerRecipient === undefined
        ? {}
        : { maxImages: config.maxImagesPerRecipient }),
    }),
  };
  if (fullEmail) {
    // Once, at start: whoever runs this proxy can read every incoming email.
    deps.log({ event: "preload-full-email" });
  }
  const jobs = createConcurrencyLimiter({
    max: config.concurrency ?? 2,
    maxQueue: config.maxQueue ?? 50,
  });
  const maxBytes = config.maxMessageBytes ?? DEFAULT_MAX_MESSAGE_BYTES;
  const maxQueuedChars = config.maxQueuedChars ?? 4 * maxBytes;
  if (!Number.isSafeInteger(maxQueuedChars) || maxQueuedChars <= 0) {
    throw new TypeError("image proxy: maxQueuedChars must be positive");
  }
  /** Characters of whole emails held by queued or running jobs. */
  let queuedChars = 0;
  const parse =
    extractor === undefined
      ? null
      : async (hook: HookMessage): Promise<ParsedEmail> =>
          await parseHookMessage(
            {
              extractor,
              isTracker: deps.isTracker,
              maxImages: deps.limits.maxImagesPerEmail,
            },
            hook
          );

  /** One email in the background. Never rejects. */
  const runJob = async (work: () => Promise<void>): Promise<void> => {
    try {
      await jobs.run(work);
    } catch (error) {
      deps.log({
        event: "preload-skipped",
        reason: error instanceof QueueFullError ? "busy" : "invalid",
      });
    }
  };

  /**
   * A whole-email job holding `size` characters. Over `maxQueuedChars`
   * (all such jobs together) it is skipped as `busy`: the queue length
   * alone would let 52 emails of 25 MB sit in memory.
   */
  const runSized = async (
    size: number,
    work: () => Promise<void>
  ): Promise<void> => {
    if (queuedChars + size > maxQueuedChars) {
      deps.log({ event: "preload-skipped", reason: "busy" });
      return;
    }
    queuedChars += size;
    try {
      await runJob(work);
    } finally {
      queuedChars -= size;
    }
  };

  const authorized = async (request: Request): Promise<boolean> => {
    const token = bearer(request.headers.get("authorization"));
    return token !== null && (await sameSecret(token, config.secret));
  };

  return {
    fullEmail,
    sweep: deps.usage.sweep,
    handlePreferences: async (request, context) =>
      await handlePreferences(deps, request, context),
    handleUrls: async (request, context) =>
      (await authorized(request))
        ? await handleUrls(deps, request, context, runJob)
        : { body: { error: "unauthorized" }, status: 401 },
    handle: async (request, context) => {
      if (!fullEmail || parse === null) {
        return { body: { error: "not-found" }, status: 404 };
      }
      if (!(await authorized(request))) {
        return { body: { error: "unauthorized" }, status: 401 };
      }
      if (!(await deps.allowed("preload:global"))) {
        deps.log({ event: "preload-skipped", reason: "rate-limited" });
        return { body: ACCEPT, status: 200 };
      }
      const body = await readLimited(request, maxBytes);
      if (!body.ok) {
        deps.log({
          event: "preload-skipped",
          reason: body.code === "body-too-large" ? "too-large" : "invalid",
        });
        return { body: ACCEPT, status: 200 };
      }
      if (config.emailTokens === true) {
        return await withEmailToken(
          deps,
          readHookMessage(body.text),
          parse,
          (work) => {
            // Started here: with no waitUntil (Node) the process keeps it.
            const job = runSized(body.text.length, work);
            context.waitUntil?.(job);
          }
        );
      }
      // Not awaited: Stalwart gets its answer now. Workers and Vercel keep
      // the job alive through waitUntil; on Node the process does.
      const job = runSized(body.text.length, async () => {
        const hook = readHookMessage(body.text);
        await preloadEmail(deps, hook === null ? null : await parse(hook));
      });
      context.waitUntil?.(job);
      return { body: ACCEPT, status: 200 };
    },
  };
};
