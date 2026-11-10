// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// New-mail notifications while the tab is open but hidden:
// push (push.ts) says "Email changed"; we ask `Email/changes` what was
// created since the last state, keep unread inbox mail that just arrived,
// and show sender + subject only (never the body: lock screens). Off by
// default; the user turns it on in Settings, which asks for permission.
// With every tab closed, the service worker does the same from Web Push
// (mail-push.ts); each email is announced once (push-store.ts).

import type {
  Email,
  EmailId,
  JmapClient,
  MailboxId,
  State,
} from "@nebulamail/jmap-client";
import { cleanDisplayText } from "@nebulamail/mail-render";

/** localStorage flag (useLocalFlag) for the setting. */
export const NOTIFY_KEY = "wm-new-mail-notifications";

/** Older mail (e.g. after the laptop wakes up) isn't announced. */
const MAX_AGE_MS = 15 * 60_000;
/** More than this at once: one summary instead of one per email. */
const MAX_SINGLE = 3;

export const notificationsSupported = (): boolean =>
  typeof Notification !== "undefined";

/** On in Settings and allowed by the browser. */
export const notificationsWanted = (): boolean => {
  if (!notificationsSupported() || Notification.permission !== "granted") {
    return false;
  }
  try {
    return globalThis.localStorage.getItem(NOTIFY_KEY) === "1";
  } catch {
    return false;
  }
};

export type ArrivedEmail = Pick<
  Email,
  "id" | "from" | "subject" | "receivedAt" | "keywords" | "mailboxIds"
>;

/** New, unread, not junk, in the inbox, and recent. */
export const worthNotifying = (
  emails: readonly ArrivedEmail[],
  inboxId: MailboxId,
  now: number
): ArrivedEmail[] =>
  emails.filter(
    (email) =>
      email.mailboxIds[inboxId] === true &&
      email.keywords.$seen !== true &&
      email.keywords.$junk !== true &&
      now - Date.parse(email.receivedAt) < MAX_AGE_MS
  );

export interface NotificationContent {
  readonly title: string;
  readonly body: string;
  /** Same email → replaces instead of stacking. */
  readonly tag: string;
  readonly emailId: EmailId | null;
}

/** Sender as the title and subject as the text; no body, no addresses beyond the sender's. */
export const notificationContent = (
  emails: readonly ArrivedEmail[],
  labels: {
    readonly noSubject: string;
    readonly many: (count: number) => string;
  }
): NotificationContent[] => {
  if (emails.length > MAX_SINGLE) {
    return [
      {
        body: "",
        emailId: null,
        tag: "wm-new-mail",
        title: labels.many(emails.length),
      },
    ];
  }
  return emails.map((email) => {
    const [sender] = email.from ?? [];
    const subject = cleanDisplayText(email.subject ?? "");
    const name = cleanDisplayText(sender?.name ?? "");
    return {
      body: subject === "" ? labels.noSubject : subject,
      emailId: email.id,
      tag: `wm-${email.id}`,
      title: name === "" ? cleanDisplayText(sender?.email ?? "") : name,
    };
  });
};

/** The account's current Email state, to ask for changes from. */
export const emailState = async (client: JmapClient): Promise<State> => {
  const [result] = await client.request((b) => [
    b.call("Email/get", { accountId: client.primaryAccountId(), ids: [] }),
  ]);
  return result.state;
};

/**
 * Emails created since `since`, and the state to use next time. Throws on
 * `cannotCalculateChanges`; the caller starts over from the current state.
 */
export const createdSince = async (
  client: JmapClient,
  since: State
): Promise<{ readonly state: State; readonly emails: ArrivedEmail[] }> => {
  const accountId = client.primaryAccountId();
  const [changes, emails] = await client.request((b) => {
    const c = b.call("Email/changes", {
      accountId,
      maxChanges: 50,
      sinceState: since,
    });
    const g = b.call("Email/get", {
      "#ids": c.ref("/created"),
      accountId,
      properties: ["from", "subject", "receivedAt", "keywords", "mailboxIds"],
    });
    return [c, g] as const;
  });
  return { emails: emails.list, state: changes.newState };
};
