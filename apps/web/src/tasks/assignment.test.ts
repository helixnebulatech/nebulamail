// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { AccountId, CalendarEventId } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import {
  buildTaskCounter,
  buildTaskDeclineCounter,
  buildTaskDelegation,
  buildTaskReply,
  buildTaskRequest,
  responseStamp,
} from "../calendar/itip";
import {
  answerOfProgress,
  answerPatch,
  assigneeStatus,
  assigneesPatch,
  assignmentOf,
  counterAuthor,
  counterDuePatch,
  delegationOf,
  mayDelegate,
  replyDelegates,
  replyPatch,
  scheduleChange,
  fromSender,
  taskCardAction,
  taskSenderBlocked,
  taskToImport,
} from "./assignment";
import { itipDue } from "./assignment-api";
import { repeats, taskSeries } from "./itip-series";
import type { RawTask } from "./model";

const ALICE = "alice@example.com";
const BOB = "bob@example.com";
const DANA = "dana@example.com";
const CAROL = "carol@example.com";
const id = CalendarEventId.parse("t1");
/** Who sent the email (one From, no DKIM/DMARC verdict). */
const from = (email: string) => ({ email, verdict: null });
const accountId = AccountId.parse("a");

const assigned = (bob: Record<string, unknown> = {}): RawTask => ({
  "@type": "Task",
  id,
  organizerCalendarAddress: `mailto:${ALICE}`,
  participants: {
    o: {
      calendarAddress: `mailto:${ALICE}`,
      name: "Alice",
      participationStatus: "accepted",
      roles: { attendee: true, owner: true },
    },
    pbob: {
      calendarAddress: `mailto:${BOB}`,
      name: "Bob",
      participationStatus: "needs-action",
      roles: { attendee: true },
      ...bob,
    },
  },
  sequence: 2,
  title: "Informe",
  uid: "t1@uid",
});

/**
 * A weekly task assigned to Bob, as the app stores a repeating task with
 * Stalwart (`start` = `due`, alerts relative to the due), with one Friday
 * taken out, one done, one moved and one with only an alert of its own.
 */
const weekly = (bob: Record<string, unknown> = {}): RawTask => ({
  ...assigned(bob),
  alerts: {
    a: {
      "@type": "Alert",
      relativeTo: "end",
      trigger: { "@type": "OffsetTrigger", offset: "-PT1H", relativeTo: "end" },
    },
  },
  due: "2026-11-06T09:00:00",
  recurrenceOverrides: {
    "2026-11-13T09:00:00": {
      progress: "completed",
      start: "2026-11-13T09:00:00",
    },
    "2026-11-20T09:00:00": { excluded: true },
    "2026-11-27T09:00:00": {
      due: "2026-11-28T12:00:00",
      start: "2026-11-28T12:00:00",
      title: "Informe (sábado)",
    },
    "2026-12-04T09:00:00": {
      "alerts/b": { "@type": "Alert" },
      "participants/pbob/participationStatus": "tentative",
    },
  },
  recurrenceRule: {
    "@type": "RecurrenceRule",
    byDay: [{ day: "fr" }],
    frequency: "weekly",
    until: "2026-12-31T23:59:59",
  },
  start: "2026-11-06T09:00:00",
  timeZone: "Europe/Madrid",
});

describe(assignmentOf, () => {
  it("reads who assigned it and to whom", () => {
    const alice = assignmentOf(assigned(), [ALICE]);
    expect(alice?.iAmOrganizer).toBeTruthy();
    expect(alice?.organizer).toStrictEqual({ email: ALICE, name: "Alice" });
    expect(alice?.assignees.map((item) => item.email)).toStrictEqual([BOB]);
    expect(alice?.me).toBeNull();
  });

  it("knows when the task is assigned to the user", () => {
    const bob = assignmentOf(assigned(), [BOB]);
    expect(bob?.iAmOrganizer).toBeFalsy();
    expect(bob?.me?.key).toBe("pbob");
  });

  it("is null without anyone else", () => {
    expect(assignmentOf({ participants: null }, [ALICE])).toBeNull();
    expect(
      assignmentOf(
        {
          ...assigned(),
          participants: { o: assigned().participants?.o ?? {} },
        },
        [ALICE]
      )
    ).toBeNull();
  });

  it("reads statuses and progress (PARTSTAT COMPLETED, IN-PROCESS)", () => {
    expect(assigneeStatus({ participationStatus: "declined" })).toBe(
      "declined"
    );
    expect(
      assigneeStatus({ participationStatus: "accepted", progress: "completed" })
    ).toBe("completed");
    expect(assigneeStatus({ progress: "in-process" })).toBe("in-process");
    expect(assigneeStatus({ participationStatus: "accepted" })).toBe(
      "accepted"
    );
    expect(assigneeStatus({})).toBe("needs-action");
  });
});

describe("patches", () => {
  it("assigns with the user as organizer, keeping answers", () => {
    const me = { email: ALICE, name: "Alice" };
    const patch = assigneesPatch(
      assigned({ participationStatus: "accepted" }),
      [
        { email: BOB, name: "Bob" },
        { email: "carol@example.com", name: null },
      ],
      me
    );
    expect(patch.organizerCalendarAddress).toBe(`mailto:${ALICE}`);
    expect(patch.participants).toMatchObject({
      pbob: { participationStatus: "accepted" },
    });
    expect(Object.keys(patch.participants ?? {})).toHaveLength(3);
    expect(assigneesPatch(assigned(), [], me)).toStrictEqual({
      organizerCalendarAddress: null,
      participants: null,
    });
  });

  it("records an answer", () => {
    const done = answerPatch(assigned(), "pbob", "completed");
    expect(done["participants/pbob"]).toMatchObject({
      participationStatus: "accepted",
      progress: "completed",
    });
    const declined = answerPatch(
      assigned({ progress: "in-process" }),
      "pbob",
      "declined"
    );
    expect(declined["participants/pbob"]).not.toHaveProperty("progress");
    expect(answerOfProgress("needs-action")).toBe("accepted");
    expect(answerOfProgress("completed")).toBe("completed");
  });
});

describe("progressUpdated of an assignee (RFC 8984 §4.4.6, §5.2.6)", () => {
  const now = Date.UTC(2026, 10, 8, 9, 30);
  const stamp = "2026-11-08T09:30:00Z";

  it("is the time the user's own progress was set", () => {
    expect(
      answerPatch(assigned(), "pbob", "in-process", now)["participants/pbob"]
    ).toMatchObject({ progress: "in-process", progressUpdated: stamp });
    // The same progress again keeps the time it was set.
    const old = "2026-11-01T08:00:00Z";
    expect(
      answerPatch(
        assigned({
          participationStatus: "accepted",
          progress: "in-process",
          progressUpdated: old,
        }),
        "pbob",
        "in-process",
        now
      )["participants/pbob"]
    ).toMatchObject({ progressUpdated: old });
  });

  it("goes with the progress: none without one", () => {
    const busy = assigned({
      participationStatus: "accepted",
      progress: "in-process",
      progressUpdated: "2026-11-01T08:00:00Z",
    });
    for (const answer of ["declined", "accepted"] as const) {
      const entry = answerPatch(busy, "pbob", answer, now)["participants/pbob"];
      expect(entry).not.toHaveProperty("progress");
      expect(entry).not.toHaveProperty("progressUpdated");
    }
    const passed = delegationOf(busy, "pbob", { email: DANA, name: null }, [
      BOB,
    ]);
    expect(passed?.participants.pbob).not.toHaveProperty("progressUpdated");
  });

  it("is the same moment as the task's own when an assignee completes it", () => {
    const result = scheduleChange(
      {
        accountId,
        update: { [id]: { progress: "completed", progressUpdated: stamp } },
      },
      new Map([[id, assigned()]]),
      [BOB]
    );
    expect(result.change.update?.[id]).toMatchObject({
      "participants/pbob": { progress: "completed", progressUpdated: stamp },
      progressUpdated: stamp,
    });
  });

  it("comes from a REPLY if it says one, else is when it is recorded", () => {
    const said = "2026-11-07T18:00:00Z";
    const reply = (extra: Record<string, unknown>) =>
      assigned({
        participationStatus: "accepted",
        progress: "completed",
        ...extra,
      });
    expect(
      replyPatch(assigned(), reply({ progressUpdated: said }), [ALICE], now)[
        "participants/pbob"
      ]
    ).toMatchObject({ progressUpdated: said });
    // Not a UTCDateTime (§1.4.4): not copied.
    for (const bad of ["2026-11-07T18:00:00", "2026-11-07T18:00:00.000Z", 5]) {
      expect(
        replyPatch(assigned(), reply({ progressUpdated: bad }), [ALICE], now)[
          "participants/pbob"
        ]
      ).toMatchObject({ progressUpdated: stamp });
    }
  });
});

describe(scheduleChange, () => {
  it("notifies when the organizer changes or deletes the task", () => {
    const before = new Map([[id, assigned()]]);
    const changed = scheduleChange(
      { accountId, update: { [id]: { title: "Otro" } } },
      before,
      [ALICE]
    );
    expect(changed.change.notify).toBeTruthy();
    expect(changed.replies).toStrictEqual([]);
    expect(
      scheduleChange({ accountId, destroy: [id] }, before, [ALICE]).change
        .notify
    ).toBeTruthy();
  });

  it("notifies when a patch assigns a task", () => {
    const plain: RawTask = { "@type": "Task", id, title: "x" };
    const patch = assigneesPatch(plain, [{ email: BOB, name: null }], {
      email: ALICE,
      name: null,
    });
    const result = scheduleChange(
      { accountId, update: { [id]: patch } },
      new Map([[id, plain]]),
      [ALICE]
    );
    expect(result.change.notify).toBeTruthy();
    // Not assigned: nothing changes.
    const quiet = scheduleChange(
      { accountId, update: { [id]: { title: "y" } } },
      new Map([[id, plain]]),
      [ALICE]
    );
    expect(quiet.change.notify).toBeUndefined();
  });

  it("an assignee completing records it and asks for a REPLY", () => {
    const result = scheduleChange(
      { accountId, update: { [id]: { progress: "completed" } } },
      new Map([[id, assigned()]]),
      [BOB]
    );
    expect(result.change.notify).toBeUndefined();
    expect(result.change.update?.[id]).toMatchObject({
      "participants/pbob": { progress: "completed" },
      progress: "completed",
    });
    expect(result.replies.map((reply) => reply.answer)).toStrictEqual([
      "completed",
    ]);
  });
});

describe("in mail", () => {
  it("imports only the task's own data, with the answer", () => {
    const copy = taskToImport(
      {
        ...assigned(),
        alerts: { a: {} },
        links: { l: { href: "https://x" } },
      },
      "pbob",
      "accepted"
    );
    expect(copy).not.toHaveProperty("alerts");
    expect(copy).not.toHaveProperty("links");
    expect(copy["@type"]).toBe("Task");
    expect(copy.participants).toMatchObject({
      pbob: { participationStatus: "accepted" },
    });
  });

  it("writes a REPLY's answers into the organizer's task", () => {
    const reply = assigned({
      participationStatus: "accepted",
      progress: "completed",
    });
    const patch = replyPatch(assigned(), reply, [ALICE]);
    expect(patch["participants/pbob"]).toMatchObject({
      participationStatus: "accepted",
      progress: "completed",
    });
    expect(replyPatch(reply, reply, [ALICE])).toStrictEqual({});
  });

  it("never writes a REPLY older than the last one applied", () => {
    // RFC 5546 §2.1.5: higher SEQUENCE, then later DTSTAMP; the organizer
    // keeps both of the last response (RFC 8984 §4.4.6).
    const stored = assigned({
      participationStatus: "declined",
      scheduleSequence: 2,
      scheduleUpdated: "2026-11-01T10:00:00Z",
    });
    const answer = (sequence: number, updated: string): RawTask => ({
      ...assigned({ participationStatus: "accepted" }),
      sequence,
      updated,
    });
    expect(
      replyPatch(stored, answer(2, "2026-11-01T09:00:00Z"), [ALICE])
    ).toStrictEqual({});
    expect(
      replyPatch(stored, answer(1, "2026-11-02T09:00:00Z"), [ALICE])
    ).toStrictEqual({});
    expect(
      replyPatch(stored, answer(2, "2026-11-01T11:00:00Z"), [ALICE])[
        "participants/pbob"
      ]
    ).toMatchObject({
      participationStatus: "accepted",
      scheduleSequence: 2,
      scheduleUpdated: "2026-11-01T11:00:00Z",
    });
    const card = (reply: RawTask) => ({
      assignment: assignmentOf(reply, [ALICE]),
      cancelled: false,
      method: "reply",
      outdated: false,
      own: [ALICE],
      sender: from(BOB),
      stamp: responseStamp(reply),
      stored,
    });
    expect(taskCardAction(card(answer(2, "2026-11-01T09:00:00Z")))).toBe(
      "none"
    );
    expect(taskCardAction(card(answer(3, "2026-10-01T09:00:00Z")))).toBe(
      "apply-reply"
    );
  });

  it("chooses the card's buttons", () => {
    const base = { cancelled: false, own: [BOB], stored: null };
    expect(
      taskCardAction({
        outdated: false,
        sender: from(ALICE),
        ...base,
        assignment: assignmentOf(assigned(), [BOB]),
        method: "request",
      })
    ).toBe("answer");
    expect(
      taskCardAction({
        outdated: false,
        sender: from(ALICE),
        ...base,
        assignment: null,
        method: "cancel",
        stored: assigned(),
      })
    ).toBe("remove");
    const reply = assigned({ progress: "completed" });
    expect(
      taskCardAction({
        outdated: false,
        sender: from(BOB),
        assignment: assignmentOf(reply, [ALICE]),
        cancelled: false,
        method: "reply",
        own: [ALICE],
        stored: assigned(),
      })
    ).toBe("apply-reply");
  });

  it("only takes a task back for its stored organizer", () => {
    const cancel = {
      outdated: false,
      assignment: null,
      cancelled: true,
      method: "cancel",
      own: [BOB],
      stored: assigned(),
    };
    expect(taskCardAction({ ...cancel, sender: from(ALICE) })).toBe("remove");
    // Mallory knows the UID (any assignee does) and writes Alice as the
    // ORGANIZER of her CANCEL: the From is still hers.
    for (const sender of [
      from(CAROL),
      { email: null, verdict: null },
      { email: ALICE, verdict: "fail" as const },
    ]) {
      expect(taskCardAction({ ...cancel, sender })).toBe("none");
      expect(taskSenderBlocked({ ...cancel, sender })).toBeTruthy();
    }
    expect(taskSenderBlocked({ ...cancel, sender: from(ALICE) })).toBeFalsy();
  });

  it("only takes an update of a stored task from its organizer", () => {
    const update = {
      outdated: false,
      assignment: assignmentOf(assigned(), [BOB]),
      cancelled: false,
      method: "request",
      own: [BOB],
      stored: assigned(),
    };
    expect(taskCardAction({ ...update, sender: from(ALICE) })).toBe("answer");
    expect(taskCardAction({ ...update, sender: from(CAROL) })).toBe("none");
  });

  it("applies a REPLY only for the assignee who sent it", () => {
    // Alice's task for Bob and Dana; Dana mails a REPLY saying Bob declines.
    const stored: RawTask = {
      ...assigned(),
      participants: {
        ...assigned().participants,
        pdana: {
          calendarAddress: `mailto:${DANA}`,
          participationStatus: "needs-action",
          roles: { attendee: true },
        },
      },
    };
    const forged: RawTask = {
      ...stored,
      participants: {
        ...stored.participants,
        pbob: { ...stored.participants?.pbob, participationStatus: "declined" },
      },
    };
    const reply = fromSender(forged, from(DANA), stored);
    expect(Object.keys(reply.participants ?? {})).toStrictEqual(["pdana"]);
    expect(replyPatch(stored, reply, [ALICE])).toStrictEqual({});
    const card = {
      outdated: false,
      assignment: assignmentOf(forged, [ALICE]),
      cancelled: false,
      method: "reply",
      own: [ALICE],
      stored,
    };
    // Even with the whole .ics, only the sender's own entry counts.
    expect(taskCardAction({ ...card, sender: from(DANA) })).toBe("none");
    expect(taskCardAction({ ...card, sender: from(BOB) })).toBe("apply-reply");
    expect(
      Object.keys(
        replyPatch(stored, fromSender(forged, from(BOB), stored), [ALICE])
      )
    ).toStrictEqual(["participants/pbob"]);
  });

  it("never lets a sender speak for a stored assignee by delegation", () => {
    // Mallory isn't in Alice's task; she writes Bob as "delegated from
    // Mallory" with a new answer, in a REPLY and in a COUNTER.
    const claim: RawTask = {
      ...assigned(),
      participants: {
        pbob: {
          calendarAddress: `mailto:${BOB}`,
          delegatedFrom: { [`mailto:${CAROL}`]: true },
          participationStatus: "declined",
          roles: { attendee: true },
        },
      },
    };
    const reply = fromSender(claim, from(CAROL), assigned());
    expect(reply.participants).toStrictEqual({});
    expect(replyPatch(assigned(), reply, [ALICE])).toStrictEqual({});
    for (const method of ["reply", "counter"]) {
      const card = {
        assignment: assignmentOf(claim, [ALICE]),
        cancelled: false,
        method,
        outdated: false,
        own: [ALICE],
        sender: from(CAROL),
        stored: assigned(),
      };
      expect(taskCardAction(card)).toBe("none");
      expect(taskSenderBlocked(card)).toBeTruthy();
    }
  });

  it("changes nothing with an email older than the stored task", () => {
    const card = {
      assignment: null,
      cancelled: true,
      method: "cancel",
      outdated: true,
      own: [BOB],
      sender: from(ALICE),
      stored: assigned(),
    };
    expect(taskCardAction(card)).toBe("none");
    expect(taskSenderBlocked(card)).toBeFalsy();
  });

  it("covers what the second review found", () => {
    // The organizer takes back a task with no assignees left.
    const alone: RawTask = {
      ...assigned(),
      participants: { o: assigned().participants?.o ?? {} },
    };
    const base = {
      assignment: null,
      outdated: false,
      own: [BOB],
      sender: from(ALICE),
    };
    expect(
      taskCardAction({
        ...base,
        cancelled: true,
        method: "cancel",
        stored: alone,
      })
    ).toBe("remove");
    // STATUS:CANCELLED in a REPLY is still an answer, not a cancellation.
    const reply = assigned({ participationStatus: "accepted" });
    expect(
      taskCardAction({
        ...base,
        assignment: assignmentOf(reply, [ALICE]),
        cancelled: true,
        method: "reply",
        own: [ALICE],
        sender: from(BOB),
        stored: assigned(),
      })
    ).toBe("apply-reply");
    // No COUNTER for a task that was taken back.
    expect(
      taskCardAction({
        ...base,
        assignment: assignmentOf(assigned(), [ALICE]),
        cancelled: false,
        method: "counter",
        own: [ALICE],
        sender: from(BOB),
        stored: { ...assigned(), progress: "cancelled" },
      })
    ).toBe("none");
  });

  it("stores a delegate as passed on by its sender only", () => {
    const reply = {
      ...assigned(),
      participants: {
        ...assigned({
          delegatedTo: { [`mailto:${DANA}`]: true },
          participationStatus: "delegated",
        }).participants,
        x: {
          calendarAddress: `mailto:${DANA}`,
          delegatedFrom: {
            [`mailto:${BOB}`]: true,
            [`mailto:${CAROL}`]: true,
          },
          participationStatus: "needs-action",
        },
      },
    };
    const patch = replyPatch(assigned(), reply, [ALICE]);
    expect(patch["participants/x"]).toMatchObject({
      delegatedFrom: { [`mailto:${BOB}`]: true },
    });
    expect(patch["participants/x"]).not.toHaveProperty([
      "delegatedFrom",
      `mailto:${CAROL}`,
    ]);
  });

  it("adds a delegate as not answered yet, whatever the REPLY says", () => {
    const reply = {
      ...assigned(),
      participants: {
        ...assigned({
          delegatedTo: { [`mailto:${DANA}`]: true },
          participationStatus: "delegated",
        }).participants,
        x: {
          calendarAddress: `mailto:${DANA}`,
          delegatedFrom: { [`mailto:${BOB}`]: true },
          participationStatus: "accepted",
        },
      },
    };
    expect(
      replyPatch(assigned(), reply, [ALICE])["participants/x"]
    ).toMatchObject({ participationStatus: "needs-action" });
  });

  it("lets a sender that may not act speak for no one", () => {
    expect(
      fromSender(assigned(), { email: BOB, verdict: "fail" }, null).participants
    ).toStrictEqual({});
    expect(
      fromSender(assigned(), { email: null, verdict: "pass" }, null)
        .participants
    ).toStrictEqual({});
  });

  it("builds a VTODO REPLY", () => {
    const ics = buildTaskReply({
      attendee: { email: BOB, name: "Bob" },
      now: Date.UTC(2026, 9, 29, 10),
      organizer: { email: ALICE, name: "Alice" },
      partstat: "COMPLETED",
      recurrenceId: null,
      recurrenceIdTimeZone: null,
      sequence: 2,
      title: "Informe",
      uid: "t1@uid",
    });
    expect(ics).toContain("METHOD:REPLY\r\nBEGIN:VTODO\r\n");
    expect(ics).toMatch(
      /^ATTENDEE;PARTSTAT=COMPLETED;.*:mailto:bob@example\.com$/mu
    );
    expect(ics).toContain("END:VTODO\r\nEND:VCALENDAR\r\n");
    expect(ics).not.toContain("VEVENT");
    // Nothing odd gets into the parameter.
    const odd = buildTaskReply({
      attendee: { email: BOB, name: null },
      now: 0,
      organizer: { email: ALICE, name: null },
      partstat: "X;Y=1",
      recurrenceId: null,
      recurrenceIdTimeZone: null,
      sequence: 0,
      title: "",
      uid: "u",
    });
    expect(odd).toContain("PARTSTAT=ACCEPTED");
  });
});

const ITIP_COMMON = {
  attendee: { email: BOB, name: "Bob" },
  now: Date.UTC(2026, 9, 29, 10),
  organizer: { email: ALICE, name: "Alice" },
  recurrenceId: null,
  recurrenceIdTimeZone: null,
  sequence: 2,
  uid: "t1@uid",
};

describe("proposing another due (COUNTER)", () => {
  it("knows who proposes, only an assignee of the user's task", () => {
    const counter = assignmentOf(assigned(), [ALICE]);
    expect(counterAuthor(counter, assigned(), [ALICE])?.email).toBe(BOB);
    // Not the user's task: nothing to accept.
    expect(counterAuthor(counter, assigned(), [BOB])).toBeNull();
    // From someone who isn't in the task.
    const stranger = assignmentOf(
      {
        ...assigned(),
        participants: {
          ...assigned().participants,
          pbob: { calendarAddress: `mailto:${DANA}` },
        },
      },
      [ALICE]
    );
    expect(counterAuthor(stranger, assigned(), [ALICE])).toBeNull();
    expect(
      taskCardAction({
        outdated: false,
        sender: from(BOB),
        assignment: counter,
        cancelled: false,
        method: "counter",
        own: [ALICE],
        stored: assigned(),
      })
    ).toBe("counter");
    expect(
      taskCardAction({
        outdated: false,
        sender: from(DANA),
        assignment: stranger,
        cancelled: false,
        method: "counter",
        own: [ALICE],
        stored: assigned(),
      })
    ).toBe("none");
  });

  it("moves the task to the proposed due, in the task's zone", () => {
    const stored: RawTask = {
      ...assigned(),
      due: "2026-11-06T17:00:00",
      showWithoutTime: false,
      start: "2026-11-06T09:00:00",
      timeZone: "Europe/Madrid",
    };
    // 10:00 UTC is 11:00 in Madrid in November.
    expect(
      counterDuePatch(
        stored,
        {
          kind: "datetime",
          local: "2026-11-09T10:00:00",
          timeZone: "Etc/UTC",
        },
        "UTC"
      )
    ).toStrictEqual({
      due: "2026-11-09T11:00:00",
      showWithoutTime: false,
      timeZone: "Europe/Madrid",
    });
    // A day: the start (a time) moves with it and the zone goes.
    expect(
      counterDuePatch(stored, { date: "2026-11-05", kind: "date" }, "UTC")
    ).toStrictEqual({
      due: "2026-11-05T00:00:00",
      showWithoutTime: true,
      start: "2026-11-05T00:00:00",
      timeZone: null,
    });
  });

  it("builds a VTODO COUNTER for a day", () => {
    const day = buildTaskCounter({
      ...ITIP_COMMON,
      comment: "Next week?",
      due: { date: "2026-11-09", kind: "date" },
      partstat: "ACCEPTED",
      title: "Informe",
    });
    expect(day).toContain("METHOD:COUNTER\r\nBEGIN:VTODO\r\n");
    expect(day).toContain("DUE;VALUE=DATE:20261109\r\n");
    expect(day).toContain("COMMENT:Next week?\r\n");
    expect(day).toMatch(
      /^ATTENDEE;PARTSTAT=ACCEPTED;.*:mailto:bob@example\.com$/mu
    );
  });

  it("builds a COUNTER with a time, in UTC", () => {
    const at = buildTaskCounter({
      ...ITIP_COMMON,
      due: { at: Date.UTC(2026, 10, 9, 10), kind: "instant" },
      partstat: "X;Y",
      title: "Informe",
    });
    expect(at).toContain("DUE:20261109T100000Z\r\n");
    expect(at).toContain("PARTSTAT=NEEDS-ACTION");
    expect(
      itipDue(
        {
          kind: "datetime",
          local: "2026-11-09T11:00:00",
          timeZone: "Europe/Madrid",
        },
        "UTC"
      )
    ).toStrictEqual({ at: Date.UTC(2026, 10, 9, 10), kind: "instant" });
  });

  it("builds a VTODO DECLINECOUNTER", () => {
    const decline = buildTaskDeclineCounter(ITIP_COMMON);
    expect(decline).toContain("METHOD:DECLINECOUNTER\r\nBEGIN:VTODO\r\n");
    expect(decline).toMatch(/^ATTENDEE;CN="Bob":mailto:bob@example\.com$/mu);
    expect(decline).not.toContain("VEVENT");
  });
});

/** Dana got the task from Bob and accepted it (her copy). */
const danas = (): { readonly task: RawTask; readonly key: string } => {
  const fromBob = delegationOf(
    assigned(),
    "pbob",
    { email: DANA, name: "Dana" },
    [BOB]
  );
  const key = fromBob?.delegateKey ?? "";
  return {
    key,
    task: {
      ...assigned(),
      participants: {
        ...fromBob?.participants,
        [key]: {
          ...fromBob?.participants[key],
          participationStatus: "accepted",
        },
      },
    },
  };
};

describe("passing a task on (DELEGATED)", () => {
  it("marks the user's copy delegated and adds the delegate", () => {
    const delegation = delegationOf(
      assigned({ progress: "in-process" }),
      "pbob",
      { email: "Dana@Example.com", name: "Dana" },
      [BOB]
    );
    expect(delegation).not.toBeNull();
    const key = delegation?.delegateKey ?? "";
    expect(delegation?.patch["participants/pbob"]).toMatchObject({
      delegatedTo: { [`mailto:${DANA}`]: true },
      participationStatus: "delegated",
    });
    expect(delegation?.patch["participants/pbob"]).not.toHaveProperty(
      "progress"
    );
    expect(delegation?.patch[`participants/${key}`]).toMatchObject({
      calendarAddress: `mailto:${DANA}`,
      delegatedFrom: { [`mailto:${BOB}`]: true },
      name: "Dana",
      participationStatus: "needs-action",
    });
  });

  it("reads the delegation back, never to someone already in it", () => {
    const delegation = delegationOf(
      assigned({ progress: "in-process" }),
      "pbob",
      { email: "Dana@Example.com", name: "Dana" },
      [BOB]
    );
    const after = assignmentOf(
      { ...assigned(), participants: delegation?.participants ?? {} },
      [BOB]
    );
    expect(after?.me).toMatchObject({
      delegatedTo: DANA,
      status: "delegated",
    });
    expect(
      after?.assignees.find((person) => person.email === DANA)?.delegatedFrom
    ).toBe(BOB);
    // Not to someone already in it, nor to the user.
    expect(
      delegationOf(assigned(), "pbob", { email: ALICE, name: null }, [BOB])
    ).toBeNull();
    expect(
      delegationOf(assigned(), "pbob", { email: BOB, name: null }, [BOB])
    ).toBeNull();
  });

  it("is offered once, and for a series only if it can be sent", () => {
    expect(mayDelegate(assigned(), [BOB])).toBeTruthy();
    expect(mayDelegate(assigned(), [ALICE])).toBeFalsy();
    expect(
      mayDelegate(assigned({ participationStatus: "delegated" }), [BOB])
    ).toBeFalsy();
    // A repeating task goes as a whole series, if the REQUEST can carry
    // it exactly (no EXRULE, a rule it can write, something to count from).
    expect({
      exrule: mayDelegate(
        {
          ...weekly(),
          excludedRecurrenceRules: [
            { "@type": "RecurrenceRule", frequency: "monthly" },
          ],
        },
        [BOB]
      ),
      noDates: mayDelegate({ ...weekly(), due: null, start: null }, [BOB]),
      oddRule: mayDelegate(
        {
          ...weekly(),
          recurrenceRule: { "@type": "RecurrenceRule", frequency: "often" },
        },
        [BOB]
      ),
      series: mayDelegate(weekly(), [BOB]),
    }).toStrictEqual({
      exrule: false,
      noDates: false,
      oddRule: false,
      series: true,
    });
  });

  it("writes a delegation REPLY into the organizer's task", () => {
    const reply = {
      ...assigned({
        delegatedTo: { [`mailto:${DANA}`]: true },
        participationStatus: "delegated",
      }),
      participants: {
        ...assigned({
          delegatedTo: { [`mailto:${DANA}`]: true },
          participationStatus: "delegated",
        }).participants,
        x: {
          calendarAddress: `mailto:${DANA}`,
          delegatedFrom: { [`mailto:${BOB}`]: true },
          name: "Dana",
          participationStatus: "needs-action",
        },
      },
    };
    const patch = replyPatch(assigned(), reply, [ALICE]);
    expect(patch["participants/pbob"]).toMatchObject({
      delegatedTo: { [`mailto:${DANA}`]: true },
      participationStatus: "delegated",
    });
    expect(patch["participants/x"]).toMatchObject({
      calendarAddress: `mailto:${DANA}`,
      delegatedFrom: { [`mailto:${BOB}`]: true },
    });
    expect(
      taskCardAction({
        outdated: false,
        sender: from(BOB),
        assignment: assignmentOf(reply, [ALICE]),
        cancelled: false,
        method: "reply",
        own: [ALICE],
        stored: assigned(),
      })
    ).toBe("apply-reply");
    // Shown before applying: the email may not be Bob's.
    expect(
      replyDelegates(assigned(), reply, [ALICE]).map((person) => [
        person.email,
        person.delegatedFrom,
      ])
    ).toStrictEqual([[DANA, BOB]]);
    // Someone saying they were delegated, without the assignee saying so.
    const claim = {
      ...assigned(),
      participants: { ...assigned().participants, x: reply.participants.x },
    };
    expect({
      delegates: replyDelegates(assigned(), claim, [ALICE]),
      patch: replyPatch(assigned(), claim, [ALICE]),
    }).toStrictEqual({ delegates: [], patch: {} });
  });

  it("builds the REPLY to the organizer", () => {
    const common = {
      attendee: { email: BOB, name: "Bob" },
      now: Date.UTC(2026, 9, 29, 10),
      organizer: { email: ALICE, name: "Alice" },
      recurrenceId: null,
      recurrenceIdTimeZone: null,
      sequence: 2,
      uid: "t1@uid",
    };
    const reply = buildTaskDelegation({
      ...common,
      delegate: { email: DANA, name: "Dana" },
      title: "Informe",
    });
    expect(reply).toContain("METHOD:REPLY\r\nBEGIN:VTODO\r\n");
    const unfolded = reply.replaceAll("\r\n ", "");
    expect(unfolded).toMatch(
      /^ATTENDEE;PARTSTAT=DELEGATED;DELEGATED-TO="mailto:dana@example\.com";CN="Bob":mailto:bob@example\.com$/mu
    );
    expect(unfolded).toMatch(
      /^ATTENDEE;PARTSTAT=NEEDS-ACTION;RSVP=TRUE;DELEGATED-FROM="mailto:bob@example\.com";CN="Dana":mailto:dana@example\.com$/mu
    );
  });

  it("builds the REQUEST to the delegate", () => {
    const request = buildTaskRequest({
      attendees: [
        {
          delegatedFrom: null,
          delegatedTo: DANA,
          email: BOB,
          name: "Bob",
          partstat: "DELEGATED",
          rsvp: false,
        },
        {
          delegatedFrom: BOB,
          delegatedTo: null,
          email: DANA,
          name: "Dana",
          partstat: "NEEDS-ACTION",
          rsvp: true,
        },
      ],
      description: "Línea 1\nLínea 2; fin",
      due: { date: "2026-11-09", kind: "date" },
      now: Date.UTC(2026, 9, 29, 10),
      organizer: { email: ALICE, name: "Alice" },
      sequence: 2,
      title: "Informe",
      uid: "t1@uid",
    });
    const lines = request.replaceAll("\r\n ", "");
    expect(lines).toMatch(
      /METHOD:REQUEST\r\nBEGIN:VTODO\r\n[^]*\r\nDUE;VALUE=DATE:20261109\r\n/u
    );
    expect(lines).toContain("DESCRIPTION:Línea 1\\nLínea 2\\; fin\r\n");
    expect(lines).toMatch(/^ORGANIZER;CN="Alice":mailto:alice@example\.com$/mu);
    expect(lines).toMatch(
      /^ATTENDEE;PARTSTAT=NEEDS-ACTION;RSVP=TRUE;DELEGATED-FROM="mailto:bob@example\.com";CN="Dana":mailto:dana@example\.com$/mu
    );
    expect(lines).not.toContain("RRULE");
  });

  it("passes on again: the chain of DELEGATED-FROM", () => {
    const { key, task } = danas();
    expect(assignmentOf(task, [DANA])?.me).toMatchObject({
      delegatedFrom: BOB,
      status: "accepted",
    });
    expect(mayDelegate(task, [DANA])).toBeTruthy();
    // Not back to Bob, who is already in it.
    expect(
      delegationOf(task, key, { email: BOB, name: null }, [DANA])
    ).toBeNull();
    const toCarol = delegationOf(task, key, { email: CAROL, name: "Carol" }, [
      DANA,
    ]);
    // Dana keeps who passed it to her, and says who she passed it to.
    expect(toCarol?.patch[`participants/${key}`]).toMatchObject({
      delegatedFrom: { [`mailto:${BOB}`]: true },
      delegatedTo: { [`mailto:${CAROL}`]: true },
      participationStatus: "delegated",
    });
    expect(
      assignmentOf({ ...task, participants: toCarol?.participants ?? {} }, [
        DANA,
      ])?.assignees.map((person) => [
        person.email,
        person.status,
        person.delegatedFrom,
        person.delegatedTo,
      ])
    ).toStrictEqual([
      [BOB, "delegated", null, DANA],
      [DANA, "delegated", BOB, CAROL],
      [CAROL, "needs-action", DANA, null],
    ]);
  });

  it("passes on again: the REPLY says both delegations", () => {
    // Dana's REPLY to Alice: her line has both, Carol's says it's from Dana.
    const reply = buildTaskDelegation({
      attendee: { email: DANA, name: "Dana" },
      delegate: { email: CAROL, name: "Carol" },
      delegatedFrom: BOB,
      now: Date.UTC(2026, 10, 1, 10),
      organizer: { email: ALICE, name: "Alice" },
      recurrenceId: null,
      recurrenceIdTimeZone: null,
      sequence: 2,
      title: "Informe",
      uid: "t1@uid",
    }).replaceAll("\r\n ", "");
    expect(reply).toMatch(
      /^ATTENDEE;PARTSTAT=DELEGATED;DELEGATED-TO="mailto:carol@example\.com";DELEGATED-FROM="mailto:bob@example\.com";CN="Dana":mailto:dana@example\.com$/mu
    );
    expect(reply).toMatch(
      /^ATTENDEE;PARTSTAT=NEEDS-ACTION;RSVP=TRUE;DELEGATED-FROM="mailto:dana@example\.com";CN="Carol":mailto:carol@example\.com$/mu
    );
  });

  it("passes on again: the organizer's copy adds the new delegate", () => {
    const bobDelegated = assigned({
      delegatedTo: { [`mailto:${DANA}`]: true },
      participationStatus: "delegated",
    });
    // Alice's copy already had Dana (from Bob's REPLY).
    const alices: RawTask = {
      ...bobDelegated,
      participants: {
        ...bobDelegated.participants,
        pdana: {
          calendarAddress: `mailto:${DANA}`,
          delegatedFrom: { [`mailto:${BOB}`]: true },
          participationStatus: "accepted",
          roles: { attendee: true },
        },
      },
    };
    const parsedReply = {
      ...alices,
      participants: {
        pdana: {
          calendarAddress: `mailto:${DANA}`,
          delegatedFrom: { [`mailto:${BOB}`]: true },
          delegatedTo: { [`mailto:${CAROL}`]: true },
          participationStatus: "delegated",
        },
        x: {
          calendarAddress: `mailto:${CAROL}`,
          delegatedFrom: { [`mailto:${DANA}`]: true },
          participationStatus: "needs-action",
        },
      },
    };
    expect(
      replyDelegates(alices, parsedReply, [ALICE]).map((person) => person.email)
    ).toStrictEqual([CAROL]);
    const patch = replyPatch(alices, parsedReply, [ALICE]);
    expect(patch["participants/pdana"]).toMatchObject({
      delegatedFrom: { [`mailto:${BOB}`]: true },
      delegatedTo: { [`mailto:${CAROL}`]: true },
      participationStatus: "delegated",
    });
    expect(patch["participants/x"]).toMatchObject({
      calendarAddress: `mailto:${CAROL}`,
      delegatedFrom: { [`mailto:${DANA}`]: true },
    });
  });
});

/** A wall time in Madrid, as `taskSeries` reads it. */
const madridAt = (local: string) => ({
  kind: "local",
  local,
  timeZone: "Europe/Madrid",
});

describe("passing on a repeating task (the whole series)", () => {
  it("reads the series as the REQUEST carries it", () => {
    const series = taskSeries(weekly());
    // Every occurrence an override names, but the one taken out: an RDATE
    // too (RFC 8984 §4.3.5, RFC 5545 §3.8.5.2: duplicates are ignored).
    expect(series).toStrictEqual({
      added: [
        madridAt("2026-11-13T09:00:00"),
        madridAt("2026-11-27T09:00:00"),
        madridAt("2026-12-04T09:00:00"),
      ],
      due: {
        kind: "local",
        local: "2026-11-06T09:00:00",
        timeZone: "Europe/Madrid",
      },
      excluded: [
        {
          kind: "local",
          local: "2026-11-20T09:00:00",
          timeZone: "Europe/Madrid",
        },
      ],
      overrides: [
        {
          completed: true,
          due: {
            kind: "local",
            local: "2026-11-13T09:00:00",
            timeZone: "Europe/Madrid",
          },
          recurrenceId: {
            kind: "local",
            local: "2026-11-13T09:00:00",
            timeZone: "Europe/Madrid",
          },
          start: {
            kind: "local",
            local: "2026-11-13T09:00:00",
            timeZone: "Europe/Madrid",
          },
          title: null,
        },
        {
          completed: false,
          due: {
            kind: "local",
            local: "2026-11-28T12:00:00",
            timeZone: "Europe/Madrid",
          },
          recurrenceId: {
            kind: "local",
            local: "2026-11-27T09:00:00",
            timeZone: "Europe/Madrid",
          },
          start: {
            kind: "local",
            local: "2026-11-28T12:00:00",
            timeZone: "Europe/Madrid",
          },
          title: "Informe (sábado)",
        },
      ],
      // Until the end of 31 December in Madrid, in UTC.
      rrule: "FREQ=WEEKLY;BYDAY=FR;UNTIL=20261231T225959Z",
      start: {
        kind: "local",
        local: "2026-11-06T09:00:00",
        timeZone: "Europe/Madrid",
      },
    });
    // Lost: what the 4 December override changed (an alert and Bob's own
    // answer, never sent), and the series' alerts; only its date stays.
    expect(
      series?.overrides.map((item) => item.recurrenceId)
    ).not.toContainEqual(madridAt("2026-12-04T09:00:00"));
    expect(repeats(weekly())).toBeTruthy();
    expect(repeats(assigned())).toBeFalsy();
    expect(taskSeries(assigned())).toBeNull();
  });

  it("sends a series of days as dates, and one with only a due", () => {
    const days = taskSeries({
      ...weekly(),
      due: "2026-11-06T00:00:00",
      recurrenceOverrides: { "2026-11-20T00:00:00": { excluded: true } },
      showWithoutTime: true,
      start: "2026-11-06T00:00:00",
      timeZone: null,
    });
    expect(days).toMatchObject({
      due: { date: "2026-11-06", kind: "date" },
      excluded: [{ date: "2026-11-20", kind: "date" }],
      rrule: "FREQ=WEEKLY;BYDAY=FR;UNTIL=20261231",
      start: { date: "2026-11-06", kind: "date" },
    });
    // tasks.org writes only DUE: the rule counts from it.
    expect(
      taskSeries({ ...weekly(), recurrenceOverrides: null, start: null })
    ).toMatchObject({ start: { local: "2026-11-06T09:00:00" } });
  });

  it("keeps an added occurrence (an empty override) as an RDATE", () => {
    // A Thursday the Friday rule doesn't make (RFC 8984 §4.3.5).
    const series = taskSeries({
      ...weekly(),
      recurrenceOverrides: { "2026-11-12T09:00:00": {} },
    });
    expect(series?.added).toStrictEqual([
      {
        kind: "local",
        local: "2026-11-12T09:00:00",
        timeZone: "Europe/Madrid",
      },
    ]);
    const days = taskSeries({
      ...weekly(),
      due: "2026-11-06T00:00:00",
      recurrenceOverrides: { "2026-11-12T00:00:00": {} },
      showWithoutTime: true,
      start: "2026-11-06T00:00:00",
      timeZone: null,
    });
    expect(days?.added).toStrictEqual([{ date: "2026-11-12", kind: "date" }]);
    expect(
      taskToImport(
        { ...weekly(), recurrenceOverrides: { "2026-11-12T09:00:00": {} } },
        "pbob",
        "accepted"
      ).recurrenceOverrides
    ).toStrictEqual({ "2026-11-12T09:00:00": {} });
  });

  it("an occurrence without its own due gets the series' gap", () => {
    const series = taskSeries({
      ...weekly(),
      due: "2026-11-06T17:00:00",
      recurrenceOverrides: {
        "2026-11-13T09:00:00": { start: "2026-11-14T10:00:00" },
      },
    });
    expect(series?.overrides[0]?.due).toStrictEqual({
      kind: "local",
      local: "2026-11-14T18:00:00",
      timeZone: "Europe/Madrid",
    });
  });

  it("refuses what it can't send exactly", () => {
    expect(
      taskSeries({
        ...weekly(),
        recurrenceOverrides: {
          "2026-11-13T09:00:00": {
            start: "2026-11-13T10:00:00",
            timeZone: "Etc/UTC",
          },
        },
      })
    ).toBeNull();
    expect(
      taskSeries({ ...weekly(), recurrenceOverrides: { nonsense: {} } })
    ).toBeNull();
    expect(taskSeries({ ...weekly(), timeZone: "Mars/Olympus" })).toBeNull();
  });

  it("builds the REQUEST to the delegate with the series", () => {
    const series = taskSeries(weekly());
    const request = buildTaskRequest({
      attendees: [],
      description: null,
      due: null,
      now: Date.UTC(2026, 10, 1, 10),
      organizer: { email: ALICE, name: "Alice" },
      sequence: 2,
      series,
      title: "Informe",
      uid: "t1@uid",
    }).replaceAll("\r\n ", "");
    expect(request).toContain("BEGIN:VTIMEZONE\r\nTZID:Europe/Madrid\r\n");
    expect(request).toContain(
      "RRULE:FREQ=WEEKLY;BYDAY=FR;UNTIL=20261231T225959Z\r\n"
    );
    expect(request).toContain("EXDATE;TZID=Europe/Madrid:20261120T090000\r\n");
    expect(request).not.toContain("VALARM");
    // 4 December only as an occurrence of the set (its alert never goes).
    expect(
      request.split("\r\n").filter((line) => line.includes("20261204"))
    ).toStrictEqual(["RDATE;TZID=Europe/Madrid:20261204T090000"]);
  });

  it("imports the series when the delegate accepts it", () => {
    // As Stalwart parses the REQUEST (the occurrences in the series).
    const parsed = {
      ...weekly(),
      participants: {
        ...weekly().participants,
        pbob: {
          ...weekly().participants?.pbob,
          delegatedTo: { [`mailto:${DANA}`]: true },
          participationStatus: "delegated",
        },
        x: {
          calendarAddress: `mailto:${DANA}`,
          delegatedFrom: { [`mailto:${BOB}`]: true },
          participationStatus: "needs-action",
        },
      },
    };
    const copy = taskToImport(parsed, "x", "accepted");
    expect(copy).toMatchObject({
      due: "2026-11-06T09:00:00",
      recurrenceRule: { byDay: [{ day: "fr" }], frequency: "weekly" },
      start: "2026-11-06T09:00:00",
      timeZone: "Europe/Madrid",
    });
    // The occurrences, without the sender's alerts or people.
    expect(copy.recurrenceOverrides).toStrictEqual({
      "2026-11-13T09:00:00": {
        progress: "completed",
        start: "2026-11-13T09:00:00",
      },
      "2026-11-20T09:00:00": { excluded: true },
      "2026-11-27T09:00:00": {
        due: "2026-11-28T12:00:00",
        start: "2026-11-28T12:00:00",
        title: "Informe (sábado)",
      },
      // Nothing of it kept, but the occurrence stays (it may be an added
      // one, RFC 8984 §4.3.5).
      "2026-12-04T09:00:00": {},
    });
    expect(copy).not.toHaveProperty("alerts");
    // A series with only a due gets it as its start.
    expect(
      taskToImport({ ...parsed, start: null }, "x", "accepted")
    ).toMatchObject({ start: "2026-11-06T09:00:00" });
  });
});
