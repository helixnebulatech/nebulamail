// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// @vitest-environment jsdom

// Scheduled send against a scripted server: HOLDUNTIL in the envelope,
// the server's limit checked first and learned from a refusal, the list
// of scheduled messages, cancelling, and changing the time (cancel first,
// then submit again; back to Drafts if the new time is refused).

import {
  EmailId,
  EmailSubmissionId,
  IdentityId,
} from "@nebulamail/jmap-client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { scriptedJmap } from "../test/scripted-jmap";
import type { Handler, Script } from "../test/scripted-jmap";
import type { Draft } from "./compose";
import { rescheduleEmail, sendEmail, undoSend } from "./compose";
import {
  ScheduleError,
  forgetLearned,
  holdUntil,
  limitFor,
  listScheduled,
} from "./schedule";
import type { ScheduledEmail } from "./schedule";
import { SendFailedError } from "./send-failed";

const jmap = vi.hoisted(() => ({
  connect: null as null | (() => Promise<unknown>),
}));

vi.mock(import("./client"), async () => {
  const { JmapClient } = await import("@nebulamail/jmap-client");
  return {
    getClient: async () => {
      const client = await jmap.connect?.();
      if (!(client instanceof JmapClient)) {
        throw new TypeError("no scripted client");
      }
      return client;
    },
    resetClient: vi.fn<() => void>(),
  };
});

const addr = (local: string): string => `${local}@example.com`;
const HOUR = 3_600_000;
const DAY = 24 * HOUR;

const draft: Draft = {
  attachments: [],
  bcc: [{ email: addr("carol") }],
  cc: [],
  html: "<p>Hi</p>",
  identity: {
    email: addr("alice"),
    id: IdentityId.parse("i1"),
    name: "Alice",
    signature: null,
  },
  subject: "Later",
  to: [{ email: addr("bob") }],
};

const DRAFT_ID = EmailId.parse("m-new");
const setBase = { accountId: "a", newState: "2", oldState: "1" };

const mailboxes: Handler = (_args, callId) => [
  [
    "Mailbox/get",
    {
      accountId: "a",
      list: [
        { id: "drafts", role: "drafts" },
        { id: "sent", role: "sent" },
      ],
      notFound: [],
      state: "1",
    },
    callId,
  ],
];

const draftCreated: Handler = (_args, callId) => [
  [
    "Email/set",
    {
      ...setBase,
      created: {
        draft: { blobId: "b1", id: DRAFT_ID, size: 10, threadId: "t1" },
      },
    },
    callId,
  ],
];

const emailUpdated: Handler = (args, callId) => [
  [
    "Email/set",
    {
      ...setBase,
      updated: Object.fromEntries(
        Object.keys(args.update ?? {}).map((id) => [id, null])
      ),
    },
    callId,
  ],
];

let scripted = scriptedJmap({ handlers: {} }, { futureRelease: true });

const serve = (handlers: Script["handlers"]) => {
  scripted.script = { handlers: { "Mailbox/get": mailboxes, ...handlers } };
};

const calls = (name: string) =>
  scripted.calls.filter(([method]) => method === name);

const scheduled: ScheduledEmail = {
  emailId: EmailId.parse("m1"),
  holdUntil: true,
  identityId: IdentityId.parse("i1"),
  mailFrom: addr("alice"),
  recipients: [addr("bob"), addr("carol")],
  sendAt: "2026-11-03T07:00:00Z",
  sendWhenSignedOut: false,
  subject: "Later",
  submissionId: EmailSubmissionId.parse("s1"),
};

const notCancelled = (undoStatus: string): Script["handlers"] => ({
  "EmailSubmission/get": (_args, callId) => [
    [
      "EmailSubmission/get",
      {
        accountId: "a",
        list: [{ id: "s1", undoStatus }],
        notFound: [],
        state: "1",
      },
      callId,
    ],
  ],
  "EmailSubmission/set": (_args, callId) => [
    [
      "EmailSubmission/set",
      { ...setBase, notUpdated: { s1: { type: "cannotUnsend" } } },
      callId,
    ],
  ],
});

const interrupted = (pending: readonly string[]): Script["handlers"] => ({
  "Email/set": emailUpdated,
  "EmailSubmission/query": (_args, callId) => [
    [
      "EmailSubmission/query",
      {
        accountId: "a",
        canCalculateChanges: false,
        ids: pending,
        position: 0,
        queryState: "1",
      },
      callId,
    ],
  ],
  "EmailSubmission/set": (args, callId) => [
    args.update === undefined
      ? ["error", { type: "serverFail" }, callId]
      : ["EmailSubmission/set", { ...setBase, updated: { s1: null } }, callId],
  ],
});

describe("scheduled send", () => {
  beforeEach(() => {
    scripted = scriptedJmap({ handlers: {} }, { futureRelease: true });
    jmap.connect = scripted.connect;
  });

  afterEach(() => {
    forgetLearned();
  });

  it("submits with HOLDUNTIL in UTC, Bcc in the envelope, and leaves Drafts", async () => {
    const at = new Date(Date.now() + 2 * HOUR);
    serve({
      "Email/set": draftCreated,
      "EmailSubmission/set": (_args, callId) => [
        [
          "EmailSubmission/set",
          {
            ...setBase,
            created: {
              send: { id: "s1", sendAt: holdUntil(at), undoStatus: "pending" },
            },
          },
          callId,
        ],
        ["Email/set", { ...setBase, updated: { [DRAFT_ID]: null } }, callId],
      ],
    });

    await expect(sendEmail(draft, { sendAt: at })).resolves.toStrictEqual({
      kind: "sent",
      sent: {
        emailId: DRAFT_ID,
        scheduled: true,
        sendAt: holdUntil(at),
        submissionId: "s1",
        undoable: true,
      },
    });
    const [submit] = calls("EmailSubmission/set");

    expect(submit?.[1]).toMatchObject({
      create: {
        send: {
          envelope: {
            mailFrom: {
              email: addr("alice"),
              parameters: { HOLDUNTIL: holdUntil(at) },
            },
            rcptTo: [{ email: addr("bob") }, { email: addr("carol") }],
          },
        },
      },
      onSuccessUpdateEmail: {
        "#send": {
          "keywords/$draft": null,
          "mailboxIds/drafts": null,
          "mailboxIds/sent": true,
        },
      },
    });
  });

  it("refuses a time past the server's limit before sending anything", async () => {
    serve({ "Email/set": draftCreated });

    await expect(
      sendEmail(draft, { sendAt: new Date(Date.now() + 31 * DAY) })
    ).rejects.toBeInstanceOf(ScheduleError);
    await expect(
      sendEmail(draft, { sendAt: new Date(Date.now() - HOUR) })
    ).rejects.toMatchObject({ failure: { kind: "time", problem: "past" } });
    expect(calls("Email/set")).toHaveLength(0);
  });

  it("refuses to schedule when the server can't hold messages", async () => {
    scripted = scriptedJmap({ handlers: {} });
    jmap.connect = scripted.connect;
    serve({ "Email/set": draftCreated });

    await expect(
      sendEmail(draft, { sendAt: new Date(Date.now() + HOUR) })
    ).rejects.toMatchObject({ failure: { problem: "invalid" } });
  });

  it("learns the real maximum when the server refuses the time (Stalwart's 7 days)", async () => {
    const maximum = new Date(Date.now() + 7 * DAY);
    maximum.setMilliseconds(0);
    serve({
      "Email/set": draftCreated,
      "EmailSubmission/get": (_args, callId) => [
        [
          "EmailSubmission/get",
          { accountId: "a", list: [], notFound: [], state: "1" },
          callId,
        ],
      ],
      "EmailSubmission/query": (_args, callId) => [
        [
          "EmailSubmission/query",
          {
            accountId: "a",
            canCalculateChanges: false,
            ids: [],
            position: 0,
            queryState: "1",
          },
          callId,
        ],
      ],
      "EmailSubmission/set": (_args, callId) => [
        [
          "EmailSubmission/set",
          {
            ...setBase,
            notCreated: {
              send: {
                description: `Server rejected MAIL-FROM: 501 5.5.4 Requested release time exceeds maximum of ${holdUntil(maximum)}.`,
                type: "forbiddenMailFrom",
              },
            },
          },
          callId,
        ],
      ],
    });

    const error = await sendEmail(draft, {
      sendAt: new Date(Date.now() + 10 * DAY),
    }).catch((error_: unknown) => error_);

    expect(error).toBeInstanceOf(SendFailedError);
    expect(error).toMatchObject({ draftId: DRAFT_ID });
    const client = await scripted.connect();

    const learned = limitFor(client);

    // Kept as "7 days from now", not as that date (see learnMaximum).
    expect(learned?.latest).toBeNull();
    expect(learned?.maxSeconds).toBeGreaterThan(7 * 86_400 - 5);
    expect(learned?.maxSeconds).toBeLessThanOrEqual(7 * 86_400);
  });

  it("lists pending submissions with their subject, soonest first", async () => {
    serve({
      "Email/get": (_args, callId) => [
        [
          "Email/get",
          {
            accountId: "a",
            list: [
              { id: "m1", subject: "Later" },
              { id: "m2", subject: "Sooner" },
            ],
            notFound: [],
            state: "1",
          },
          callId,
        ],
      ],
      "EmailSubmission/get": (_args, callId) => [
        [
          "EmailSubmission/get",
          {
            accountId: "a",
            list: [
              {
                emailId: "m1",
                envelope: {
                  mailFrom: {
                    email: addr("alice"),
                    parameters: { HOLDUNTIL: "2026-11-03T07:00:00Z" },
                  },
                  rcptTo: [{ email: addr("bob"), parameters: null }],
                },
                id: "s1",
                identityId: "i1",
                sendAt: "2026-11-03T07:00:00Z",
                undoStatus: "pending",
              },
              {
                emailId: "m2",
                envelope: null,
                id: "s2",
                identityId: "i1",
                sendAt: "2026-11-02T20:00:00Z",
                undoStatus: "pending",
              },
            ],
            notFound: [],
            state: "1",
          },
          callId,
        ],
      ],
      "EmailSubmission/query": (_args, callId) => [
        [
          "EmailSubmission/query",
          {
            accountId: "a",
            canCalculateChanges: false,
            ids: ["s1", "s2"],
            position: 0,
            queryState: "1",
          },
          callId,
        ],
      ],
    });

    const list = await listScheduled();

    expect(list.map((item) => [item.subject, item.mailFrom])).toStrictEqual([
      ["Sooner", null],
      ["Later", addr("alice")],
    ]);
    expect(list[1]?.recipients).toStrictEqual([addr("bob")]);
    expect(calls("EmailSubmission/query")[0]?.[1]).toMatchObject({
      filter: { undoStatus: "pending" },
      sort: [{ isAscending: true, property: "sentAt" }],
    });
  });

  it("cancels and puts the email back in Drafts", async () => {
    serve({
      "Email/set": emailUpdated,
      "EmailSubmission/set": (_args, callId) => [
        ["EmailSubmission/set", { ...setBase, updated: { s1: null } }, callId],
      ],
    });

    await undoSend(scheduled);

    expect(calls("Email/set")[0]?.[1]).toMatchObject({
      update: {
        m1: {
          "keywords/$draft": true,
          "mailboxIds/drafts": true,
          "mailboxIds/sent": null,
        },
      },
    });
  });

  it("changes the time: cancels first, then submits the same email again", async () => {
    const at = new Date(Date.now() + 3 * DAY);
    serve({
      "EmailSubmission/set": (args, callId) => [
        [
          "EmailSubmission/set",
          args.update === undefined
            ? {
                ...setBase,
                created: {
                  again: {
                    id: "s2",
                    sendAt: holdUntil(at),
                    undoStatus: "pending",
                  },
                },
              }
            : { ...setBase, updated: { s1: null } },
          callId,
        ],
      ],
    });

    await rescheduleEmail(scheduled, at);
    const [cancel, again] = calls("EmailSubmission/set");

    expect(cancel?.[1]).toMatchObject({
      update: { s1: { undoStatus: "canceled" } },
    });
    expect(again?.[1]).toMatchObject({
      create: {
        again: {
          emailId: "m1",
          envelope: {
            mailFrom: {
              email: addr("alice"),
              parameters: { HOLDUNTIL: holdUntil(at) },
            },
            rcptTo: [{ email: addr("bob") }, { email: addr("carol") }],
          },
          identityId: "i1",
        },
      },
    });
  });

  it("never submits again when the cancel came too late", async () => {
    serve(notCancelled("final"));

    await expect(
      rescheduleEmail(scheduled, new Date(Date.now() + DAY))
    ).rejects.toMatchObject({ failure: { kind: "alreadySent" } });
    expect(calls("EmailSubmission/set")).toHaveLength(1);
  });

  it("says so when it was already cancelled (another tab), not «too late»", async () => {
    serve(notCancelled("canceled"));

    await expect(undoSend(scheduled)).rejects.toMatchObject({
      failure: { kind: "alreadyCanceled" },
    });
    expect(calls("Email/set")).toHaveLength(0);
  });

  it("puts the email back in Drafts when the new submission fails after the cancel", async () => {
    serve(interrupted([]));

    await expect(
      rescheduleEmail(scheduled, new Date(Date.now() + DAY))
    ).rejects.toMatchObject({ failure: { kind: "interrupted" } });
    expect(calls("EmailSubmission/query")[0]?.[1]).toMatchObject({
      filter: { emailIds: ["m1"], undoStatus: "pending" },
    });
    expect(calls("Email/set")[0]?.[1]).toMatchObject({
      update: { m1: { "mailboxIds/drafts": true } },
    });
  });

  it("leaves it scheduled when the lost answer had made it after all", async () => {
    serve(interrupted(["s2"]));

    await expect(
      rescheduleEmail(scheduled, new Date(Date.now() + DAY))
    ).rejects.toMatchObject({ failure: { kind: "interrupted" } });
    expect(calls("Email/set")).toHaveLength(0);
  });

  it("puts the email back in Drafts when the new time is refused", async () => {
    serve({
      "Email/set": emailUpdated,
      "EmailSubmission/set": (args, callId) => [
        [
          "EmailSubmission/set",
          args.update === undefined
            ? {
                ...setBase,
                notCreated: {
                  again: {
                    description: "Server rejected MAIL-FROM",
                    type: "forbiddenMailFrom",
                  },
                },
              }
            : { ...setBase, updated: { s1: null } },
          callId,
        ],
      ],
    });

    await expect(
      rescheduleEmail(scheduled, new Date(Date.now() + DAY))
    ).rejects.toMatchObject({ failure: { kind: "refused", maximum: null } });
    expect(calls("Email/set")[0]?.[1]).toMatchObject({
      update: { m1: { "mailboxIds/drafts": true } },
    });
  });

  it("checks the new time before cancelling anything", async () => {
    serve({});

    await expect(
      rescheduleEmail(scheduled, new Date(Date.now() + 40 * DAY))
    ).rejects.toMatchObject({ failure: { problem: "tooFar" } });
    expect(calls("EmailSubmission/set")).toHaveLength(0);
  });
});
