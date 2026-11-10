// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Nextcloud's calendar extras through the bridge
// publishing a calendar at a public link, webcal
// subscriptions and the trash bin. Only where the account's server offers
// them (the bridge's account capability); elsewhere nothing shows. The
// web never opens a public link or a subscription URL: it only shows them
// (the subscription is fetched by the DAV server).

import type {
  AccountId,
  CalendarId,
  JmapClient,
  SetError,
} from "@nebulamail/jmap-client";
import { BRIDGE_CALENDAR, CAPABILITIES } from "@nebulamail/jmap-client";

import { getClient } from "../mail/client";
import { chunk } from "../mail/thread-ids";
import { isSafeColor } from "./colors";
import type { CalendarPublishing, CalendarSummary } from "./model";

/** Longest subscription URL the bridge takes. */
export const MAX_SUBSCRIPTION_URL = 2048;

/** What the account's server offers. */
export interface CalendarExtras {
  readonly publishing: boolean;
  readonly subscriptions: boolean;
  readonly trash: boolean;
}

export const NO_EXTRAS: CalendarExtras = {
  publishing: false,
  subscriptions: false,
  trash: false,
};

const flag = (object: unknown, key: string): boolean =>
  typeof object === "object" &&
  object !== null &&
  Reflect.get(object, key) === true;

/** The bridge's account capability of the user's own calendars account. */
export const extrasOf = (client: JmapClient): CalendarExtras => {
  const own = client.primaryAccountId(CAPABILITIES.calendars);
  const capability: unknown =
    client.session.accounts[own]?.accountCapabilities[CAPABILITIES.bridge];
  return {
    publishing: flag(capability, "calendarPublishing"),
    subscriptions: flag(capability, "calendarSubscriptions"),
    trash: flag(capability, "calendarTrash"),
  };
};

export const calendarExtras = async (): Promise<CalendarExtras> =>
  extrasOf(await getClient());

/** Only `http(s)` links are shown (anything else: none). */
const shownUrl = (value: unknown): string | null => {
  if (typeof value !== "string" || value.length > MAX_SUBSCRIPTION_URL) {
    return null;
  }
  try {
    const url = new URL(value);
    return ["http:", "https:", "webcal:", "webcals:"].includes(url.protocol)
      ? url.href
      : null;
  } catch {
    return null;
  }
};

/** A calendar's publishing, by the bridge's word (null: not offered). */
export const publishingOf = (calendar: object): CalendarPublishing | null => {
  const published: unknown = Reflect.get(calendar, BRIDGE_CALENDAR.isPublished);
  if (typeof published !== "boolean") {
    return null;
  }
  const url = published
    ? shownUrl(Reflect.get(calendar, BRIDGE_CALENDAR.publicUrl))
    : null;
  return { published: published && url !== null, url };
};

/** A subscription's source (null: not a subscription). */
export const subscriptionOf = (
  calendar: object
): { readonly url: string | null } | null =>
  Object.hasOwn(calendar, BRIDGE_CALENDAR.subscriptionUrl)
    ? { url: shownUrl(Reflect.get(calendar, BRIDGE_CALENDAR.subscriptionUrl)) }
    : null;

/** Why a URL can't be subscribed to (null: it can). */
export type SubscriptionUrlProblem = "empty" | "scheme" | "credentials" | null;

/**
 * Checks a URL as the bridge does, to say it before sending: `https`,
 * `webcal` or `webcals`, with a host and no user or password.
 */
export const subscriptionUrlProblem = (
  text: string
): SubscriptionUrlProblem => {
  const value = text.trim();
  if (value === "") {
    return "empty";
  }
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return "scheme";
  }
  if (
    !["https:", "webcal:", "webcals:"].includes(url.protocol) ||
    url.hostname === "" ||
    value.length > MAX_SUBSCRIPTION_URL ||
    /\s/u.test(value)
  ) {
    return "scheme";
  }
  if (url.username !== "" || url.password !== "") {
    return "credentials";
  }
  return null;
};

/** A refused write, with the server's error type. */
export class ExtrasError extends Error {
  readonly type: string;
  constructor(error: SetError | undefined) {
    super(error?.description ?? error?.type ?? "unknown");
    this.name = "ExtrasError";
    this.type = error?.type ?? "unknown";
  }
}

/** Publishes a calendar of the user's own, or stops publishing it. */
export const setPublished = async (
  calendar: Pick<CalendarSummary, "accountId" | "id">,
  published: boolean
): Promise<void> => {
  const client = await getClient();
  const { accountId, id } = calendar;
  const [result] = await client.request((b) => {
    b.using(CAPABILITIES.bridge);
    return [
      b.call("Calendar/set", {
        accountId,
        update: {
          [id]: {
            "https:~1~1helixnebula.tech~1jmap~1bridge~1isPublished": published,
          },
        },
      }),
    ] as const;
  });
  const error = result.notUpdated[id];
  if (error !== undefined) {
    throw new ExtrasError(error);
  }
};

/** Subscribes to a calendar URL; the server fetches it. Returns its id. */
export const subscribe = async (input: {
  readonly name: string;
  readonly url: string;
  readonly color: string;
}): Promise<CalendarId> => {
  const client = await getClient();
  const accountId = client.primaryAccountId(CAPABILITIES.calendars);
  const [result] = await client.request((b) => {
    b.using(CAPABILITIES.bridge);
    return [
      b.call("Calendar/set", {
        accountId,
        create: {
          subscription: {
            "https://helixnebula.tech/jmap/bridge/subscriptionUrl":
              input.url.trim(),
            ...(isSafeColor(input.color) ? { color: input.color } : {}),
            isSubscribed: true,
            isVisible: true,
            name: input.name.trim(),
          },
        },
      }),
    ] as const;
  });
  const created = result.created.subscription;
  if (created === undefined) {
    throw new ExtrasError(result.notCreated.subscription);
  }
  return created.id;
};

/** Something in the trash bin. */
export interface TrashItem {
  readonly id: string;
  readonly kind: "calendar" | "event";
  /** Name or title (from other people: show with `cleanDisplayText`). */
  readonly name: string;
  readonly color: string | null;
  /** Epoch ms. */
  readonly deletedAt: number | null;
  /** Events: their first start (epoch ms). */
  readonly start: number | null;
  /** Events: the calendar they were in, if it is still there. */
  readonly calendarId: string | null;
}

export interface Trash {
  /** How long things stay, in days (null: the server doesn't say). */
  readonly retentionDays: number | null;
  readonly items: readonly TrashItem[];
}

const time = (value: string | null | undefined): number | null => {
  if (value === null || value === undefined) {
    return null;
  }
  const ms = Date.parse(value);
  return Number.isNaN(ms) ? null : ms;
};

/** The trash bin now (asked to the server each time). */
export const getTrash = async (): Promise<Trash> => {
  const client = await getClient();
  const accountId: AccountId = client.primaryAccountId(CAPABILITIES.calendars);
  const [bin] = await client.request((b) => [
    b.call("CalendarTrash/get", { accountId }),
  ]);
  const items: TrashItem[] = [
    ...bin.calendars.map((calendar) => ({
      calendarId: null,
      color:
        typeof calendar.color === "string" && isSafeColor(calendar.color)
          ? calendar.color
          : null,
      deletedAt: time(calendar.deletedAt),
      id: calendar.id,
      kind: "calendar" as const,
      name: calendar.name ?? "",
      start: null,
    })),
    ...bin.events.map((event) => ({
      calendarId: event.calendarId ?? null,
      color: null,
      deletedAt: time(event.deletedAt),
      id: event.id,
      kind: "event" as const,
      name: event.title ?? "",
      start: time(event.utcStart),
    })),
  ];
  const seconds = bin.retentionSeconds ?? null;
  return {
    items: items.toSorted((a, b) => (b.deletedAt ?? 0) - (a.deletedAt ?? 0)),
    retentionDays: seconds === null ? null : Math.round(seconds / 86_400),
  };
};

/** Restores items, or deletes them for good. Throws if any failed. */
export const changeTrash = async (
  action: "restore" | "destroy",
  ids: readonly string[]
): Promise<void> => {
  const client = await getClient();
  const accountId = client.primaryAccountId(CAPABILITIES.calendars);
  // RFC 8620 §5.3: at most maxObjectsInSet objects per /set call.
  let first: SetError | undefined;
  for (const part of chunk(ids, client.session.core.maxObjectsInSet)) {
    // oxlint-disable-next-line no-await-in-loop -- one after the other.
    const [result] = await client.request((b) => [
      b.call(
        "CalendarTrash/set",
        action === "restore"
          ? { accountId, restore: part }
          : { accountId, destroy: part }
      ),
    ]);
    const failed =
      action === "restore" ? result.notRestored : result.notDestroyed;
    first ??= Object.values(failed)[0];
  }
  if (first !== undefined) {
    throw new ExtrasError(first);
  }
};
