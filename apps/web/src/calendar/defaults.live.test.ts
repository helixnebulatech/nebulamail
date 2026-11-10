// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// A calendar's time zone and default alerts against a real Stalwart
// (1.0.1 since 2026-10-30), with the exact patches of the settings
// dialog (calendar/defaults.ts) on a temporary calendar, deleted again.
// Sends no email. Skipped unless these are set (password in docker/.env);
// CI runs it:
//
//   CAL_LIVE_URL=http://localhost:8080 \
//   CAL_LIVE_USER=alice@nebulamail.test CAL_LIVE_PASSWORD=… bun run test

import type { FetchLike, JmapClient } from "@nebulamail/jmap-client";
import { CalendarId, JmapClient as Client } from "@nebulamail/jmap-client";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { z } from "zod";

import { calendarsOf } from "./api";
import type { CalendarSettings, SettingsPatch } from "./defaults";
import { newAlertId, settingsOf, settingsPatch } from "./defaults";
import type { CalendarSummary } from "./model";

const env = {
  password: process.env.CAL_LIVE_PASSWORD ?? "",
  url: process.env.CAL_LIVE_URL ?? "",
  user: process.env.CAL_LIVE_USER ?? "",
};

const configured = Object.values(env).every((value) => value !== "");

const authorization = (): string =>
  `Basic ${btoa(`${env.user}:${env.password}`)}`;

/** Stalwart takes basic auth too; swap the client's Bearer header for it. */
const basicFetch: FetchLike = async (input, init) => {
  const headers = new Headers(init?.headers);
  headers.set("Authorization", authorization());
  return await fetch(input, { ...init, headers });
};

const CALENDARS = "urn:ietf:params:jmap:calendars";
const NAME = `nebuladefaults${Date.now().toString(36)}`;

/** The first response's `notUpdated` (anything else is ignored). */
const SetReply = z.object({
  methodResponses: z
    .array(
      z.tuple([
        z.string(),
        z.object({ notUpdated: z.record(z.string(), z.unknown()).nullish() }),
        z.string(),
      ])
    )
    .min(1),
});

const display = (offset: string) => ({
  "@type": "Alert",
  action: "display",
  trigger: { "@type": "OffsetTrigger", offset, relativeTo: "start" },
});

describe.skipIf(!configured)(
  "a calendar's zone and default alerts (live Stalwart)",
  () => {
    let client: JmapClient;
    let id: CalendarId | null = null;

    const accountId = () => client.primaryAccountId(CALENDARS);

    const ref = (): CalendarId => {
      if (id === null) {
        throw new Error("No calendar");
      }
      return id;
    };

    /** The temporary calendar as the app reads it. */
    const read = async (): Promise<CalendarSummary> => {
      const calendars = await calendarsOf(client);
      const found = calendars.find((calendar) => calendar.id === ref());
      if (found === undefined) {
        throw new Error("The calendar is gone");
      }
      return found;
    };

    /** A `Calendar/set` update, raw (also what the app never sends). */
    const update = async (patch: object): Promise<unknown> => {
      const response = await fetch(client.session.apiUrl, {
        body: JSON.stringify({
          methodCalls: [
            [
              "Calendar/set",
              { accountId: accountId(), update: { [ref()]: patch } },
              "0",
            ],
          ],
          using: ["urn:ietf:params:jmap:core", CALENDARS],
        }),
        headers: {
          Authorization: authorization(),
          "Content-Type": "application/json",
        },
        method: "POST",
      });
      const body = SetReply.parse(await response.json());
      return body.methodResponses[0]?.[1].notUpdated?.[ref()] ?? null;
    };

    /** The zone stored now. */
    const zoneNow = async (): Promise<string | null | undefined> => {
      const calendar = await read();
      return calendar.timeZone;
    };

    /** The minutes of the default alerts with time stored now. */
    const withTimeNow = async (): Promise<readonly number[] | undefined> => {
      const calendar = await read();
      return calendar.defaultAlertsWithTime?.minutes;
    };

    /** What the dialog saves when going from the stored values to `after`. */
    const save = async (after: CalendarSettings): Promise<SettingsPatch> => {
      const calendar = await read();
      const patch = settingsPatch(calendar, settingsOf(calendar), after);
      await expect(update(patch)).resolves.toBeNull();
      return patch;
    };

    beforeAll(async () => {
      client = await Client.connect({
        fetch: basicFetch,
        getAccessToken: async () => await Promise.resolve("unused"),
        sessionUrl: `${env.url}/.well-known/jmap`,
      });
      // As the new-calendar dialog creates it (zone and the usual 10 min).
      const settings = settingsPatch(null, settingsOf(null), {
        ...settingsOf(null),
        timeZone: "Europe/Madrid",
      });
      const [created] = await client.request((b) => [
        b.call("Calendar/set", {
          accountId: accountId(),
          create: { temp: { ...settings, isVisible: true, name: NAME } },
        }),
      ]);
      const createdId = created.created.temp?.id;
      if (createdId === undefined) {
        throw new Error("Could not create the temporary calendar");
      }
      id = CalendarId.parse(createdId);
    });

    afterAll(async () => {
      if (id === null) {
        return;
      }
      await client.request((b) => [
        b.call("Calendar/set", {
          accountId: accountId(),
          destroy: [ref()],
          onDestroyRemoveEvents: true,
        }),
      ]);
    });

    it("keeps what the new-calendar dialog sends", async () => {
      const calendar = await read();
      expect(calendar.timeZone).toBe("Europe/Madrid");
      expect(calendar.defaultAlertsWithTime).toStrictEqual({
        minutes: [10],
        others: {},
      });
      // Nothing sent for all-day events: an empty map, not null.
      expect(calendar.defaultAlertsWithoutTime).toStrictEqual({
        minutes: [],
        others: {},
      });
    });

    it("replaces whole maps and clears them with {}", async () => {
      const patch = await save({
        timeZone: "Europe/Madrid",
        withTime: [30, 5],
        withoutTime: [900],
      });
      expect(Object.keys(patch)).toStrictEqual([
        "defaultAlertsWithTime",
        "defaultAlertsWithoutTime",
      ]);
      let calendar = await read();
      expect(calendar.defaultAlertsWithTime?.minutes).toStrictEqual([30, 5]);
      expect(calendar.defaultAlertsWithoutTime?.minutes).toStrictEqual([900]);
      await save({ ...settingsOf(calendar), withTime: [] });
      calendar = await read();
      expect(calendar.defaultAlertsWithTime).toStrictEqual({
        minutes: [],
        others: {},
      });
    });

    it("takes `<map>/<id>` patches too", async () => {
      const x = newAlertId();
      await expect(
        update({ [`defaultAlertsWithTime/${x}`]: display("-PT1H") })
      ).resolves.toBeNull();
      await expect(withTimeNow()).resolves.toStrictEqual([60]);
      await expect(
        update({ [`defaultAlertsWithTime/${x}`]: null })
      ).resolves.toBeNull();
      await expect(withTimeNow()).resolves.toStrictEqual([]);
    });

    it("refuses an alert id used twice (Stalwart 1.0: unique in the account)", async () => {
      // Why `defaultAlertsMap` gives every alert a new random id.
      const dup = newAlertId();
      await expect(
        update({
          defaultAlertsWithTime: { [dup]: display("-PT1H") },
          defaultAlertsWithoutTime: { [dup]: display("-PT15H") },
        })
      ).resolves.toMatchObject({ type: "invalidProperties" });
    });

    it("keeps alerts the app can't show when saving others", async () => {
      const email = {
        "@type": "Alert",
        action: "email",
        trigger: {
          "@type": "OffsetTrigger",
          offset: "-PT2H",
          relativeTo: "start",
        },
      };
      const m = newAlertId();
      await expect(
        update({ defaultAlertsWithTime: { [m]: email } })
      ).resolves.toBeNull();
      let calendar = await read();
      expect(calendar.defaultAlertsWithTime).toMatchObject({
        minutes: [],
        others: { [m]: { action: "email" } },
      });
      await save({ ...settingsOf(calendar), withTime: [15] });
      calendar = await read();
      // Kept, under a new id.
      expect(calendar.defaultAlertsWithTime?.minutes).toStrictEqual([15]);
      expect(
        Object.values(calendar.defaultAlertsWithTime?.others ?? {})
      ).toMatchObject([{ action: "email" }]);
    });

    it("clears a map with null; refuses absolute triggers", async () => {
      await expect(
        update({ defaultAlertsWithTime: { [newAlertId()]: display("-PT1H") } })
      ).resolves.toBeNull();
      await expect(update({ defaultAlertsWithTime: null })).resolves.toBeNull();
      await expect(withTimeNow()).resolves.toStrictEqual([]);
      await expect(
        update({
          defaultAlertsWithoutTime: {
            [newAlertId()]: {
              "@type": "Alert",
              trigger: {
                "@type": "AbsoluteTrigger",
                when: "2026-11-01T10:00:00Z",
              },
            },
          },
        })
      ).resolves.toMatchObject({ type: "invalidProperties" });
    });

    it("changes and removes the zone; refuses an unknown one", async () => {
      const calendar = await read();
      await save({ ...settingsOf(calendar), timeZone: "America/New_York" });
      await expect(zoneNow()).resolves.toBe("America/New_York");
      // Stalwart stores "UTC" by its canonical name.
      await expect(update({ timeZone: "UTC" })).resolves.toBeNull();
      await expect(zoneNow()).resolves.toBe("Etc/UTC");
      await expect(update({ timeZone: "Not/AZone" })).resolves.toMatchObject({
        properties: ["timeZone"],
        type: "invalidProperties",
      });
      const latest = await read();
      await save({ ...settingsOf(latest), timeZone: null });
      await expect(zoneNow()).resolves.toBeNull();
    });
  }
);
