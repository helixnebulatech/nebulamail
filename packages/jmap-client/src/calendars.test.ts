// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { describe, expect, expectTypeOf, it } from "vitest";
import { z } from "zod";

import { JmapClient } from "./client";
import {
  AccountId,
  BlobId,
  CalendarEventId,
  CalendarId,
  PrincipalId,
} from "./ids";
import type { CalendarEventId as EventId } from "./ids";
import {
  SESSION_URL,
  fakeFetch,
  methodResponses,
  sessionThen,
  tokens,
} from "./test-support";

const accountId = AccountId.parse("d");

const usingOf = (body: unknown): string[] =>
  z.object({ using: z.array(z.string()) }).parse(body).using;

const connect = async (
  fetch: Parameters<typeof JmapClient.connect>[0]["fetch"]
) =>
  await JmapClient.connect({
    sessionUrl: SESSION_URL,
    ...tokens(),
    ...(fetch === undefined ? {} : { fetch }),
  });

describe("CalendarEvent", () => {
  it("expands a range and reads occurrences without default values", async () => {
    const fake = fakeFetch(
      sessionThen(() =>
        methodResponses([
          [
            "CalendarEvent/query",
            {
              accountId: "d",
              canCalculateChanges: true,
              ids: ["h1pkdeaaaaab"],
              position: 0,
              queryState: "q1",
            },
            "c0",
          ],
          [
            "CalendarEvent/get",
            {
              accountId: "d",
              // As Stalwart answers (checked live): `showWithoutTime`
              // was asked for but is left out because it is false.
              list: [
                {
                  baseEventId: "b",
                  duration: "PT1H",
                  id: "h1pkdeaaaaab",
                  recurrenceId: "2026-11-02T10:00:00",
                  recurrenceRule: null,
                  start: "2026-11-02T10:00:00",
                  timeZone: "Europe/Madrid",
                  title: "Weekly",
                  utcEnd: "2026-11-02T10:00:00Z",
                  utcStart: "2026-11-02T09:00:00Z",
                },
              ],
              notFound: [],
              state: "s1",
            },
            "c1",
          ],
        ])
      )
    );
    const client = await connect(fake.fetch);
    const [, events] = await client.request((b) => {
      const query = b.call("CalendarEvent/query", {
        accountId,
        expandRecurrences: true,
        filter: { after: "2026-11-01T00:00:00", before: "2026-11-08T00:00:00" },
        timeZone: "Europe/Madrid",
      });
      const get = b.call("CalendarEvent/get", {
        "#ids": query.ref("/ids"),
        accountId,
        properties: ["title", "start", "showWithoutTime", "utcStart"],
      });
      return [query, get] as const;
    });
    const [event] = events.list;
    expectTypeOf(event?.id).toEqualTypeOf<EventId | undefined>();
    expect(event?.baseEventId).toBe("b");
    expect(event?.showWithoutTime).toBeUndefined();
    expect(event?.utcStart).toBe("2026-11-02T09:00:00Z");
    expect(usingOf(fake.calls[1]?.body)).toContain(
      "urn:ietf:params:jmap:calendars"
    );
  });

  it("parses invitation blobs (array or single object) with their method", async () => {
    const blob = BlobId.parse("blob1");
    const fake = fakeFetch(
      sessionThen(() =>
        methodResponses([
          [
            "CalendarEvent/parse",
            {
              accountId: "d",
              parsed: {
                blob1: [
                  {
                    "@type": "Event",
                    method: "request",
                    start: "2026-10-28T16:00:00",
                    title: "Planning",
                    uid: "u1",
                  },
                ],
                blob2: { method: "cancel", uid: "u2" },
              },
            },
            "c0",
          ],
        ])
      )
    );
    const client = await connect(fake.fetch);
    const [parsed] = await client.request((b) => [
      b.call("CalendarEvent/parse", {
        accountId,
        blobIds: [blob, BlobId.parse("blob2")],
      }),
    ]);
    expect(parsed.parsed.blob1?.[0]?.method).toBe("request");
    expect(parsed.parsed.blob2?.[0]?.uid).toBe("u2");
    expect(parsed.notParsable).toStrictEqual([]);
    expect(usingOf(fake.calls[1]?.body)).toStrictEqual(
      expect.arrayContaining([
        "urn:ietf:params:jmap:calendars",
        "urn:ietf:params:jmap:calendars:parse",
      ])
    );
  });

  it("sends sendSchedulingMessages and reads the set response", async () => {
    const fake = fakeFetch(
      sessionThen(() =>
        methodResponses([
          [
            "CalendarEvent/set",
            {
              accountId: "d",
              created: { e1: { id: "c" } },
              newState: "s2",
              oldState: "s1",
            },
            "c0",
          ],
        ])
      )
    );
    const client = await connect(fake.fetch);
    const [result] = await client.request((b) => [
      b.call("CalendarEvent/set", {
        accountId,
        create: {
          e1: {
            calendarIds: { [CalendarId.parse("b")]: true },
            duration: "PT30M",
            start: "2026-10-28T16:00:00",
            timeZone: "Europe/Madrid",
            title: "Planning",
          },
        },
        sendSchedulingMessages: true,
      }),
    ]);
    expect(result.created.e1?.id).toBe(CalendarEventId.parse("c"));
    const body = z
      .object({
        methodCalls: z.array(z.tuple([z.string(), z.unknown(), z.string()])),
      })
      .parse(fake.calls[1]?.body);
    expect(body.methodCalls[0]?.[1]).toMatchObject({
      sendSchedulingMessages: true,
    });
  });
});

describe("Principal/getAvailability", () => {
  it("needs the availability capability and returns busy periods", async () => {
    const fake = fakeFetch(
      sessionThen(() =>
        methodResponses([
          [
            "Principal/getAvailability",
            {
              list: [
                {
                  busyStatus: "confirmed",
                  event: null,
                  utcEnd: "2026-10-28T15:30:00Z",
                  utcStart: "2026-10-28T15:00:00Z",
                },
              ],
            },
            "c0",
          ],
        ])
      )
    );
    const client = await connect(fake.fetch);
    const [busy] = await client.request((b) => [
      b.call("Principal/getAvailability", {
        accountId,
        id: PrincipalId.parse("p"),
        utcEnd: "2026-11-02T00:00:00Z",
        utcStart: "2026-10-26T00:00:00Z",
      }),
    ]);
    expect(busy.list[0]?.busyStatus).toBe("confirmed");
    expect(usingOf(fake.calls[1]?.body)).toContain(
      "urn:ietf:params:jmap:principals:availability"
    );
  });
});
