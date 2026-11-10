// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Live updates: JMAP push over SSE. Each change only says
// "this type changed": emails and threads are brought up to date with
// only what changed (sync.ts); folders and delivery status are fetched
// again. New inbox mail can also raise a notification while the tab is
// hidden (notify.ts).

import type {
  EmailId,
  JmapClient,
  State,
  StateChange,
} from "@nebulamail/jmap-client";
import {
  AccountId,
  CAPABILITIES,
  hasCapability,
} from "@nebulamail/jmap-client";
import type { QueryClient } from "@tanstack/react-query";

import { m } from "../paraglide/messages.js";
import { settingsKeys } from "../settings/shared-queries";
import type { MailboxSummary } from "./api";
import { getClient } from "./client";
import { hasSubmissions } from "./delivery";
import { keepMailPushInStep, mailPushAccount } from "./mail-push";
import { forgetMailboxRoles } from "./mailbox-roles";
import {
  createdSince,
  emailState,
  notificationContent,
  notificationsSupported,
  notificationsWanted,
  worthNotifying,
} from "./notify";
import { claimNotice } from "./push-store";
import { mailKeys } from "./queries";
import { sharedMailAccounts } from "./sharing";
import type { MailSync } from "./sync";
import { createMailSync, jmapSyncServer } from "./sync";

const refresh = async (
  queryClient: QueryClient,
  sync: MailSync,
  change: StateChange
): Promise<void> => {
  const jobs: Promise<void>[] = [];
  const types = new Set<string>();
  for (const [account, changed] of Object.entries(change.changed)) {
    const accountId = AccountId.safeParse(account);
    const names = Object.keys(changed ?? {});
    for (const name of names) {
      types.add(name);
    }
    if (
      accountId.success &&
      (names.includes("Email") || names.includes("Thread"))
    ) {
      jobs.push(sync.changed(accountId.data));
    }
  }
  if (types.has("Mailbox")) {
    // Drafts or Sent may be other mailboxes now (compose.ts).
    forgetMailboxRoles();
    jobs.push(
      queryClient.invalidateQueries({ queryKey: mailKeys.mailboxes() }),
      queryClient.invalidateQueries({ queryKey: settingsKeys.imapNames() })
    );
  }
  if (types.has("SieveScript")) {
    // Here or in another client (the vacation response is a script too).
    jobs.push(
      queryClient.invalidateQueries({ queryKey: settingsKeys.scripts() }),
      queryClient.invalidateQueries({ queryKey: settingsKeys.vacation() })
    );
  }
  if (types.has("EmailSubmission")) {
    // A bounce, a delivery or a read receipt arrived (delivery.ts).
    jobs.push(queryClient.invalidateQueries({ queryKey: mailKeys.delivery() }));
  }
  await Promise.all(jobs);
};

export interface PushOptions {
  /** A notification was clicked: show this email. */
  readonly onOpenEmail?: (id: EmailId) => void;
}

/**
 * Shows it unless the service worker (or another tab) already announced
 * the same emails (push-store.ts): with Web Push both may hear about it.
 */
const show = async (
  content: ReturnType<typeof notificationContent>[number],
  claims: readonly Promise<boolean>[],
  onOpenEmail: PushOptions["onOpenEmail"]
): Promise<void> => {
  // Storage unavailable: show it (a duplicate beats a missed email).
  const fresh = await Promise.all(
    claims.map(async (claim) => await claim.catch(() => true))
  );
  if (fresh.length > 0 && !fresh.includes(true)) {
    return;
  }
  try {
    const notification = new Notification(content.title, {
      body: content.body,
      tag: content.tag,
    });
    notification.addEventListener("click", () => {
      globalThis.focus();
      if (content.emailId !== null) {
        onOpenEmail?.(content.emailId);
      }
      notification.close();
    });
  } catch {
    // e.g. Chrome on Android only allows notifications from a service worker.
  }
};

/** Tracks the Email state and notifies about new inbox mail when hidden. */
const createNotifier = (
  client: JmapClient,
  queryClient: QueryClient,
  options: PushOptions
) => {
  let since: State | null = null;
  let busy = Promise.resolve();

  const run = async (pushed: State): Promise<void> => {
    const wanted =
      since !== null &&
      notificationsWanted() &&
      document.visibilityState === "hidden";
    const inbox = queryClient
      .getQueryData<MailboxSummary[]>(mailKeys.mailboxesOf(null))
      ?.find((box) => box.role === "inbox");
    if (!wanted || since === null || inbox === undefined) {
      since = pushed;
      return;
    }
    try {
      const { emails, state } = await createdSince(client, since);
      since = state;
      const worth = worthNotifying(emails, inbox.id, Date.now());
      const account = mailPushAccount(client);
      const claims = new Map(
        worth.map((email) => [email.id, claimNotice(account, email.id)])
      );
      const contents = notificationContent(worth, {
        many: (count) => m.notify_many({ count }),
        noSubject: m.row_no_subject(),
      });
      for (const content of contents) {
        const mine =
          content.emailId === null
            ? [...claims.values()]
            : [claims.get(content.emailId) ?? Promise.resolve(true)];
        // oxlint-disable-next-line no-await-in-loop -- at most 3, in order.
        await show(content, mine, options.onOpenEmail);
      }
    } catch {
      // cannotCalculateChanges or a network error: start over from here.
      since = pushed;
    }
  };

  return {
    /** Learn the starting state (only where notifications exist). */
    start: async (): Promise<void> => {
      if (notificationsSupported()) {
        since = await emailState(client).catch(() => null);
        if (since !== null) {
          // The worker announces with every tab closed (mail-push.ts).
          keepMailPushInStep(client, since, notificationsWanted()).catch(
            () => null
          );
        }
      }
    },
    /** Changes are handled one at a time, in order. */
    changed: (pushed: State): void => {
      const previous = busy;
      busy = (async () => {
        await previous;
        await run(pushed);
      })();
    },
  };
};

/**
 * Without an open push connection (the server has none, it failed, or the
 * hosting of the full BFF mode doesn't keep long responses), changes are
 * looked for this often.
 */
export const POLL_MS = 60_000;

/**
 * One look for changes, as if push had said everything changed: emails
 * and threads from their last state (`Email/changes`, sync.ts), folders
 * and delivery status again, and the notifier with the current state.
 */
const poll = async (
  client: JmapClient,
  queryClient: QueryClient,
  sync: MailSync,
  notifier: ReturnType<typeof createNotifier>
): Promise<void> => {
  const primary = client.primaryAccountId();
  const accounts = [
    primary,
    ...sharedMailAccounts(client.session.accounts, primary).map(
      (account) => account.accountId
    ),
  ];
  const state = await emailState(client).catch(() => null);
  if (state !== null) {
    notifier.changed(state);
  }
  await Promise.all([
    ...accounts.map(async (accountId) => {
      await sync.changed(accountId);
    }),
    queryClient.invalidateQueries({ queryKey: mailKeys.mailboxes() }),
    ...(hasSubmissions(client)
      ? [queryClient.invalidateQueries({ queryKey: mailKeys.delivery() })]
      : []),
  ]);
};

/**
 * Runs until `signal` aborts. Reconnects on its own (jmap-client); while
 * no push connection is delivering, polls every `POLL_MS`.
 */
export const startPush = async (
  queryClient: QueryClient,
  signal: AbortSignal,
  options: PushOptions = {}
): Promise<void> => {
  const client = await getClient();
  const accountId = client.primaryAccountId();
  const notifier = createNotifier(client, queryClient, options);
  const sync = createMailSync(queryClient, jmapSyncServer(client));
  await notifier.start();
  // Filters appear once the bridge has reached the ManageSieve server,
  // which may be after this page read the session.
  const stopWatching = client.onSessionChange(() => {
    void queryClient.invalidateQueries({ queryKey: settingsKeys.support() });
  });
  signal.addEventListener("abort", stopWatching, { once: true });
  // Also when push never opens or stops for good (a fatal error below),
  // and while a connection is open but has delivered nothing yet: a host or
  // proxy that holds the response back (Cloudflare quick tunnels, seen live
  // with the bridge on 2026-11-08) opens it and then stays silent until the
  // watchdog drops it, over and over.
  let pushLive = false;
  let polling = false;
  const tick = async (): Promise<void> => {
    if (pushLive || polling) {
      return;
    }
    polling = true;
    try {
      await poll(client, queryClient, sync, notifier);
    } catch {
      // Next time.
    } finally {
      polling = false;
    }
  };
  const poller = setInterval(() => {
    void tick();
  }, POLL_MS);
  signal.addEventListener(
    "abort",
    () => {
      clearInterval(poller);
    },
    { once: true }
  );
  const changes = client.push({
    // After a reconnect we may have missed changes: refetch everything.
    onConnect: () => {
      // Missed Mailbox changes too: Drafts or Sent may have moved.
      forgetMailboxRoles();
      void sync.reset();
    },
    onDisconnect: () => {
      pushLive = false;
    },
    onLive: () => {
      pushLive = true;
    },
    signal,
    types: [
      "Email",
      "Mailbox",
      "Thread",
      ...(hasSubmissions(client) ? (["EmailSubmission"] as const) : []),
      // Only the bridge is known to announce it.
      ...(hasCapability(client.session, CAPABILITIES.bridge)
        ? (["SieveScript"] as const)
        : []),
    ],
  });
  for await (const change of changes) {
    const emailChanged = change.changed[accountId]?.Email;
    if (emailChanged !== undefined) {
      notifier.changed(emailChanged);
    }
    await refresh(queryClient, sync, change);
  }
};
