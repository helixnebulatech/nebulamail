// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Calendar calls against a real Stalwart. Skipped
// unless these are set (passwords in docker/.env after `bun run dev:setup`):
//
//   JMAP_LIVE_URL=http://localhost:8080 JMAP_LIVE_USER=alice@nebulamail.test \
//   JMAP_LIVE_PASSWORD=… bun run test
//
// Uses HTTP Basic auth (Stalwart accepts it for JMAP) by swapping the
// Bearer header in a wrapped fetch. Everything it creates is deleted again.

import { describe, expect, it } from "vitest";

import { JmapClient } from "./client";
import { CalendarId } from "./ids";
import type { FetchLike } from "./transport";

const url = process.env.JMAP_LIVE_URL ?? "";
const user = process.env.JMAP_LIVE_USER ?? "";
const password = process.env.JMAP_LIVE_PASSWORD ?? "";
const configured = url !== "" && user !== "" && password !== "";

const basicFetch: FetchLike = async (input, init) => {
  const headers = new Headers(init.headers);
  headers.set("Authorization", `Basic ${btoa(`${user}:${password}`)}`);
  return await fetch(input, { ...init, headers });
};

const connect = async () =>
  await JmapClient.connect({
    fetch: basicFetch,
    getAccessToken: async () => await Promise.resolve("unused"),
    sessionUrl: `${url}/.well-known/jmap`,
  });

const ICS = [
  "BEGIN:VCALENDAR",
  "VERSION:2.0",
  "PRODID:-//Nebula Mail//live test//EN",
  "METHOD:REQUEST",
  "BEGIN:VEVENT",
  "UID:live-test-parse@nebula.test",
  "DTSTAMP:20261024T080000Z",
  "DTSTART;TZID=America/New_York:20261105T090000",
  "DURATION:PT45M",
  "SUMMARY:Live parse",
  "ORGANIZER;CN=Carol:mailto:carol@example.com",
  "ATTENDEE;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:alice@nebulamail.test",
  "END:VEVENT",
  "END:VCALENDAR",
  "",
].join("\r\n");

describe.skipIf(!configured)("live Stalwart calendars", () => {
  it("creates a recurring event, expands it and edits one occurrence", async () => {
    const client = await connect();
    const accountId = client.primaryAccountId("urn:ietf:params:jmap:calendars");
    const [calendars] = await client.request((b) => [
      b.call("Calendar/get", { accountId }),
    ]);
    const calendar = calendars.list.find((item) => item.isDefault === true);
    expect(calendar).toBeDefined();
    const calendarId = CalendarId.parse(calendar?.id);

    const [created] = await client.request((b) => [
      b.call("CalendarEvent/set", {
        accountId,
        create: {
          live: {
            calendarIds: { [calendarId]: true },
            duration: "PT1H",
            recurrenceRule: {
              "@type": "RecurrenceRule",
              count: 3,
              frequency: "weekly",
            },
            start: "2031-03-03T10:00:00",
            timeZone: "Europe/Madrid",
            title: "Live weekly",
          },
        },
      }),
    ]);
    const id = created.created.live?.id;
    expect(id).toBeDefined();
    if (id === undefined) {
      return;
    }
    try {
      const [, occurrences] = await client.request((b) => {
        const query = b.call("CalendarEvent/query", {
          accountId,
          expandRecurrences: true,
          filter: {
            after: "2031-03-01T00:00:00",
            before: "2031-04-01T00:00:00",
            inCalendar: calendarId,
          },
          timeZone: "Europe/Madrid",
        });
        const get = b.call("CalendarEvent/get", {
          "#ids": query.ref("/ids"),
          accountId,
          properties: [
            "title",
            "start",
            "utcStart",
            "baseEventId",
            "recurrenceId",
          ],
        });
        return [query, get] as const;
      });
      const mine = occurrences.list.filter((event) => event.baseEventId === id);
      expect(mine).toHaveLength(3);
      // 10:00 in Madrid (CET, UTC+1) before the March change of time.
      expect(
        mine
          .map((event) => event.utcStart ?? "")
          .toSorted((a, b) => a.localeCompare(b))[0]
      ).toBe("2031-03-03T09:00:00Z");
      const second = mine.find(
        (event) => event.recurrenceId === "2031-03-10T10:00:00"
      );
      if (second === undefined) {
        throw new Error("The second occurrence is missing");
      }
      const [updated] = await client.request((b) => [
        b.call("CalendarEvent/set", {
          accountId,
          update: { [second.id]: { title: "Moved" } },
        }),
      ]);
      const [master] = await client.request((b) => [
        b.call("CalendarEvent/get", {
          accountId,
          ids: [id],
          properties: ["recurrenceOverrides"],
        }),
      ]);
      expect(updated.updated).toHaveProperty(second.id);
      expect(
        master.list[0]?.recurrenceOverrides?.["2031-03-10T10:00:00"]
      ).toMatchObject({ title: "Moved" });
    } finally {
      await client.request((b) => [
        b.call("CalendarEvent/set", { accountId, destroy: [id] }),
      ]);
    }
  });

  it("parses an uploaded invitation with its iTIP method", async () => {
    const client = await connect();
    const accountId = client.primaryAccountId("urn:ietf:params:jmap:calendars");
    const upload = await client.upload(
      accountId,
      new Blob([ICS], { type: "text/calendar" })
    );
    const [parsed] = await client.request((b) => [
      b.call("CalendarEvent/parse", { accountId, blobIds: [upload.blobId] }),
    ]);
    const [event] = parsed.parsed[upload.blobId] ?? [];
    expect(event).toMatchObject({
      method: "request",
      start: "2026-11-05T09:00:00",
      timeZone: "America/New_York",
      title: "Live parse",
      uid: "live-test-parse@nebula.test",
    });
    const attendee = Object.values(event?.participants ?? {}).find(
      (participant) =>
        participant.calendarAddress === "mailto:alice@nebulamail.test"
    );
    expect(attendee?.participationStatus).toBe("needs-action");
  });

  it("reads the user's own availability", async () => {
    const client = await connect();
    const accountId = client.primaryAccountId("urn:ietf:params:jmap:calendars");
    const [query] = await client.request((b) => [
      b.call("Principal/query", { accountId, filter: { email: user } }),
    ]);
    const [principal] = query.ids;
    expect(principal).toBeDefined();
    if (principal === undefined) {
      return;
    }
    const [busy] = await client.request((b) => [
      b.call("Principal/getAvailability", {
        accountId,
        id: principal,
        utcEnd: "2026-11-30T00:00:00Z",
        utcStart: "2026-10-01T00:00:00Z",
      }),
    ]);
    expect(Array.isArray(busy.list)).toBeTruthy();
  });
});
