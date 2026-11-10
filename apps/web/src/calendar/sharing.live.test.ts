// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Sharing a calendar against a real Stalwart,
// with the exact requests of the share dialog (calendar/sharing.ts): alice
// shares a temporary calendar with bob at each level, bob sees it (or
// not) in alice's account with those rights, an unknown person is
// refused, and unsharing hides it again. Sends no email. Skipped unless
// these are set (passwords in docker/.env); CI runs it:
//
//   CAL_LIVE_URL=http://localhost:8080 \
//   CAL_LIVE_USER=alice@nebulamail.test CAL_LIVE_PASSWORD=… \
//   CAL_LIVE_ORGANIZER=bob@nebulamail.test CAL_LIVE_ORGANIZER_PASSWORD=… \
//   bun run test
//
// The temporary calendar is deleted again.

import type { FetchLike, JmapClient } from "@nebulamail/jmap-client";
import {
  CalendarId,
  JmapClient as Client,
  PrincipalId,
} from "@nebulamail/jmap-client";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { calendarsOf } from "./api";
import type { SharedCalendarRef } from "./sharing";
import { calendarSharesOf, shareCalendarWith, shareFailure } from "./sharing";

const env = {
  other: process.env.CAL_LIVE_ORGANIZER ?? "",
  otherPassword: process.env.CAL_LIVE_ORGANIZER_PASSWORD ?? "",
  password: process.env.CAL_LIVE_PASSWORD ?? "",
  url: process.env.CAL_LIVE_URL ?? "",
  user: process.env.CAL_LIVE_USER ?? "",
};

const configured = Object.values(env).every((value) => value !== "");

/** Stalwart takes basic auth too; swap the client's Bearer header for it. */
const basicFetch =
  (user: string, password: string): FetchLike =>
  async (input, init) => {
    const headers = new Headers(init?.headers);
    headers.set("Authorization", `Basic ${btoa(`${user}:${password}`)}`);
    return await fetch(input, { ...init, headers });
  };

const connect = async (user: string, password: string): Promise<JmapClient> =>
  await Client.connect({
    fetch: basicFetch(user, password),
    getAccessToken: async () => await Promise.resolve("unused"),
    sessionUrl: `${env.url}/.well-known/jmap`,
  });

const CALENDARS = "urn:ietf:params:jmap:calendars";
const NAME = `nebulashare${Date.now().toString(36)}`;

describe.skipIf(!configured)("sharing a calendar (live Stalwart)", () => {
  let owner: JmapClient;
  let other: JmapClient;
  let calendar: SharedCalendarRef | null = null;
  let otherId: PrincipalId;

  beforeAll(async () => {
    owner = await connect(env.user, env.password);
    other = await connect(env.other, env.otherPassword);
    const accountId = owner.primaryAccountId(CALENDARS);
    const [created] = await owner.request((b) => [
      b.call("Calendar/set", {
        accountId,
        create: { temp: { isVisible: true, name: NAME } },
      }),
    ]);
    const id = created.created.temp?.id;
    if (id === undefined) {
      throw new Error("Could not create the temporary calendar");
    }
    calendar = { accountId, id: CalendarId.parse(id) };
    // The person, as the dialog finds them (Principal/query + get).
    const [, people] = await owner.request((b) => {
      const query = b.call("Principal/query", { accountId });
      const get = b.call("Principal/get", {
        "#ids": query.ref("/ids"),
        accountId,
        properties: ["name", "email", "type"],
      });
      return [query, get] as const;
    });
    const person = people.list.find(
      (principal) => principal.email === env.other
    );
    if (person === undefined) {
      throw new Error("The other user isn't in the directory");
    }
    otherId = person.id;
  });

  afterAll(async () => {
    if (calendar === null) {
      return;
    }
    const { accountId, id } = calendar;
    await owner.request((b) => [
      b.call("Calendar/set", {
        accountId,
        destroy: [id],
        onDestroyRemoveEvents: true,
      }),
    ]);
  });

  const ref = (): SharedCalendarRef => {
    if (calendar === null) {
      throw new Error("No calendar");
    }
    return calendar;
  };

  /** The calendar as the other user sees it now (undefined: not at all). */
  const seenByOther = async () => {
    await other.refreshSession();
    const calendars = await calendarsOf(other);
    return calendars.find(
      (item) => item.accountId === ref().accountId && item.id === ref().id
    );
  };

  /** The other user's level as the owner reads it now. */
  const levelNow = async () => {
    const [share] = await calendarSharesOf(owner, ref(), []);
    return share?.level;
  };

  it("offers sharing on our own calendar", async () => {
    const calendars = await calendarsOf(owner);
    const ours = calendars.find((item) => item.id === ref().id);
    expect(ours?.mayShare).toBeTruthy();
    await expect(calendarSharesOf(owner, ref(), [])).resolves.toStrictEqual([]);
  });

  it("shares to read: they see it, read only", async () => {
    await shareCalendarWith(owner, ref(), otherId, "read");
    const [share] = await calendarSharesOf(owner, ref(), []);
    expect(share).toMatchObject({ level: "read", person: { id: otherId } });
    const seen = await seenByOther();
    expect(seen).toMatchObject({ mayShare: false, mayWrite: false });
    expect(seen?.owner).not.toBeNull();
  });

  it("changes to write: they may write", async () => {
    await shareCalendarWith(owner, ref(), otherId, "write");
    await expect(levelNow()).resolves.toBe("write");
    const seen = await seenByOther();
    expect(seen).toMatchObject({ mayShare: false, mayWrite: true });
  });

  it("changes to manage: they may share it too", async () => {
    await shareCalendarWith(owner, ref(), otherId, "manage");
    await expect(levelNow()).resolves.toBe("manage");
    const seen = await seenByOther();
    expect(seen).toMatchObject({ mayShare: true, mayWrite: true });
  });

  it("changes to free/busy: they don't get the calendar", async () => {
    // Stalwart doesn't give them the account at all.
    await shareCalendarWith(owner, ref(), otherId, "freeBusy");
    await expect(levelNow()).resolves.toBe("freeBusy");
    await expect(seenByOther()).resolves.toBeUndefined();
  });

  it("refuses someone the server doesn't know", async () => {
    const failure = await shareCalendarWith(
      owner,
      ref(),
      PrincipalId.parse("nobody-here"),
      "read"
    ).catch((error: unknown) => error);
    expect(shareFailure(failure)).toBe("invalidProperties");
  });

  it("stops sharing", async () => {
    await shareCalendarWith(owner, ref(), otherId, "read");
    await expect(seenByOther()).resolves.toBeDefined();
    await shareCalendarWith(owner, ref(), otherId, null);
    await expect(calendarSharesOf(owner, ref(), [])).resolves.toStrictEqual([]);
    await expect(seenByOther()).resolves.toBeUndefined();
  });
});
