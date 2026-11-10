// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { JmapError, PrincipalId } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import { EventSaveError } from "./api";
import {
  CALENDAR_SHARE_LEVELS,
  calendarLevelOf,
  calendarRightsFor,
  calendarSharePatch,
  shareFailure,
} from "./sharing";

const NONE = {
  mayDelete: false,
  mayReadFreeBusy: false,
  mayReadItems: false,
  mayRSVP: false,
  mayShare: false,
  mayUpdatePrivate: false,
  mayWriteAll: false,
  mayWriteOwn: false,
};

const WRITE = {
  mayRSVP: true,
  mayUpdatePrivate: true,
  mayWriteAll: true,
  mayWriteOwn: true,
};

describe("calendar share levels", () => {
  it("map each level to its rights", () => {
    expect(calendarRightsFor("freeBusy")).toStrictEqual({
      ...NONE,
      mayReadFreeBusy: true,
    });
    expect(calendarRightsFor("read")).toStrictEqual({
      ...NONE,
      mayReadFreeBusy: true,
      mayReadItems: true,
    });
    expect(calendarRightsFor("write")).toStrictEqual({
      ...NONE,
      ...WRITE,
      mayReadFreeBusy: true,
      mayReadItems: true,
    });
    expect(calendarRightsFor("manage")).toStrictEqual({
      ...NONE,
      ...WRITE,
      mayReadFreeBusy: true,
      mayReadItems: true,
      mayShare: true,
    });
  });

  it("read every level back (Stalwart returns them as sent)", () => {
    for (const level of CALENDAR_SHARE_LEVELS) {
      expect(calendarLevelOf(calendarRightsFor(level))).toBe(level);
    }
  });

  it("take missing rights as false", () => {
    expect(calendarLevelOf({ mayReadFreeBusy: true, mayReadItems: true })).toBe(
      "read"
    );
  });

  it("accept mayDelete with writing (the bridge's DAV write)", () => {
    expect(
      calendarLevelOf({ ...calendarRightsFor("write"), mayDelete: true })
    ).toBe("write");
    expect(
      calendarLevelOf({ ...calendarRightsFor("manage"), mayDelete: true })
    ).toBe("manage");
    expect(
      calendarLevelOf({ ...calendarRightsFor("read"), mayDelete: true })
    ).toBe("custom");
  });

  it("show other combinations as custom, never as a level", () => {
    // Only their own events (another client).
    expect(
      calendarLevelOf({
        ...calendarRightsFor("read"),
        mayWriteOwn: true,
      })
    ).toBe("custom");
    // Items without free/busy.
    expect(calendarLevelOf({ ...NONE, mayReadItems: true })).toBe("custom");
    expect(calendarLevelOf(NONE)).toBe("custom");
  });
});

describe(calendarSharePatch, () => {
  const bob = PrincipalId.parse("d");

  it("sets one person's rights", () => {
    expect(calendarSharePatch(bob, "read")).toStrictEqual({
      "shareWith/d": calendarRightsFor("read"),
    });
  });

  it("removes them with null", () => {
    expect(calendarSharePatch(bob, null)).toStrictEqual({
      "shareWith/d": null,
    });
  });
});

describe(shareFailure, () => {
  it("reads set errors and method errors", () => {
    expect(shareFailure(new EventSaveError("forbidden", null))).toBe(
      "forbidden"
    );
    expect(
      shareFailure(new EventSaveError("invalidProperties", "Account id"))
    ).toBe("invalidProperties");
    expect(
      shareFailure(
        new JmapError({
          arguments: {},
          callId: "0",
          kind: "method",
          method: "Calendar/set",
          type: "forbidden",
        })
      )
    ).toBe("forbidden");
    expect(shareFailure(new Error("offline"))).toBe("other");
    expect(shareFailure(new EventSaveError("serverFail", null))).toBe("other");
  });
});
