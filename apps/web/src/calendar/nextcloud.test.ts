// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { BRIDGE_CALENDAR, bridgePatchKey } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import {
  publishingOf,
  subscriptionOf,
  subscriptionUrlProblem,
} from "./nextcloud";

describe(publishingOf, () => {
  it("is null where the bridge doesn't offer it", () => {
    expect(publishingOf({ id: "c1" })).toBeNull();
    expect(publishingOf({ [BRIDGE_CALENDAR.isPublished]: "yes" })).toBeNull();
  });

  it("reads the link of a published calendar", () => {
    expect(
      publishingOf({
        [BRIDGE_CALENDAR.isPublished]: true,
        [BRIDGE_CALENDAR.publicUrl]:
          "https://cloud.example.com/remote.php/dav/public-calendars/X",
      })
    ).toStrictEqual({
      published: true,
      url: "https://cloud.example.com/remote.php/dav/public-calendars/X",
    });
    expect(
      publishingOf({
        [BRIDGE_CALENDAR.isPublished]: false,
        [BRIDGE_CALENDAR.publicUrl]: null,
      })
    ).toStrictEqual({ published: false, url: null });
  });

  it("never shows a link that isn't http(s)", () => {
    expect(
      publishingOf({
        [BRIDGE_CALENDAR.isPublished]: true,
        [BRIDGE_CALENDAR.publicUrl]: "ftp://example.com/x",
      })
    ).toStrictEqual({ published: false, url: null });
  });
});

describe(subscriptionOf, () => {
  it("is null for an ordinary calendar", () => {
    expect(subscriptionOf({ id: "c1" })).toBeNull();
  });

  it("reads the source, or null for an odd one", () => {
    expect(
      subscriptionOf({
        [BRIDGE_CALENDAR.subscriptionUrl]: "webcal://example.com/h.ics",
      })
    ).toStrictEqual({ url: "webcal://example.com/h.ics" });
    expect(
      subscriptionOf({ [BRIDGE_CALENDAR.subscriptionUrl]: "data:text/x,1" })
    ).toStrictEqual({ url: null });
    expect(
      subscriptionOf({ [BRIDGE_CALENDAR.subscriptionUrl]: null })
    ).toStrictEqual({ url: null });
  });
});

describe(subscriptionUrlProblem, () => {
  it("takes https and webcal(s)", () => {
    for (const url of [
      "https://example.com/h.ics",
      " webcal://example.com/h.ics ",
      "webcals://example.com/a?b=c",
    ]) {
      expect(subscriptionUrlProblem(url)).toBeNull();
    }
  });

  it("refuses what the bridge refuses", () => {
    expect(subscriptionUrlProblem("  ")).toBe("empty");
    expect(
      [
        "http://example.com/h.ics",
        "file:///etc/passwd",
        "example.com/h.ics",
        "https://a b/x",
        `https://x/${"a".repeat(2100)}`,
      ].map((url) => subscriptionUrlProblem(url))
    ).toStrictEqual(["scheme", "scheme", "scheme", "scheme", "scheme"]);
    expect(subscriptionUrlProblem("https://user:pw@example.com/h.ics")).toBe(
      "credentials"
    );
  });
});

describe(bridgePatchKey, () => {
  it("escapes the property as a JSON pointer", () => {
    expect(bridgePatchKey(BRIDGE_CALENDAR.isPublished)).toBe(
      "https:~1~1helixnebula.tech~1jmap~1bridge~1isPublished"
    );
  });
});
