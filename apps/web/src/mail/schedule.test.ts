// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Scheduled send's rules: the server's limit, the quick times in the
// user's zone, the checks on a chosen time and the HOLDUNTIL value.

import { CalendarDate, Time } from "@internationalized/date";
import {
  EmailId,
  EmailSubmissionId,
  IdentityId,
} from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import type { ScheduledEmail } from "./schedule";
import {
  MIN_LEAD_MS,
  checkSendAt,
  holdUntil,
  latestAllowed,
  maximumInRefusal,
  pickedTime,
  quickTimes,
  scheduleLimit,
  scheduledByEmail,
} from "./schedule";

const DAY = 86_400;

describe(scheduleLimit, () => {
  it("is null without FUTURERELEASE or without delayed send", () => {
    expect(scheduleLimit(undefined)).toBeNull();
    expect(
      scheduleLimit({ maxDelayedSend: DAY, submissionExtensions: {} })
    ).toBeNull();
    expect(
      scheduleLimit({
        maxDelayedSend: 0,
        submissionExtensions: { FUTURERELEASE: ["86400"] },
      })
    ).toBeNull();
  });

  it("takes maxDelayedSend, like Stalwart's FUTURERELEASE without arguments", () => {
    expect(
      scheduleLimit({
        maxDelayedSend: 30 * DAY,
        submissionExtensions: { FUTURERELEASE: [] },
      })
    ).toStrictEqual({ latest: null, maxSeconds: 30 * DAY });
  });

  it("takes the lower interval and the latest date of the arguments (RFC 4865)", () => {
    expect(
      scheduleLimit({
        maxDelayedSend: 30 * DAY,
        submissionExtensions: {
          FUTURERELEASE: ["604800", "2026-12-01T00:00:00Z"],
        },
      })
    ).toStrictEqual({
      latest: new Date("2026-12-01T00:00:00Z"),
      maxSeconds: 7 * DAY,
    });
  });

  it("ignores arguments that aren't numbers or UTC date-times", () => {
    expect(
      scheduleLimit({
        maxDelayedSend: DAY,
        submissionExtensions: { FUTURERELEASE: ["-5", "tomorrow"] },
      })
    ).toStrictEqual({ latest: null, maxSeconds: DAY });
  });
});

describe(checkSendAt, () => {
  const now = new Date("2026-11-02T10:00:00Z");
  const limit = { latest: null, maxSeconds: 7 * DAY };

  it("accepts a time between a minute ahead and the limit", () => {
    expect(
      checkSendAt(new Date(now.getTime() + MIN_LEAD_MS), now, limit)
    ).toBeNull();
    expect(
      checkSendAt(new Date("2026-11-08T10:00:00Z"), now, limit)
    ).toBeNull();
  });

  it("refuses past or too close times", () => {
    expect(checkSendAt(new Date("2026-11-02T09:00:00Z"), now, limit)).toBe(
      "past"
    );
    expect(checkSendAt(new Date("2026-11-02T10:00:30Z"), now, limit)).toBe(
      "past"
    );
  });

  it("refuses times past the limit, with a margin for clock differences", () => {
    expect(checkSendAt(new Date("2026-11-09T10:00:00Z"), now, limit)).toBe(
      "tooFar"
    );
    expect(latestAllowed(now, limit)).toStrictEqual(
      new Date("2026-11-09T09:55:00Z")
    );
    expect(
      checkSendAt(new Date("2026-11-05T00:00:00Z"), now, {
        latest: new Date("2026-11-04T00:00:00Z"),
        maxSeconds: 30 * DAY,
      })
    ).toBe("tooFar");
  });

  it("refuses an invalid date", () => {
    expect(checkSendAt(new Date(Number.NaN), now, limit)).toBe("invalid");
  });
});

describe(holdUntil, () => {
  it("is UTC with whole seconds", () => {
    expect(holdUntil(new Date("2026-11-03T08:00:00.987+01:00"))).toBe(
      "2026-11-03T07:00:00Z"
    );
  });
});

describe(quickTimes, () => {
  const zone = "Europe/Madrid";
  const ids = (now: string) =>
    quickTimes(new Date(now), zone).map((time) => [
      time.id,
      time.at.toISOString(),
    ]);

  it("offers this afternoon, tomorrow and Monday in the user's zone", () => {
    // Wednesday 4 November, 9:00 in Madrid (UTC+1).
    expect(ids("2026-11-04T08:00:00Z")).toStrictEqual([
      ["thisAfternoon", "2026-11-04T13:00:00.000Z"],
      ["tomorrowMorning", "2026-11-05T07:00:00.000Z"],
      ["tomorrowAfternoon", "2026-11-05T13:00:00.000Z"],
      ["monday", "2026-11-09T07:00:00.000Z"],
    ]);
  });

  it("hides this afternoon once it's (nearly) past", () => {
    // 13:55 in Madrid.
    expect(ids("2026-11-04T12:55:00Z").map(([id]) => id)).toStrictEqual([
      "tomorrowMorning",
      "tomorrowAfternoon",
      "monday",
    ]);
  });

  it("leaves Monday out on Sundays (it would be tomorrow morning) and offers next week's on Mondays", () => {
    expect(ids("2026-11-08T08:00:00Z").map(([id]) => id)).not.toContain(
      "monday"
    );
    expect(ids("2026-11-09T08:00:00Z").at(-1)).toStrictEqual([
      "monday",
      "2026-11-16T07:00:00.000Z",
    ]);
  });

  it("follows the zone's clock across a DST change", () => {
    // Saturday 24 October 2026 (UTC+2); Monday 26 is after the change (UTC+1).
    expect(ids("2026-10-24T08:00:00Z").at(-1)).toStrictEqual([
      "monday",
      "2026-10-26T07:00:00.000Z",
    ]);
  });
});

describe(maximumInRefusal, () => {
  it("reads Stalwart's maximum from its refusal", () => {
    expect(
      maximumInRefusal(
        "Server rejected MAIL-FROM: 501 5.5.4 Requested release time exceeds maximum of 2026-11-09T20:06:18Z."
      )
    ).toStrictEqual(new Date("2026-11-09T20:06:18Z"));
  });

  it("is null for anything else", () => {
    expect(maximumInRefusal("HOLDUNTIL must be in the future.")).toBeNull();
    expect(maximumInRefusal("exceeds maximum of soon.")).toBeNull();
    expect(maximumInRefusal(null)).toBeNull();
  });
});

describe(pickedTime, () => {
  it("is the instant of the date and time in the zone", () => {
    expect(
      pickedTime(
        new CalendarDate(2026, 11, 3),
        new Time(8, 30),
        "Europe/Madrid"
      )
    ).toStrictEqual(new Date("2026-11-03T07:30:00Z"));
  });

  it("needs both", () => {
    expect(pickedTime(null, new Time(8), "UTC")).toBeNull();
    expect(pickedTime(new CalendarDate(2026, 11, 3), null, "UTC")).toBeNull();
  });
});

const item = (
  emailId: string,
  sendAt: string,
  held = true
): ScheduledEmail => ({
  emailId: EmailId.parse(emailId),
  holdUntil: held,
  identityId: IdentityId.parse("i1"),
  mailFrom: "alice@example.com",
  recipients: ["bob@example.com"],
  sendAt,
  sendWhenSignedOut: false,
  subject: "Later",
  submissionId: EmailSubmissionId.parse(`s-${emailId}`),
});

describe(scheduledByEmail, () => {
  const NOW = new Date("2026-11-03T10:00:00Z");

  it("maps each email to when it goes out", () => {
    const marks = scheduledByEmail(
      [item("m1", "2026-11-04T07:00:00Z"), item("m2", "2026-11-03T13:00:00Z")],
      NOW
    );
    expect([...marks]).toStrictEqual([
      ["m1", "2026-11-04T07:00:00Z"],
      ["m2", "2026-11-03T13:00:00Z"],
    ]);
  });

  it("leaves out times already past and undo send's few seconds", () => {
    const marks = scheduledByEmail(
      [
        item("past", "2026-11-03T09:59:00Z"),
        item("now", "2026-11-03T10:00:00Z"),
        // HOLDFOR 10 s: not a scheduled send.
        item("undo", "2026-11-03T10:00:10Z", false),
        // HOLDUNTIL in its last minute: still scheduled.
        item("soon", "2026-11-03T10:00:30Z"),
        // A server that drops the parameters: far ahead still counts.
        item("dropped", "2026-11-03T12:00:00Z", false),
        item("bad", "tomorrow"),
      ],
      NOW
    );
    expect([...marks.keys()]).toStrictEqual(["soon", "dropped"]);
  });

  it("an email submitted twice shows the soonest", () => {
    const marks = scheduledByEmail(
      [item("m1", "2026-11-05T07:00:00Z"), item("m1", "2026-11-04T07:00:00Z")],
      NOW
    );
    expect(marks.get(EmailId.parse("m1"))).toBe("2026-11-04T07:00:00Z");
  });
});
