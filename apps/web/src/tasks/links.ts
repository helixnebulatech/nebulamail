// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Tasks made from an email, and the links tasks carry.
// Pure, with tests.
//
// A task from an email gets the subject as its title, "From: … · date" as
// its notes and two links: `mid:<message-id>` (RFC 2392: any client can
// find the message) and this app's URL of the email (opens it here). A
// task's links are untrusted (other clients write them): only our own
// email URLs open inside the app; web links go through the same
// confirmation as links in mail; anything else is shown as text.

import type { AccountId, CalendarEventId } from "@nebulamail/jmap-client";

/** `<id@host>` or `id@host` → "id@host" (null if empty). */
const bareMessageId = (messageId: string): string | null => {
  const bare = messageId.trim().replace(/^</u, "").replace(/>$/u, "").trim();
  return bare === "" ? null : bare;
};

/**
 * RFC 2392 `mid:` URL of a Message-ID: the id without angle brackets,
 * percent-encoded (`@` may stay as it is).
 */
export const midUrl = (messageId: string): string | null => {
  const bare = bareMessageId(messageId);
  return bare === null
    ? null
    : `mid:${encodeURIComponent(bare).replaceAll("%40", "@")}`;
};

/** Path of an email in this app: `/mail/<folder>/<email>`. */
const EMAIL_PATH = /^\/mail\/[\w.-]{1,512}\/[\w-]{1,255}$/u;

export type TaskLinkKind =
  /** Our own URL of an email: opens it in the app (`path`). */
  | { readonly kind: "email"; readonly path: string }
  /** A Message-ID (`mid:`): no way to open it here. */
  | { readonly kind: "mid"; readonly messageId: string }
  | { readonly kind: "web"; readonly href: string }
  | { readonly kind: "other" };

/** What a task's link is, for this app at `origin`. */
export const classifyLink = (href: string, origin: string): TaskLinkKind => {
  const text = href.trim();
  if (/^mid:/iu.test(text)) {
    try {
      // "mid:<message-id>/<content-id>": the part after "/" is a body part.
      const encoded = text.slice(4).split("/")[0] ?? "";
      const messageId = decodeURIComponent(encoded);
      return messageId === "" ? { kind: "other" } : { kind: "mid", messageId };
    } catch {
      return { kind: "other" };
    }
  }
  let url: URL;
  try {
    url = new URL(text);
  } catch {
    return { kind: "other" };
  }
  if (url.origin === origin && EMAIL_PATH.test(url.pathname)) {
    return { kind: "email", path: url.pathname };
  }
  if (url.protocol === "https:" || url.protocol === "http:") {
    return { href: url.href, kind: "web" };
  }
  return { kind: "other" };
};

export interface EmailForTask {
  readonly subject: string;
  readonly from: {
    readonly name: string | null;
    readonly email: string;
  } | null;
  /** Epoch ms it was received. */
  readonly receivedAt: number | null;
  readonly messageIds: readonly string[];
  /** Folder segment of its URL (`mail/<folder>/<email>`). */
  readonly folder: string;
  readonly emailId: string;
}

export interface EmailTaskWords {
  /** "From: Alice <alice@example.com> · 27 Oct 2026, 10:00". */
  readonly notes: (sender: string, date: string) => string;
  /** "Email: <subject>" (the title of the app link). */
  readonly linkTitle: (subject: string) => string;
  readonly noSubject: string;
  readonly formatDate: (ms: number) => string;
}

export interface EmailTaskInput {
  readonly title: string;
  readonly notes: string;
  readonly links: readonly { readonly href: string; readonly title: string }[];
}

/** One line of untrusted text: no line breaks, cut to `max`. */
const oneLine = (text: string, max: number): string =>
  text.replaceAll(/\s+/gu, " ").trim().slice(0, max);

/** The new task for an email: title, notes and its two links. */
export const taskFromEmail = (
  email: EmailForTask,
  origin: string,
  words: EmailTaskWords
): EmailTaskInput => {
  const subject = oneLine(email.subject, 500);
  const sender =
    email.from === null
      ? ""
      : oneLine(
          email.from.name === null || email.from.name === ""
            ? email.from.email
            : `${email.from.name} <${email.from.email}>`,
          300
        );
  const date =
    email.receivedAt === null ? "" : words.formatDate(email.receivedAt);
  const mids = email.messageIds.flatMap((id) => {
    const url = midUrl(id);
    return url === null ? [] : [url];
  });
  const path = `/mail/${encodeURIComponent(email.folder)}/${encodeURIComponent(email.emailId)}`;
  return {
    links: [
      ...(mids[0] === undefined
        ? []
        : [{ href: mids[0], title: subject || words.noSubject }]),
      {
        href: `${origin}${path}`,
        title: words.linkTitle(subject || words.noSubject),
      },
    ],
    notes: sender === "" && date === "" ? "" : words.notes(sender, date),
    title: subject === "" ? words.noSubject : subject,
  };
};

// ---------------------------------------------------------------- URL keys

/**
 * A task or list in the URL: its id for the user's own account,
 * `<account>.<id>` for one shared by others (ids never have a ".", like
 * contacts and shared folders).
 */
export const refParam = (
  accountId: string,
  id: string,
  own: string | null
): string => (accountId === own ? id : `${accountId}.${id}`);

const ID = /^[\w-]{1,255}$/u;

/** Reads `refParam` back (null if malformed). */
export const parseRefParam = (
  value: string | undefined,
  own: string | null
): { readonly accountId: string; readonly id: string } | null => {
  if (value === undefined) {
    return null;
  }
  const [first, second, ...rest] = value.split(".");
  if (rest.length > 0 || first === undefined || !ID.test(first)) {
    return null;
  }
  if (second === undefined) {
    return own === null ? null : { accountId: own, id: first };
  }
  return ID.test(second) ? { accountId: first, id: second } : null;
};

/** The ids of a task the URL points at, typed. */
export interface TaskRef {
  readonly accountId: AccountId;
  readonly id: CalendarEventId;
}
