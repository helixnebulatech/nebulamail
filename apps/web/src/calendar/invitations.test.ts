// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { AccountId, BlobId, CalendarEventId } from "@nebulamail/jmap-client";
import type { ParsedEvent } from "@nebulamail/jmap-client";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as Api from "./api";
import type { InvitationView, StoredCopy } from "./invitations";
import type { InvitationSender } from "./itip";
import type { Occurrence } from "./model";

const api = vi.hoisted(() => ({
  createEvent: vi.fn<typeof Api.createEvent>(),
  deleteEvent: vi.fn<typeof Api.deleteEvent>(),
  findByUid: vi.fn<typeof Api.findByUid>(),
  listOccurrences: vi.fn<typeof Api.listOccurrences>(),
  updateEvent: vi.fn<typeof Api.updateEvent>(),
}));

vi.mock(import("./api"), async (importOriginal) => ({
  ...(await importOriginal()),
  ...api,
}));

const { applyReply, OccurrenceGoneError, removeCancelled, replyRecipient } =
  await import("./invitations");
const { analyzeInvitation, cardAction } = await import("./itip");

const ALICE = "alice@example.com";
const BOB = "bob@example.com";
const CAROL = "carol@example.com";
const seriesId = CalendarEventId.parse("series");
const occurrenceIdOf = CalendarEventId.parse("series-occ");
const account = AccountId.parse("c");
const START = Date.UTC(2026, 10, 10, 10);
const options = { calendars: [], own: [BOB], zone: "Europe/Madrid" };

const occurrence = (id: CalendarEventId, start: number): Occurrence => ({
  accountId: account,
  allDay: false,
  baseId: seriesId,
  calendarId: null,
  calendarKey: null,
  color: "#000",
  end: start + 3_600_000,
  endDay: null,
  firstDay: null,
  hasParticipants: true,
  id,
  isOrganizer: true,
  location: null,
  myStatus: null,
  recurring: true,
  start,
  status: "confirmed",
  title: "Planning",
});

const event = (
  method: string,
  participants: ParsedEvent["participants"],
  recurrenceId: string | null
): ParsedEvent => ({
  "@type": "Event",
  duration: "PT1H",
  method,
  organizerCalendarAddress: `mailto:${BOB}`,
  participants,
  ...(recurrenceId === null ? {} : { recurrenceId }),
  sequence: 1,
  start: "2026-11-10T11:00:00",
  timeZone: "Europe/Madrid",
  title: "Planning",
  uid: "u1",
});

const view = (
  parsed: ParsedEvent,
  own: readonly string[],
  stored: StoredCopy,
  sender: InvitationSender
): InvitationView => {
  const invitation = analyzeInvitation([parsed], own);
  if (invitation === null) {
    throw new Error("fixture");
  }
  return {
    action: cardAction(invitation, stored, sender),
    blobId: BlobId.parse("blob"),
    conflicts: [],
    invitation,
    myStatus: null,
    occurrence: occurrence(CalendarEventId.parse("invitation"), START),
    outdated: false,
    sender,
    senderBlocked: false,
    stored,
  };
};

/** Bob's event in Bob's calendar: Alice and Carol are invited. */
const bobsCopy: StoredCopy = {
  cancelled: false,
  duration: "PT1H",
  id: seriesId,
  organizer: BOB,
  organizerIsMe: true,
  participants: {
    b: {
      calendarAddress: `mailto:${BOB}`,
      roles: { owner: true },
    },
    x: {
      calendarAddress: `mailto:${ALICE}`,
      participationStatus: "needs-action",
    },
    y: {
      calendarAddress: `mailto:${CAROL}`,
      participationStatus: "needs-action",
    },
  },
  sequence: 1,
  start: "2026-11-03T11:00:00",
  timeZone: "Europe/Madrid",
};

/** Bob's event as Alice has it. */
const alicesCopy: StoredCopy = {
  ...bobsCopy,
  organizerIsMe: false,
};

const reply = (recurrenceId: string | null) =>
  event(
    "reply",
    {
      a: {
        calendarAddress: `mailto:${ALICE}`,
        participationStatus: "accepted",
      },
      // Copied in by Alice: Carol didn't answer.
      c: {
        calendarAddress: `mailto:${CAROL}`,
        participationStatus: "declined",
      },
    },
    recurrenceId
  );

describe("invitation actions", () => {
  beforeEach(() => {
    for (const mock of Object.values(api)) {
      mock.mockReset();
    }
    api.deleteEvent.mockResolvedValue();
    api.updateEvent.mockResolvedValue();
    api.listOccurrences.mockResolvedValue([
      occurrence(CalendarEventId.parse("other"), START - 86_400_000),
      occurrence(occurrenceIdOf, START),
    ]);
  });

  it("removes only the cancelled occurrence", async () => {
    const cancel = view(
      event("cancel", {}, "2026-11-10T11:00:00"),
      [ALICE],
      alicesCopy,
      { email: BOB, verdict: null }
    );
    expect(cancel.action).toBe("remove");
    await removeCancelled(cancel, { ...options, own: [ALICE] });
    expect(api.deleteEvent).toHaveBeenCalledWith(occurrenceIdOf, false);
  });

  it("removes the whole series for a cancelled series", async () => {
    const cancel = view(event("cancel", {}, null), [ALICE], alicesCopy, {
      email: BOB,
      verdict: null,
    });
    await removeCancelled(cancel, { ...options, own: [ALICE] });
    expect(api.deleteEvent).toHaveBeenCalledWith(seriesId, false);
  });

  it("removes nothing when the occurrence is gone", async () => {
    api.listOccurrences.mockResolvedValue([]);
    const cancel = view(
      event("cancel", {}, "2026-11-10T11:00:00"),
      [ALICE],
      alicesCopy,
      { email: BOB, verdict: null }
    );
    await expect(
      removeCancelled(cancel, { ...options, own: [ALICE] })
    ).rejects.toBeInstanceOf(OccurrenceGoneError);
    expect(api.deleteEvent).not.toHaveBeenCalled();
  });

  it("removes nothing for a cancellation from someone else", async () => {
    const cancel = view(event("cancel", {}, null), [ALICE], alicesCopy, {
      email: "mallory@evil.example",
      verdict: null,
    });
    expect(cancel.action).toBe("none");
    await removeCancelled(cancel, { ...options, own: [ALICE] });
    expect(api.deleteEvent).not.toHaveBeenCalled();
  });

  it("applies only the sender's answer", async () => {
    const answer = view(
      { ...reply(null), updated: "2026-11-01T10:11:12Z" },
      [BOB],
      bobsCopy,
      { email: ALICE, verdict: null }
    );
    expect(answer.action).toBe("apply-reply");
    await applyReply(answer, options);
    expect(api.updateEvent).toHaveBeenCalledExactlyOnceWith(
      seriesId,
      {
        "participants/x/participationStatus": "accepted",
        // RFC 5546 §2.1.5: the SEQUENCE and DTSTAMP of the response.
        "participants/x/scheduleSequence": 1,
        "participants/x/scheduleUpdated": "2026-11-01T10:11:12Z",
      },
      false
    );
  });

  it("applies an answer for one occurrence to that occurrence", async () => {
    const answer = view(reply("2026-11-10T11:00:00"), [BOB], bobsCopy, {
      email: ALICE,
      verdict: null,
    });
    await applyReply(answer, options);
    expect(api.updateEvent).toHaveBeenCalledExactlyOnceWith(
      occurrenceIdOf,
      {
        "participants/x/participationStatus": "accepted",
        "participants/x/scheduleSequence": 1,
        start: "2026-11-10T11:00:00",
      },
      false
    );
  });

  it("answers the stored organizer, not the one the .ics names", () => {
    const forged = view(
      {
        ...event(
          "request",
          {
            a: { calendarAddress: `mailto:${ALICE}` },
            m: {
              calendarAddress: "mailto:mallory@evil.example",
              roles: { owner: true },
            },
          },
          null
        ),
        organizerCalendarAddress: "mailto:mallory@evil.example",
      },
      [ALICE],
      alicesCopy,
      { email: BOB, verdict: null }
    );
    expect(replyRecipient(forged)).toStrictEqual({ email: BOB, name: null });
  });
});
