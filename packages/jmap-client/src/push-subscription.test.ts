// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { describe, expect, it } from "vitest";

import { JmapClient } from "./client";
import type { CalendarAlert } from "./push-subscription";
import {
  SESSION_URL,
  fakeFetch,
  methodResponses,
  sessionThen,
  tokens,
} from "./test-support";
import type { Handler } from "./test-support";

const setup = async (api: Handler) => {
  const fake = fakeFetch(sessionThen(api));
  const client = await JmapClient.connect({
    fetch: fake.fetch,
    sessionUrl: SESSION_URL,
    ...tokens(),
  });
  return { calls: fake.calls, client };
};

const ALERT = {
  "@type": "CalendarAlert",
  accountId: "c",
  alertId: "a1",
  calendarEventId: "v",
  recurrenceId: null,
  uid: "a8d54570-96dc-402d-bdeb-a70c15d7ecac",
};

describe("calendar alerts on the event source", () => {
  it("passes calendarAlert events to onCalendarAlert, not the iterator", async () => {
    // As Stalwart sends it (checked live, 2026-10-26).
    const { client } = await setup(
      () =>
        new Response(
          `event: calendarAlert\ndata: ${JSON.stringify(ALERT)}\n\nevent: calendarAlert\ndata: {"@type":"CalendarAlert","alertId":""}\n\nevent: state\ndata: {"@type":"StateChange","changed":{"c":{"CalendarEvent":"s2"}}}\n\n`,
          { headers: { "Content-Type": "text/event-stream" } }
        )
    );
    const alerts: CalendarAlert[] = [];
    const changes = client.push({
      onCalendarAlert: (alert) => {
        alerts.push(alert);
      },
      types: ["CalendarAlert", "CalendarEvent"],
    });
    const first = await changes.next();
    await changes.return();
    expect(first.value).toMatchObject({
      changed: { c: { CalendarEvent: "s2" } },
    });
    // The invalid one (no ids) is dropped.
    expect(alerts).toStrictEqual([ALERT]);
  });
});

describe("PushSubscription", () => {
  it("creates and reads subscriptions without accountId or state", async () => {
    const { calls, client } = await setup((call) => {
      const isSet = JSON.stringify(call.body).includes("PushSubscription/set");
      return isSet
        ? methodResponses([
            [
              "PushSubscription/set",
              // Stalwart adds an accountId and `keys: null` (2026-10-26).
              {
                accountId: "c",
                created: {
                  push: {
                    expires: "2026-11-02T20:51:47Z",
                    id: "b",
                    keys: null,
                  },
                },
              },
              "c0",
            ],
          ])
        : methodResponses([
            [
              "PushSubscription/get",
              {
                list: [
                  {
                    deviceClientId: "nebula-1",
                    expires: "2026-11-02T20:51:47Z",
                    id: "b",
                    types: ["CalendarAlert"],
                    verificationCode: null,
                  },
                ],
                notFound: [],
              },
              "c0",
            ],
          ]);
    });
    const [created] = await client.request((b) => [
      b.call("PushSubscription/set", {
        create: {
          push: {
            deviceClientId: "nebula-1",
            keys: { auth: "x", p256dh: "y" },
            types: ["CalendarAlert"],
            url: "https://push.example/abc",
          },
        },
      }),
    ]);
    expect(created.created.push?.id).toBe("b");
    const [got] = await client.request((b) => [
      b.call("PushSubscription/get", {}),
    ]);
    expect(got.list[0]).toMatchObject({ deviceClientId: "nebula-1", id: "b" });
    // Only the core capability is needed.
    expect(calls.at(-1)?.body).toMatchObject({
      using: ["urn:ietf:params:jmap:core"],
    });
  });
});
