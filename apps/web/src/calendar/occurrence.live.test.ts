// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// An invitation to ONE occurrence of a series against a real Stalwart
//bob invites alice to a weekly series, moves
// one occurrence without scheduling and emails her a REQUEST for it (as
// an organizer whose server doesn't schedule would). Our code reads it
// (CalendarEvent/parse + analyzeInvitation: RECURRENCE-ID, "rsvp"), and
// the REPLY we build for it, sent from alice, updates only that
// occurrence in bob's calendar. Skipped unless these are set (passwords
// in docker/.env); CI runs it (.github/workflows/ci.yml):
//
//   CAL_LIVE_URL=http://localhost:8080 \
//   CAL_LIVE_USER=alice@nebulamail.test CAL_LIVE_PASSWORD=… \
//   CAL_LIVE_ORGANIZER=bob@nebulamail.test CAL_LIVE_ORGANIZER_PASSWORD=… \
//   CAL_LIVE_DELEGATE=carol@nebulamail.test CAL_LIVE_DELEGATE_PASSWORD=… \
//   bun run test
//
// (CAL_LIVE_DELEGATE only for delegating a task; without it that part is
// skipped.)
//
// Everything it creates (events of both, the emails) is deleted again.

import { setTimeout as sleep } from "node:timers/promises";

import type {
  AccountId,
  BlobId,
  CalendarEventId,
  FetchLike,
  JmapClient,
  RecurrenceRule,
} from "@nebulamail/jmap-client";
import { CalendarId, JmapClient as Client } from "@nebulamail/jmap-client";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

import type { TaskAssignee } from "../tasks/assignment";
import {
  assigneesPatch,
  assignmentOf,
  delegationOf,
  taskToImport,
} from "../tasks/assignment";
import { newTaskProperties } from "../tasks/editing";
import { taskSeries } from "../tasks/itip-series";
import type { TaskDue } from "../tasks/model";
import type { Invitation } from "./itip";
import {
  analyzeInvitation,
  buildOccurrenceRequest,
  buildReply,
  buildTaskDelegation,
  buildTaskReply,
  buildTaskRequest,
  cardAction,
} from "./itip";

const env = {
  organizer: process.env.CAL_LIVE_ORGANIZER ?? "",
  organizerPassword: process.env.CAL_LIVE_ORGANIZER_PASSWORD ?? "",
  password: process.env.CAL_LIVE_PASSWORD ?? "",
  url: process.env.CAL_LIVE_URL ?? "",
  user: process.env.CAL_LIVE_USER ?? "",
};

const configured = Object.values(env).every((value) => value !== "");

/** A third account, for delegating a task (optional; CAL_LIVE_DELEGATE). */
const delegateEnv = {
  delegate: process.env.CAL_LIVE_DELEGATE ?? "",
  delegatePassword: process.env.CAL_LIVE_DELEGATE_PASSWORD ?? "",
};
const delegateConfigured =
  configured && Object.values(delegateEnv).every((value) => value !== "");

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
const MARKER = `nebulaocc${Date.now().toString(36)}`;
const UID = `${MARKER}@nebulamail.test`;
const DAY_MS = 86_400_000;

/** A Monday a few weeks ahead (iTIP only for events still to come). */
const firstMonday = (): string => {
  const date = new Date(Date.now() + 21 * DAY_MS);
  while (date.getUTCDay() !== 1) {
    date.setUTCDate(date.getUTCDate() + 1);
  }
  return date.toISOString().slice(0, 10);
};
const FIRST = firstMonday();
const SECOND = new Date(Date.parse(`${FIRST}T00:00:00Z`) + 7 * DAY_MS)
  .toISOString()
  .slice(0, 10);
const compact = (day: string): string => day.replaceAll("-", "");
const nextDay = (day: string): string =>
  new Date(Date.parse(`${day}T00:00:00Z`) + DAY_MS).toISOString().slice(0, 10);
const stamp = (): string =>
  new Date()
    .toISOString()
    .replaceAll(/[-:]/gu, "")
    .replace(/\.\d{3}/u, "");

const eventually = async <T>(
  read: () => Promise<T | null>,
  what: () => string
): Promise<T> => {
  for (let tries = 0; tries < 60; tries += 1) {
    // oxlint-disable-next-line no-await-in-loop -- polling until the server delivers.
    const value = await read();
    if (value !== null) {
      return value;
    }
    // oxlint-disable-next-line no-await-in-loop -- polling.
    await sleep(500);
  }
  throw new Error(`Timed out: ${what()}`);
};

/** `{ [Sent]: true }` of an account (a sent email needs a mailbox). */
const sentOf = async (client: JmapClient): Promise<Record<string, true>> => {
  const [result] = await client.request((b) => [
    b.call("Mailbox/get", {
      accountId: client.primaryAccountId(),
      properties: ["role"],
    }),
  ]);
  const sent = result.list.find((mailbox) => mailbox.role === "sent");
  return sent === undefined ? {} : { [sent.id]: true };
};

/** Emails an iMIP message (text + text/calendar) from the client's user. */
const sendItip = async (
  client: JmapClient,
  message: {
    readonly from: string;
    readonly to: string;
    readonly method: string;
    readonly subject: string;
    readonly ics: string;
  }
): Promise<boolean> => {
  const accountId = client.primaryAccountId();
  const [identities] = await client.request((b) => [
    b.call("Identity/get", { accountId, properties: ["email"] }),
  ]);
  const identity = identities.list.find(
    (item) => item.email.toLowerCase() === message.from
  );
  if (identity === undefined) {
    return false;
  }
  const mailboxIds = await sentOf(client);
  const [, submission] = await client.request((b) => {
    const set = b.call("Email/set", {
      accountId,
      create: {
        itip: {
          bodyStructure: {
            subParts: [
              { partId: "text", type: "text/plain" },
              {
                partId: "ics",
                type: `text/calendar; method=${message.method}`,
              },
            ],
            type: "multipart/alternative",
          },
          bodyValues: {
            ics: { value: message.ics },
            text: { value: `${message.subject} (live test)` },
          },
          from: [{ email: message.from, name: null }],
          keywords: { $seen: true },
          mailboxIds,
          subject: message.subject,
          to: [{ email: message.to, name: null }],
        },
      },
    });
    const submit = b.call("EmailSubmission/set", {
      accountId,
      create: { send: { emailId: "#itip", identityId: identity.id } },
    });
    return [set, submit] as const;
  });
  return submission.created.send !== undefined;
};

/** Ids of the test's emails (the marker is one word of the subject). */
const markedEmails = async (client: JmapClient) => {
  const accountId = client.primaryAccountId();
  const [, emails] = await client.request((b) => {
    const query = b.call("Email/query", {
      accountId,
      filter: { subject: MARKER },
    });
    const get = b.call("Email/get", {
      "#ids": query.ref("/ids"),
      accountId,
      properties: ["subject", "attachments"],
    });
    return [query, get] as const;
  });
  return emails.list;
};

const removeEvents = async (
  client: JmapClient,
  accountId: AccountId,
  uid: string = UID
): Promise<void> => {
  const [, copies] = await client.request((b) => {
    const query = b.call("CalendarEvent/query", {
      accountId,
      filter: { uid },
    });
    const get = b.call("CalendarEvent/get", {
      "#ids": query.ref("/ids"),
      accountId,
      properties: ["uid"],
    });
    return [query, get] as const;
  });
  const ids = copies.list
    .filter((event) => event.uid === uid)
    .map((event) => event.id);
  if (ids.length > 0) {
    await client.request((b) => [
      b.call("CalendarEvent/set", { accountId, destroy: ids }),
    ]);
  }
};

const removeEmails = async (client: JmapClient): Promise<void> => {
  const emails = await markedEmails(client);
  const ids = emails.map((email) => email.id);
  if (ids.length > 0) {
    await client.request((b) => [
      b.call("Email/set", {
        accountId: client.primaryAccountId(),
        destroy: ids,
      }),
    ]);
  }
};

describe.skipIf(!configured)(
  "live Stalwart: invitation to one occurrence",
  () => {
    const bobAddress = env.organizer.toLowerCase();
    const aliceAddress = env.user.toLowerCase();
    const participants = {
      a: {
        "@type": "Participant",
        calendarAddress: `mailto:${aliceAddress}`,
        expectReply: true,
        participationStatus: "needs-action",
        roles: { attendee: true },
      },
      o: {
        "@type": "Participant",
        calendarAddress: `mailto:${bobAddress}`,
        participationStatus: "accepted",
        roles: { attendee: true, owner: true },
      },
    };
    const state: {
      bob?: JmapClient;
      alice?: JmapClient;
      seriesId?: CalendarEventId | undefined;
      invitation?: Invitation | null;
    } = {};

    const clients = (): {
      readonly bob: JmapClient;
      readonly alice: JmapClient;
    } => {
      const { alice, bob } = state;
      if (alice === undefined || bob === undefined) {
        throw new Error("Not connected");
      }
      return { alice, bob };
    };

    beforeAll(async () => {
      state.bob = await connect(env.organizer, env.organizerPassword);
      state.alice = await connect(env.user, env.password);
    });

    afterAll(async () => {
      const { alice, bob } = clients();
      await removeEvents(bob, bob.primaryAccountId(CALENDARS)).catch(
        () => null
      );
      await removeEvents(alice, alice.primaryAccountId(CALENDARS)).catch(
        () => null
      );
      await removeEmails(alice).catch(() => null);
      await removeEmails(bob).catch(() => null);
    });

    it("bob invites alice to a weekly series", async () => {
      const { bob } = clients();
      const accountId = bob.primaryAccountId(CALENDARS);
      const [calendars] = await bob.request((b) => [
        b.call("Calendar/get", { accountId }),
      ]);
      const calendarId = CalendarId.parse(
        (
          calendars.list.find((item) => item.isDefault === true) ??
          calendars.list[0]
        )?.id
      );
      const [created] = await bob.request((b) => [
        b.call("CalendarEvent/set", {
          accountId,
          create: {
            series: {
              calendarIds: { [calendarId]: true },
              duration: "PT1H",
              organizerCalendarAddress: `mailto:${bobAddress}`,
              participants,
              recurrenceRule: {
                "@type": "RecurrenceRule",
                count: 3,
                frequency: "weekly",
              },
              start: `${FIRST}T10:00:00`,
              timeZone: "Europe/Madrid",
              title: `${MARKER} weekly`,
              uid: UID,
            },
          },
          sendSchedulingMessages: true,
        }),
      ]);
      state.seriesId = created.created.series?.id;
      expect(state.seriesId).toBeDefined();
    });

    it("bob moves one occurrence and sends a REQUEST for it", async () => {
      const { bob } = clients();
      const accountId = bob.primaryAccountId(CALENDARS);
      const [, expanded] = await bob.request((b) => {
        const query = b.call("CalendarEvent/query", {
          accountId,
          expandRecurrences: true,
          filter: { after: `${SECOND}T00:00:00`, before: `${SECOND}T23:59:59` },
          timeZone: "Europe/Madrid",
        });
        const get = b.call("CalendarEvent/get", {
          "#ids": query.ref("/ids"),
          accountId,
          properties: ["baseEventId"],
        });
        return [query, get] as const;
      });
      const occurrence = expanded.list.find(
        (event) => event.baseEventId === state.seriesId
      );
      // Stored with the guests: Stalwart matches a REPLY to the
      // occurrence's own attendees (without them it ignores the answer).
      const [moved] = await bob.request((b) => [
        b.call("CalendarEvent/set", {
          accountId,
          update: {
            [occurrence?.id ?? "missing"]: {
              participants,
              start: `${SECOND}T11:00:00`,
              title: `${MARKER} moved`,
            },
          },
        }),
      ]);
      expect(Object.keys(moved.updated)).toHaveLength(1);
      // 10:00 Madrid in November is 09:00 UTC; it moves to 10:00 UTC.
      const ics = [
        "BEGIN:VCALENDAR",
        "VERSION:2.0",
        "PRODID:-//Nebula Mail//live test//EN",
        "METHOD:REQUEST",
        "BEGIN:VEVENT",
        `UID:${UID}`,
        `DTSTAMP:${stamp()}`,
        `RECURRENCE-ID:${compact(SECOND)}T090000Z`,
        `DTSTART:${compact(SECOND)}T100000Z`,
        "DURATION:PT1H",
        "SEQUENCE:0",
        `SUMMARY:${MARKER} moved`,
        `ORGANIZER:mailto:${bobAddress}`,
        `ATTENDEE;PARTSTAT=ACCEPTED:mailto:${bobAddress}`,
        `ATTENDEE;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:${aliceAddress}`,
        "END:VEVENT",
        "END:VCALENDAR",
        "",
      ].join("\r\n");
      await expect(
        sendItip(bob, {
          from: bobAddress,
          ics,
          method: "REQUEST",
          subject: `Updated invitation: ${MARKER} moved`,
          to: aliceAddress,
        })
      ).resolves.toBeTruthy();
    });

    it("alice's card reads it as that occurrence and offers to answer", async () => {
      const { alice } = clients();
      let seen: string[] = [];
      const blobId = await eventually(
        async (): Promise<BlobId | null> => {
          const emails = await markedEmails(alice);
          seen = emails.map((email) => email.subject ?? "");
          const email = emails.find(
            (item) => item.subject?.includes("moved") === true
          );
          const part = email?.attachments?.find(
            (attachment) =>
              attachment.type?.toLowerCase() === "text/calendar" ||
              attachment.name?.toLowerCase().endsWith(".ics") === true
          );
          return part?.blobId ?? null;
        },
        () => `no REQUEST for the occurrence; saw ${seen.join(" | ")}`
      );
      const accountId = alice.primaryAccountId(CALENDARS);
      const [parsed] = await alice.request((b) => [
        b.call("CalendarEvent/parse", { accountId, blobIds: [blobId] }),
      ]);
      state.invitation = analyzeInvitation(parsed.parsed[blobId] ?? [], [
        aliceAddress,
      ]);
      expect(state.invitation).toMatchObject({ method: "request", uid: UID });
      expect(state.invitation?.recurrenceId).toMatch(/^\d{4}-/u);
      expect(state.invitation?.me?.email).toBe(aliceAddress);
      expect(
        state.invitation === null || state.invitation === undefined
          ? null
          : cardAction(state.invitation, null, {
              email: bobAddress,
              verdict: null,
            })
      ).toBe("rsvp");
    });

    it("our REPLY updates only that occurrence in bob's calendar", async () => {
      const { alice, bob } = clients();
      const { invitation } = state;
      if (
        invitation === null ||
        invitation === undefined ||
        invitation.organizer === null
      ) {
        throw new Error("No invitation read");
      }
      const ics = buildReply({
        attendee: { email: aliceAddress, name: null },
        comment: "Only this one",
        now: Date.now(),
        organizer: invitation.organizer,
        recurrenceId: invitation.recurrenceId,
        recurrenceIdTimeZone: invitation.recurrenceIdTimeZone,
        sequence: invitation.sequence,
        status: "accepted",
        title: `${MARKER} moved`,
        uid: UID,
      });
      expect(ics).toContain(`RECURRENCE-ID:${compact(SECOND)}T090000Z`);
      await expect(
        sendItip(alice, {
          from: aliceAddress,
          ics,
          method: "REPLY",
          subject: `Accepted: ${MARKER} moved`,
          to: bobAddress,
        })
      ).resolves.toBeTruthy();
      const accountId = bob.primaryAccountId(CALENDARS);
      const aliceIn = <
        P extends { readonly calendarAddress?: string | null | undefined },
      >(
        people: Readonly<Record<string, P>> | null
      ): P | undefined =>
        Object.values(people ?? {}).find(
          (participant) =>
            participant.calendarAddress?.toLowerCase() ===
            `mailto:${aliceAddress}`
        );
      let last = "";
      // Read as the app does, the occurrence expanded by the server, which
      // keeps the override as a patch ("participants/a/…").
      await eventually(
        async () => {
          const [, expanded] = await bob.request((b) => {
            const query = b.call("CalendarEvent/query", {
              accountId,
              expandRecurrences: true,
              filter: {
                after: `${SECOND}T00:00:00`,
                before: `${SECOND}T23:59:59`,
              },
              timeZone: "Europe/Madrid",
            });
            const get = b.call("CalendarEvent/get", {
              "#ids": query.ref("/ids"),
              accountId,
              properties: ["baseEventId", "participants"],
            });
            return [query, get] as const;
          });
          const occurrence = expanded.list.find(
            (item) => item.baseEventId === state.seriesId
          );
          last = JSON.stringify(occurrence ?? {});
          return aliceIn(occurrence?.participants ?? null)
            ?.participationStatus === "accepted"
            ? true
            : null;
        },
        () => `bob's occurrence never got the answer: ${last}`
      );
      const [series] = await bob.request((b) => [
        b.call("CalendarEvent/get", {
          accountId,
          ids: state.seriesId === undefined ? [] : [state.seriesId],
          properties: ["participants"],
        }),
      ]);
      // The series itself still waits for her answer.
      expect(
        aliceIn(series.list[0]?.participants ?? null)?.participationStatus
      ).toBe("needs-action");
    }, 60_000);

    // Stalwart sends the REQUEST for ONE
    // changed occurrence itself (RFC 5546 §3.2.2 with RECURRENCE-ID), with
    // SEQUENCE raised (RFC 5546 §2.1.4): the app sends no second one, and
    // accepting a COUNTER for one occurrence needs nothing more either.
    it("Stalwart sends the REQUEST for one changed occurrence itself", async () => {
      const { alice, bob } = clients();
      const accountId = bob.primaryAccountId(CALENDARS);
      const third = new Date(Date.parse(`${SECOND}T00:00:00Z`) + 7 * DAY_MS)
        .toISOString()
        .slice(0, 10);
      const [, expanded] = await bob.request((b) => {
        const query = b.call("CalendarEvent/query", {
          accountId,
          expandRecurrences: true,
          filter: { after: `${third}T00:00:00`, before: `${third}T23:59:59` },
          timeZone: "Europe/Madrid",
        });
        const get = b.call("CalendarEvent/get", {
          "#ids": query.ref("/ids"),
          accountId,
          properties: ["baseEventId", "sequence"],
        });
        return [query, get] as const;
      });
      const occurrence = expanded.list.find(
        (event) => event.baseEventId === state.seriesId
      );
      const before = occurrence?.sequence ?? 0;
      const [moved] = await bob.request((b) => [
        b.call("CalendarEvent/set", {
          accountId,
          sendSchedulingMessages: true,
          update: {
            [occurrence?.id ?? "missing"]: {
              start: `${third}T12:00:00`,
              title: `${MARKER} rescheduled`,
            },
          },
        }),
      ]);
      expect(Object.keys(moved.updated)).toHaveLength(1);
      const aliceAccount = alice.primaryAccountId(CALENDARS);
      let seen: string[] = [];
      const parsed = await eventually(
        async () => {
          const emails = await markedEmails(alice);
          seen = emails.map((email) => email.subject ?? "");
          const part = emails
            .find((item) => item.subject?.includes("rescheduled") === true)
            ?.attachments?.find(
              (attachment) => attachment.type?.toLowerCase() === "text/calendar"
            );
          const blobId = part?.blobId;
          if (blobId === undefined || blobId === null) {
            return null;
          }
          const [result] = await alice.request((b) => [
            b.call("CalendarEvent/parse", {
              accountId: aliceAccount,
              blobIds: [blobId],
            }),
          ]);
          return result.parsed[blobId] ?? null;
        },
        () => `Stalwart sent no REQUEST; saw ${seen.join(" | ")}`
      );
      const invitation = analyzeInvitation(parsed, [aliceAddress]);
      expect(invitation).toMatchObject({ method: "request", uid: UID });
      expect(invitation?.recurrenceId).toMatch(new RegExp(`^${third}T`, "u"));
      expect(invitation?.sequence).toBeGreaterThan(before);
    }, 60_000);
  }
);

// ALL-DAY series. Changing one occurrence keeps the series
// (support.stalw.art/t/1714), so the app offers "This event" for them
// (`canChangeOneOccurrence` in editing.ts). Deleting
// one occurrence works (an excluded override). The REQUEST the app builds
// for one all-day occurrence (DATE values) is read back as that day.
describe.skipIf(!configured)("live Stalwart: one all-day occurrence", () => {
  const DAY_UID = `${MARKER}-day@nebulamail.test`;
  const bobAddress = env.organizer.toLowerCase();
  const aliceAddress = env.user.toLowerCase();
  const participants = {
    a: {
      "@type": "Participant",
      calendarAddress: `mailto:${aliceAddress}`,
      expectReply: true,
      participationStatus: "needs-action",
      roles: { attendee: true },
    },
    o: {
      "@type": "Participant",
      calendarAddress: `mailto:${bobAddress}`,
      participationStatus: "accepted",
      roles: { attendee: true, owner: true },
    },
  };
  const state: { bob?: JmapClient; alice?: JmapClient } = {};

  const clients = (): {
    readonly bob: JmapClient;
    readonly alice: JmapClient;
  } => {
    const { alice, bob } = state;
    if (alice === undefined || bob === undefined) {
      throw new Error("Not connected");
    }
    return { alice, bob };
  };

  /** Bob's all-day weekly series and its second occurrence's id. */
  const allDaySeries = async (): Promise<{
    readonly seriesId: CalendarEventId;
    readonly occurrenceId: CalendarEventId;
    readonly recurrenceId: string;
  }> => {
    const { bob } = clients();
    const accountId = bob.primaryAccountId(CALENDARS);
    const [calendars] = await bob.request((b) => [
      b.call("Calendar/get", { accountId }),
    ]);
    const calendarId = CalendarId.parse(
      (
        calendars.list.find((item) => item.isDefault === true) ??
        calendars.list[0]
      )?.id
    );
    const [created] = await bob.request((b) => [
      b.call("CalendarEvent/set", {
        accountId,
        create: {
          series: {
            calendarIds: { [calendarId]: true },
            duration: "P1D",
            organizerCalendarAddress: `mailto:${bobAddress}`,
            participants,
            recurrenceRule: {
              "@type": "RecurrenceRule",
              count: 3,
              frequency: "weekly",
            },
            showWithoutTime: true,
            start: `${FIRST}T00:00:00`,
            title: `${MARKER} offsite`,
            uid: DAY_UID,
          },
        },
      }),
    ]);
    const seriesId = created.created.series?.id;
    const [, expanded] = await bob.request((b) => {
      const query = b.call("CalendarEvent/query", {
        accountId,
        expandRecurrences: true,
        filter: { after: `${SECOND}T00:00:00`, before: `${SECOND}T23:59:59` },
        timeZone: "Etc/UTC",
      });
      const get = b.call("CalendarEvent/get", {
        "#ids": query.ref("/ids"),
        accountId,
        properties: ["baseEventId", "recurrenceId"],
      });
      return [query, get] as const;
    });
    const occurrence = expanded.list.find(
      (event) => event.baseEventId === seriesId
    );
    if (seriesId === undefined || occurrence === undefined) {
      throw new Error("No all-day series or occurrence");
    }
    return {
      occurrenceId: occurrence.id,
      recurrenceId: occurrence.recurrenceId ?? `${SECOND}T00:00:00`,
      seriesId,
    };
  };

  const seriesNow = async (seriesId: CalendarEventId) => {
    const { bob } = clients();
    const [result] = await bob.request((b) => [
      b.call("CalendarEvent/get", {
        accountId: bob.primaryAccountId(CALENDARS),
        ids: [seriesId],
        properties: ["recurrenceRule", "recurrenceOverrides", "start"],
      }),
    ]);
    return result.list[0];
  };

  beforeAll(async () => {
    state.bob = await connect(env.organizer, env.organizerPassword);
    state.alice = await connect(env.user, env.password);
  });

  afterAll(async () => {
    const { alice, bob } = clients();
    await removeEvents(bob, bob.primaryAccountId(CALENDARS), DAY_UID).catch(
      () => null
    );
    // Stalwart adds invitations from users of the server by itself.
    await removeEvents(alice, alice.primaryAccountId(CALENDARS), DAY_UID).catch(
      () => null
    );
    await removeEmails(alice).catch(() => null);
    await removeEmails(bob).catch(() => null);
  });

  it("changing one day keeps the series", async () => {
    const { bob } = clients();
    const { occurrenceId, seriesId } = await allDaySeries();
    try {
      const [changed] = await bob.request((b) => [
        b.call("CalendarEvent/set", {
          accountId: bob.primaryAccountId(CALENDARS),
          update: {
            [occurrenceId]: {
              participants,
              start: `${nextDay(SECOND)}T00:00:00`,
            },
          },
        }),
      ]);
      expect(Object.keys(changed.updated ?? {})).toStrictEqual([occurrenceId]);
      const series = await seriesNow(seriesId);
      expect(series?.recurrenceRule).toMatchObject({
        count: 3,
        frequency: "weekly",
      });
      expect(series?.start).toBe(`${FIRST}T00:00:00`);
      expect(series?.recurrenceOverrides).toMatchObject({
        [`${SECOND}T00:00:00`]: { start: `${nextDay(SECOND)}T00:00:00` },
      });
    } finally {
      await removeEvents(bob, bob.primaryAccountId(CALENDARS), DAY_UID);
    }
  });

  it("deleting one all-day occurrence keeps the series", async () => {
    const { bob } = clients();
    const { occurrenceId, seriesId } = await allDaySeries();
    try {
      const [gone] = await bob.request((b) => [
        b.call("CalendarEvent/set", {
          accountId: bob.primaryAccountId(CALENDARS),
          destroy: [occurrenceId],
        }),
      ]);
      expect(gone.destroyed).toStrictEqual([occurrenceId]);
      const series = await seriesNow(seriesId);
      expect(series?.recurrenceRule).toMatchObject({ frequency: "weekly" });
      expect(series?.recurrenceOverrides).toStrictEqual({
        [`${SECOND}T00:00:00`]: { excluded: true },
      });
    } finally {
      await removeEvents(bob, bob.primaryAccountId(CALENDARS), DAY_UID);
    }
  });

  it("our REQUEST for one all-day occurrence is read as that day", async () => {
    const { alice, bob } = clients();
    const moved = nextDay(SECOND);
    const ics = buildOccurrenceRequest({
      allDay: true,
      endDate: nextDay(moved),
      guests: [
        {
          email: aliceAddress,
          name: null,
          optional: false,
          status: "needs-action",
        },
      ],
      location: null,
      now: Date.now(),
      organizer: { email: bobAddress, name: null },
      recurrenceId: `${SECOND}T00:00:00`,
      recurrenceIdTimeZone: null,
      sequence: 0,
      startDate: moved,
      title: `${MARKER} offsite moved`,
      uid: DAY_UID,
    });
    expect(ics).toContain(`RECURRENCE-ID;VALUE=DATE:${compact(SECOND)}`);
    await expect(
      sendItip(bob, {
        from: bobAddress,
        ics,
        method: "REQUEST",
        subject: `Updated invitation: ${MARKER} offsite moved`,
        to: aliceAddress,
      })
    ).resolves.toBeTruthy();
    const blobId = await eventually(
      async (): Promise<BlobId | null> => {
        const emails = await markedEmails(alice);
        const email = emails.find(
          (item) => item.subject?.includes("offsite moved") === true
        );
        return (
          email?.attachments?.find(
            (attachment) =>
              attachment.type?.toLowerCase() === "text/calendar" ||
              attachment.name?.toLowerCase().endsWith(".ics") === true
          )?.blobId ?? null
        );
      },
      () => "no REQUEST for the all-day occurrence"
    );
    const [parsed] = await alice.request((b) => [
      b.call("CalendarEvent/parse", {
        accountId: alice.primaryAccountId(CALENDARS),
        blobIds: [blobId],
      }),
    ]);
    const [event] = parsed.parsed[blobId] ?? [];
    // Stalwart converts a DATE RECURRENCE-ID to `recurrenceId`.
    expect(event?.recurrenceId).toBe(`${SECOND}T00:00:00`);
    const invitation = analyzeInvitation(parsed.parsed[blobId] ?? [], [
      aliceAddress,
    ]);
    expect(invitation).toMatchObject({
      event: { showWithoutTime: true, start: `${moved}T00:00:00` },
      method: "request",
      recurrenceId: `${SECOND}T00:00:00`,
      uid: DAY_UID,
    });
    // Her answer names that day the same way (a DATE).
    const reply = buildReply({
      allDay: invitation?.event.showWithoutTime === true,
      attendee: { email: aliceAddress, name: null },
      now: Date.now(),
      organizer: { email: bobAddress, name: null },
      recurrenceId: invitation?.recurrenceId ?? null,
      recurrenceIdTimeZone: invitation?.recurrenceIdTimeZone ?? null,
      sequence: 0,
      status: "accepted",
      title: `${MARKER} offsite moved`,
      uid: DAY_UID,
    });
    expect(reply).toContain(`RECURRENCE-ID;VALUE=DATE:${compact(SECOND)}`);
  }, 60_000);
});

/** bob's new task, assigned to `assignee` and mailed by Stalwart. */
const createAssignedTask = async (
  bob: JmapClient,
  task: {
    readonly uid: string;
    readonly title: string;
    readonly organizer: string;
    readonly assignee: string;
    /** Default: the day FIRST. */
    readonly due?: TaskDue;
    readonly recurrenceRule?: RecurrenceRule;
  }
): Promise<CalendarEventId | undefined> => {
  const accountId = bob.primaryAccountId(CALENDARS);
  const [calendars] = await bob.request((b) => [
    b.call("Calendar/get", { accountId }),
  ]);
  const calendarId = CalendarId.parse(
    (
      calendars.list.find((item) => item.isDefault === true) ??
      calendars.list[0]
    )?.id
  );
  const properties = {
    ...newTaskProperties(
      {
        calendarId,
        due: task.due ?? { date: FIRST, kind: "date" },
        title: task.title,
        ...(task.recurrenceRule === undefined
          ? {}
          : { recurrenceRule: task.recurrenceRule }),
      },
      { display: "Europe/Madrid", uid: task.uid }
    ),
    ...assigneesPatch({}, [{ email: task.assignee, name: null }], {
      email: task.organizer,
      name: null,
    }),
  };
  const [created] = await bob.request((b) => [
    b.call("CalendarEvent/set", {
      accountId,
      // A task; the type wants an event's shape (as in tasks/adapter.ts).
      // oxlint-disable-next-line typescript/no-unsafe-type-assertion
      create: { task: properties as never },
      sendSchedulingMessages: true,
    }),
  ]);
  return created.created.task?.id;
};

/** The task's people as the organizer's copy has them. */
const assigneesIn = async (
  organizer: JmapClient,
  taskId: CalendarEventId | undefined,
  organizerAddress: string
): Promise<readonly TaskAssignee[]> => {
  const [stored] = await organizer.request((b) => [
    b.call("CalendarEvent/get", {
      accountId: organizer.primaryAccountId(CALENDARS),
      ids: taskId === undefined ? [] : [taskId],
      properties: ["@type", "participants", "organizerCalendarAddress"],
    }),
  ]);
  return (
    assignmentOf(stored.list[0] ?? {}, [organizerAddress])?.assignees ?? []
  );
};

/** The `.ics` of the first marked email whose subject has `subject`. */
const calendarPartOf = async (
  client: JmapClient,
  subject: string
): Promise<BlobId> => {
  let seen: string[] = [];
  return await eventually(
    async (): Promise<BlobId | null> => {
      const emails = await markedEmails(client);
      seen = emails.map((email) => email.subject ?? "");
      const email = emails.find(
        (item) => item.subject?.includes(subject) === true
      );
      const part = email?.attachments?.find(
        (attachment) =>
          attachment.type?.toLowerCase() === "text/calendar" ||
          attachment.name?.toLowerCase().endsWith(".ics") === true
      );
      return part?.blobId ?? null;
    },
    () => `no "${subject}" email; saw ${seen.join(" | ")}`
  );
};

// Assigned tasks over Stalwart's own JMAP:
// what the app writes (newTaskProperties + assigneesPatch) with
// `sendSchedulingMessages`, and the assignee's REPLY the app mails.
describe.skipIf(!configured)("live Stalwart: assigning a task", () => {
  const bobAddress = env.organizer.toLowerCase();
  const aliceAddress = env.user.toLowerCase();
  const taskUid = `${MARKER}task@nebulamail.test`;
  const title = `${MARKER} assigned task`;
  const state: {
    bob?: JmapClient;
    alice?: JmapClient;
    taskId?: CalendarEventId | undefined;
  } = {};

  const clients = (): {
    readonly bob: JmapClient;
    readonly alice: JmapClient;
  } => {
    const { alice, bob } = state;
    if (alice === undefined || bob === undefined) {
      throw new Error("Not connected");
    }
    return { alice, bob };
  };

  beforeAll(async () => {
    state.bob = await connect(env.organizer, env.organizerPassword);
    state.alice = await connect(env.user, env.password);
  });

  afterAll(async () => {
    const { alice, bob } = clients();
    await removeEvents(bob, bob.primaryAccountId(CALENDARS), taskUid).catch(
      () => null
    );
    await removeEvents(alice, alice.primaryAccountId(CALENDARS), taskUid).catch(
      () => null
    );
    await removeEmails(alice).catch(() => null);
    await removeEmails(bob).catch(() => null);
  });

  it("bob assigns a task to alice and Stalwart keeps the participants", async () => {
    const { bob } = clients();
    state.taskId = await createAssignedTask(bob, {
      assignee: aliceAddress,
      organizer: bobAddress,
      title,
      uid: taskUid,
    });
    expect(state.taskId).toBeDefined();
    const [stored] = await bob.request((b) => [
      b.call("CalendarEvent/get", {
        accountId: bob.primaryAccountId(CALENDARS),
        ids: state.taskId === undefined ? [] : [state.taskId],
        properties: ["@type", "participants", "organizerCalendarAddress"],
      }),
    ]);
    const assignment = assignmentOf(stored.list[0] ?? {}, [bobAddress]);
    expect(assignment?.iAmOrganizer).toBeTruthy();
    expect(assignment?.assignees.map((person) => person.email)).toStrictEqual([
      aliceAddress,
    ]);
  });

  it("alice gets the REQUEST as a task assigned to her", async () => {
    const { alice } = clients();
    const blobId = await calendarPartOf(alice, "assigned task");
    const accountId = alice.primaryAccountId(CALENDARS);
    const [parsed] = await alice.request((b) => [
      b.call("CalendarEvent/parse", { accountId, blobIds: [blobId] }),
    ]);
    const [task] = parsed.parsed[blobId] ?? [];
    expect(task).toMatchObject({ "@type": "Task", uid: taskUid });
    const assignment = assignmentOf(task ?? {}, [aliceAddress]);
    expect(assignment?.organizer?.email).toBe(bobAddress);
    expect(assignment?.me?.status).toBe("needs-action");
  }, 60_000);

  it("Stalwart puts the task in alice's calendar by itself, like an event", async () => {
    const { alice } = clients();
    const accountId = alice.primaryAccountId(CALENDARS);
    let seen = "";
    const copy = await eventually(
      async () => {
        const [, copies] = await alice.request((b) => {
          const query = b.call("CalendarEvent/query", {
            accountId,
            filter: { uid: taskUid },
          });
          const get = b.call("CalendarEvent/get", {
            "#ids": query.ref("/ids"),
            accountId,
            properties: [
              "@type",
              "uid",
              "title",
              "participants",
              "organizerCalendarAddress",
            ],
          });
          return [query, get] as const;
        });
        seen = JSON.stringify(copies.list);
        return copies.list.find((item) => item.uid === taskUid) ?? null;
      },
      () => `no copy of the task in alice's calendar: ${seen}`
    );
    expect(copy).toMatchObject({ "@type": "Task", title });
    const assignment = assignmentOf(copy, [aliceAddress]);
    expect(assignment?.organizer?.email).toBe(bobAddress);
    expect(assignment?.me?.status).toBe("needs-action");
  }, 60_000);

  it("alice's REPLY marks the task completed in bob's copy", async () => {
    const { alice, bob } = clients();
    const ics = buildTaskReply({
      attendee: { email: aliceAddress, name: null },
      now: Date.now(),
      organizer: { email: bobAddress, name: null },
      partstat: "COMPLETED",
      recurrenceId: null,
      recurrenceIdTimeZone: null,
      sequence: 0,
      title,
      uid: taskUid,
    });
    await expect(
      sendItip(alice, {
        from: aliceAddress,
        ics,
        method: "REPLY",
        subject: `Completed: ${title}`,
        to: bobAddress,
      })
    ).resolves.toBeTruthy();
    let last = "";
    await eventually(
      async () => {
        const assignees = await assigneesIn(bob, state.taskId, bobAddress);
        last = JSON.stringify(assignees);
        const assignee = assignees.find(
          (person) => person.email === aliceAddress
        );
        return assignee?.status === "completed" ? true : null;
      },
      () => `bob's task never got the answer: ${last}`
    );
  }, 60_000);
});

// Delegating an assigned task
//  against Stalwart 1.0.1: alice passes bob's task on to carol
// the way the app does (a REPLY to bob with DELEGATED-TO and carol's
// ATTENDEE line, and a REQUEST to carol), bob's copy then has carol, and
// carol's REPLY reaches it. Needs a third account (CAL_LIVE_DELEGATE).
describe.skipIf(!delegateConfigured)("live Stalwart: delegating a task", () => {
  const bobAddress = env.organizer.toLowerCase();
  const aliceAddress = env.user.toLowerCase();
  const carolAddress = delegateEnv.delegate.toLowerCase();
  const taskUid = `${MARKER}delegated@nebulamail.test`;
  const title = `${MARKER} delegated task`;
  const state: {
    bob?: JmapClient;
    alice?: JmapClient;
    carol?: JmapClient;
    taskId?: CalendarEventId | undefined;
  } = {};

  const clients = (): {
    readonly bob: JmapClient;
    readonly alice: JmapClient;
    readonly carol: JmapClient;
  } => {
    const { alice, bob, carol } = state;
    if (alice === undefined || bob === undefined || carol === undefined) {
      throw new Error("Not connected");
    }
    return { alice, bob, carol };
  };

  const waitFor = async (
    check: (assignees: readonly TaskAssignee[]) => boolean,
    what: string
  ): Promise<readonly TaskAssignee[]> => {
    const { bob } = clients();
    let last = "";
    return await eventually(
      async () => {
        const assignees = await assigneesIn(bob, state.taskId, bobAddress);
        last = JSON.stringify(assignees);
        return check(assignees) ? assignees : null;
      },
      () => `${what}: ${last}`
    );
  };

  beforeAll(async () => {
    state.bob = await connect(env.organizer, env.organizerPassword);
    state.alice = await connect(env.user, env.password);
    state.carol = await connect(
      delegateEnv.delegate,
      delegateEnv.delegatePassword
    );
  });

  afterAll(async () => {
    for (const client of Object.values(clients())) {
      // oxlint-disable-next-line no-await-in-loop -- three accounts, in turn.
      await removeEvents(
        client,
        client.primaryAccountId(CALENDARS),
        taskUid
      ).catch(() => null);
      // oxlint-disable-next-line no-await-in-loop -- three accounts, in turn.
      await removeEmails(client).catch(() => null);
    }
  });

  it("alice's delegation adds carol to bob's copy", async () => {
    const { alice, bob } = clients();
    state.taskId = await createAssignedTask(bob, {
      assignee: aliceAddress,
      organizer: bobAddress,
      title,
      uid: taskUid,
    });
    expect(state.taskId).toBeDefined();
    // Alice has it before she passes it on.
    await calendarPartOf(alice, "delegated task");
    const common = {
      attendee: { email: aliceAddress, name: null },
      now: Date.now(),
      organizer: { email: bobAddress, name: null },
      recurrenceId: null,
      recurrenceIdTimeZone: null,
      sequence: 0,
    };
    await expect(
      sendItip(alice, {
        from: aliceAddress,
        ics: buildTaskDelegation({
          ...common,
          delegate: { email: carolAddress, name: null },
          title,
          uid: taskUid,
        }),
        method: "REPLY",
        subject: `Delegated: ${title}`,
        to: bobAddress,
      })
    ).resolves.toBeTruthy();
    await expect(
      sendItip(alice, {
        from: aliceAddress,
        ics: buildTaskRequest({
          attendees: [
            {
              delegatedFrom: null,
              delegatedTo: carolAddress,
              email: aliceAddress,
              name: null,
              partstat: "DELEGATED",
              rsvp: false,
            },
            {
              delegatedFrom: aliceAddress,
              delegatedTo: null,
              email: carolAddress,
              name: null,
              partstat: "NEEDS-ACTION",
              rsvp: true,
            },
          ],
          description: null,
          due: { date: FIRST, kind: "date" },
          now: common.now,
          organizer: common.organizer,
          sequence: 0,
          title,
          uid: taskUid,
        }),
        method: "REQUEST",
        subject: `Task: ${title}`,
        to: carolAddress,
      })
    ).resolves.toBeTruthy();
    const assignees = await waitFor(
      (people) => people.some((person) => person.email === carolAddress),
      "bob's copy never got carol"
    );
    expect(
      assignees.map(({ delegatedFrom, delegatedTo, email, status }) => ({
        delegatedFrom,
        delegatedTo,
        email,
        status,
      }))
    ).toStrictEqual(
      expect.arrayContaining([
        {
          delegatedFrom: null,
          delegatedTo: carolAddress,
          email: aliceAddress,
          status: "delegated",
        },
        {
          delegatedFrom: aliceAddress,
          delegatedTo: null,
          email: carolAddress,
          status: "needs-action",
        },
      ])
    );
  }, 90_000);

  it("carol gets the task from alice and her REPLY reaches bob", async () => {
    const { carol } = clients();
    const blobId = await calendarPartOf(carol, `Task: ${MARKER}`);
    const [parsed] = await carol.request((b) => [
      b.call("CalendarEvent/parse", {
        accountId: carol.primaryAccountId(CALENDARS),
        blobIds: [blobId],
      }),
    ]);
    const [task] = parsed.parsed[blobId] ?? [];
    expect(task).toMatchObject({ "@type": "Task", uid: taskUid });
    const assignment = assignmentOf(task ?? {}, [carolAddress]);
    expect(assignment?.organizer?.email).toBe(bobAddress);
    expect(assignment?.me).toMatchObject({
      delegatedFrom: aliceAddress,
      status: "needs-action",
    });
    // Stalwart doesn't file a REQUEST from someone who isn't the organizer:
    // carol has it only once she accepts it from the email's card.
    const accountId = carol.primaryAccountId(CALENDARS);
    const [, copies] = await carol.request((b) => {
      const query = b.call("CalendarEvent/query", {
        accountId,
        filter: { uid: taskUid },
      });
      const get = b.call("CalendarEvent/get", {
        "#ids": query.ref("/ids"),
        accountId,
        properties: ["uid"],
      });
      return [query, get] as const;
    });
    expect(copies.list.filter((copy) => copy.uid === taskUid)).toHaveLength(0);
    await expect(
      sendItip(carol, {
        from: carolAddress,
        ics: buildTaskReply({
          attendee: { email: carolAddress, name: null },
          now: Date.now(),
          organizer: { email: bobAddress, name: null },
          partstat: "COMPLETED",
          recurrenceId: null,
          recurrenceIdTimeZone: null,
          sequence: 0,
          title,
          uid: taskUid,
        }),
        method: "REPLY",
        subject: `Completed: ${title}`,
        to: bobAddress,
      })
    ).resolves.toBeTruthy();
    await waitFor(
      (people) =>
        people.some(
          (person) =>
            person.email === carolAddress && person.status === "completed"
        ),
      "carol's answer never reached bob"
    );
  }, 90_000);

  it("carol passes it on again and bob's copy follows", async () => {
    const { carol } = clients();
    // Anyone's address: only bob's copy is looked at (nothing is sent to it).
    const daveAddress = "dave@nebulamail.test";
    await expect(
      sendItip(carol, {
        from: carolAddress,
        ics: buildTaskDelegation({
          attendee: { email: carolAddress, name: null },
          delegate: { email: daveAddress, name: null },
          now: Date.now(),
          organizer: { email: bobAddress, name: null },
          recurrenceId: null,
          recurrenceIdTimeZone: null,
          sequence: 0,
          title,
          uid: taskUid,
        }),
        method: "REPLY",
        subject: `Delegated again: ${title}`,
        to: bobAddress,
      })
    ).resolves.toBeTruthy();
    const assignees = await waitFor(
      (people) => people.some((person) => person.email === daveAddress),
      "bob's copy never got dave"
    );
    expect(
      assignees
        .filter((person) => person.email !== aliceAddress)
        .map(({ delegatedFrom, delegatedTo, email, status }) => ({
          delegatedFrom,
          delegatedTo,
          email,
          status,
        }))
    ).toStrictEqual(
      expect.arrayContaining([
        {
          delegatedFrom: aliceAddress,
          delegatedTo: daveAddress,
          email: carolAddress,
          status: "delegated",
        },
        {
          delegatedFrom: carolAddress,
          delegatedTo: null,
          email: daveAddress,
          status: "needs-action",
        },
      ])
    );
  }, 90_000);
});

// Delegating a repeating task
// bob assigns alice a weekly task at 09:00 Madrid
// time; alice passes the whole series on to carol as the app does (her
// stored copy → `taskSeries` → `buildTaskRequest` with RRULE, the zone's
// VTIMEZONE and an EXDATE). Checks that Stalwart parses it back as the
// series (rule, zone, the Monday taken out) and that carol's "Accept"
// (`taskToImport`) stores a repeating task. Needs CAL_LIVE_DELEGATE.
// Not run yet (no Docker in the session that wrote it, 2026-11-01).
describe.skipIf(!delegateConfigured)(
  "live Stalwart: delegating a repeating task",
  () => {
    const bobAddress = env.organizer.toLowerCase();
    const aliceAddress = env.user.toLowerCase();
    const carolAddress = delegateEnv.delegate.toLowerCase();
    const taskUid = `${MARKER}-series@nebulamail.test`;
    const title = `${MARKER} weekly delegated`;
    const state: {
      bob?: JmapClient;
      alice?: JmapClient;
      carol?: JmapClient;
    } = {};

    const clients = (): {
      readonly bob: JmapClient;
      readonly alice: JmapClient;
      readonly carol: JmapClient;
    } => {
      const { alice, bob, carol } = state;
      if (alice === undefined || bob === undefined || carol === undefined) {
        throw new Error("Not connected");
      }
      return { alice, bob, carol };
    };

    beforeAll(async () => {
      state.bob = await connect(env.organizer, env.organizerPassword);
      state.alice = await connect(env.user, env.password);
      state.carol = await connect(
        delegateEnv.delegate,
        delegateEnv.delegatePassword
      );
    });

    afterAll(async () => {
      for (const client of Object.values(clients())) {
        // oxlint-disable-next-line no-await-in-loop -- three accounts, in turn.
        await removeEvents(
          client,
          client.primaryAccountId(CALENDARS),
          taskUid
        ).catch(() => null);
        // oxlint-disable-next-line no-await-in-loop -- three accounts, in turn.
        await removeEmails(client).catch(() => null);
      }
    });

    it("alice passes the series on and carol stores it as one", async () => {
      const { alice, bob, carol } = clients();
      await createAssignedTask(bob, {
        assignee: aliceAddress,
        due: {
          kind: "datetime",
          local: `${FIRST}T09:00:00`,
          timeZone: "Europe/Madrid",
        },
        organizer: bobAddress,
        recurrenceRule: { "@type": "RecurrenceRule", frequency: "weekly" },
        title,
        uid: taskUid,
      });
      // Alice's copy, as Stalwart put it in her calendar.
      const accountId = alice.primaryAccountId(CALENDARS);
      const stored = await eventually(
        async () => {
          const [, found] = await alice.request((b) => {
            const query = b.call("CalendarEvent/query", {
              accountId,
              filter: { uid: taskUid },
            });
            return [
              query,
              b.call("CalendarEvent/get", {
                "#ids": query.ref("/ids"),
                accountId,
              }),
            ] as const;
          });
          return found.list[0] ?? null;
        },
        () => "the task never reached alice's calendar"
      );
      const me = assignmentOf(stored, [aliceAddress])?.me ?? null;
      const delegation =
        me === null
          ? null
          : delegationOf(stored, me.key, { email: carolAddress, name: null }, [
              aliceAddress,
            ]);
      // The second Monday taken out, as alice's copy could have it.
      const series = taskSeries({
        ...stored,
        recurrenceOverrides: {
          [`${SECOND}T09:00:00`]: { excluded: true },
        },
      });
      expect(series).not.toBeNull();
      const after = assignmentOf(
        { ...stored, participants: delegation?.participants ?? {} },
        [aliceAddress]
      );
      await expect(
        sendItip(alice, {
          from: aliceAddress,
          ics: buildTaskRequest({
            attendees: (after?.assignees ?? []).map((person) => ({
              delegatedFrom: person.delegatedFrom,
              delegatedTo: person.delegatedTo,
              email: person.email,
              name: null,
              partstat:
                person.email === carolAddress ? "NEEDS-ACTION" : "DELEGATED",
              rsvp: person.email === carolAddress,
            })),
            description: null,
            due: null,
            now: Date.now(),
            organizer: { email: bobAddress, name: null },
            sequence: Math.trunc(stored.sequence ?? 0),
            series,
            title,
            uid: taskUid,
          }),
          method: "REQUEST",
          subject: `Series: ${title}`,
          to: carolAddress,
        })
      ).resolves.toBeTruthy();
      const blobId = await calendarPartOf(carol, `Series: ${MARKER}`);
      const [parsed] = await carol.request((b) => [
        b.call("CalendarEvent/parse", {
          accountId: carol.primaryAccountId(CALENDARS),
          blobIds: [blobId],
        }),
      ]);
      const [task] = parsed.parsed[blobId] ?? [];
      expect(task).toMatchObject({
        "@type": "Task",
        recurrenceOverrides: { [`${SECOND}T09:00:00`]: { excluded: true } },
        recurrenceRule: { frequency: "weekly" },
        start: `${FIRST}T09:00:00`,
        timeZone: "Europe/Madrid",
        uid: taskUid,
      });
      // "Accept": carol's own repeating task.
      const carolMe = assignmentOf(task ?? {}, [carolAddress])?.me ?? null;
      const [calendars] = await carol.request((b) => [
        b.call("Calendar/get", {
          accountId: carol.primaryAccountId(CALENDARS),
        }),
      ]);
      const calendarId = (
        calendars.list.find((item) => item.isDefault === true) ??
        calendars.list[0]
      )?.id;
      const [created] = await carol.request((b) => [
        b.call("CalendarEvent/set", {
          accountId: carol.primaryAccountId(CALENDARS),
          create: {
            // A task; the type wants an event's shape (tasks/adapter.ts).
            // oxlint-disable-next-line typescript/no-unsafe-type-assertion
            task: {
              ...taskToImport(task ?? {}, carolMe?.key ?? null, "accepted"),
              calendarIds: { [calendarId ?? ""]: true },
            } as never,
          },
        }),
      ]);
      const [read] = await carol.request((b) => [
        b.call("CalendarEvent/get", {
          accountId: carol.primaryAccountId(CALENDARS),
          ids:
            created.created.task === undefined ? [] : [created.created.task.id],
        }),
      ]);
      expect(read.list[0]).toMatchObject({
        recurrenceOverrides: { [`${SECOND}T09:00:00`]: { excluded: true } },
        recurrenceRule: { frequency: "weekly" },
        timeZone: "Europe/Madrid",
      });
    }, 120_000);
  }
);
