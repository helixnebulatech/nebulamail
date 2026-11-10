// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// @vitest-environment jsdom

// Scheduled mail sent after sign-out (send-signed-out.ts) against a
// scripted server: the vendor property on a scheduled send only on the
// bridge that announces it, with the device's setting; the «Programados»
// list reads it, a new time keeps it, and one message can change it.

import {
  CAPABILITIES,
  EmailId,
  EmailSubmissionId,
  IdentityId,
  SEND_WHEN_SIGNED_OUT,
  SEND_WHEN_SIGNED_OUT_PATCH,
} from "@nebulamail/jmap-client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { setLocalFlag } from "../lib/use-local-flag";
import { scriptedJmap } from "../test/scripted-jmap";
import type { Handler, Script } from "../test/scripted-jmap";
import type { Draft } from "./compose";
import { rescheduleEmail, sendEmail } from "./compose";
import { forgetLearned, holdUntil, listScheduled } from "./schedule";
import type { ScheduledEmail } from "./schedule";
import {
  SEND_SIGNED_OUT_FLAG,
  SignedOutChangeError,
  setSendWhenSignedOut,
  waitingForSignIn,
} from "./send-signed-out";

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
const setBase = { accountId: "a", newState: "2", oldState: "1" };
const DRAFT_ID = EmailId.parse("m-new");

const draft: Draft = {
  attachments: [],
  bcc: [],
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

const scheduled = (
  sendWhenSignedOut: boolean,
  holdUntilParam = true
): ScheduledEmail => ({
  emailId: EmailId.parse("m1"),
  holdUntil: holdUntilParam,
  identityId: IdentityId.parse("i1"),
  mailFrom: addr("alice"),
  recipients: [addr("bob")],
  sendAt: "2026-11-03T07:00:00Z",
  sendWhenSignedOut,
  subject: "Later",
  submissionId: EmailSubmissionId.parse("s1"),
});

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

const sending: Script["handlers"] = {
  "Email/set": (_args, callId) => [
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
  ],
  "EmailSubmission/set": (_args, callId) => [
    [
      "EmailSubmission/set",
      {
        ...setBase,
        created: {
          send: {
            id: "s1",
            sendAt: "2026-11-03T07:00:00Z",
            undoStatus: "pending",
          },
        },
      },
      callId,
    ],
    ["Email/set", { ...setBase, updated: { [DRAFT_ID]: null } }, callId],
  ],
  "Mailbox/get": mailboxes,
};

let scripted = scriptedJmap({ handlers: {} });

const connect = (bridge: boolean, handlers: Script["handlers"]) => {
  scripted = scriptedJmap(
    { handlers },
    { futureRelease: true, sendWhenSignedOut: bridge }
  );
  jmap.connect = scripted.connect;
};

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null;

/** One object of a recorded call's `create`. */
const createdIn = (
  args: Record<string, unknown> | undefined,
  key: string
): Record<string, unknown> => {
  const create = args?.create;
  const one = isRecord(create) ? create[key] : undefined;
  return isRecord(one) ? one : {};
};

/** The `send` create of the last EmailSubmission/set. */
const submissionCreate = () => {
  const submit = scripted.calls.findLast(
    ([method]) => method === "EmailSubmission/set"
  );
  return createdIn(submit?.[1], "send");
};

describe("scheduled mail sent after sign-out", () => {
  beforeEach(() => {
    globalThis.localStorage.clear();
  });

  afterEach(() => {
    forgetLearned();
  });

  it.each([true, false])(
    "a scheduled send carries the device's choice (%s) on the bridge",
    async (on) => {
      setLocalFlag(SEND_SIGNED_OUT_FLAG, on);
      connect(true, sending);
      await sendEmail(draft, { sendAt: new Date(Date.now() + 2 * HOUR) });
      expect(submissionCreate()[SEND_WHEN_SIGNED_OUT]).toBe(on);
      expect(scripted.usings.at(-1)).toContain(CAPABILITIES.bridge);
    }
  );

  it("is off by default", async () => {
    connect(true, sending);
    await sendEmail(draft, { sendAt: new Date(Date.now() + 2 * HOUR) });
    expect(submissionCreate()).toHaveProperty(SEND_WHEN_SIGNED_OUT, false);
  });

  it("is never sent to a server without it, nor for undo send", async () => {
    setLocalFlag(SEND_SIGNED_OUT_FLAG, true);
    connect(false, sending);
    await sendEmail(draft, { sendAt: new Date(Date.now() + 2 * HOUR) });
    expect(submissionCreate()).not.toHaveProperty(SEND_WHEN_SIGNED_OUT);
    expect(scripted.usings.at(-1)).not.toContain(CAPABILITIES.bridge);

    connect(true, sending);
    await sendEmail(draft);
    expect(submissionCreate()).not.toHaveProperty(SEND_WHEN_SIGNED_OUT);
  });

  it("the list reads it, only from the bridge", async () => {
    const handlers: Script["handlers"] = {
      "Email/get": (_args, callId) => [
        [
          "Email/get",
          {
            accountId: "a",
            list: [{ id: "m1", subject: "Later" }],
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
                envelope: null,
                id: "s1",
                identityId: "i1",
                sendAt: "2026-11-03T07:00:00Z",
                undoStatus: "pending",
                [SEND_WHEN_SIGNED_OUT]: true,
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
            ids: ["s1"],
            position: 0,
            queryState: "1",
          },
          callId,
        ],
      ],
    };
    connect(true, handlers);
    const list = await listScheduled();
    expect(list[0]?.sendWhenSignedOut).toBeTruthy();
    const get = scripted.calls.find(([m]) => m === "EmailSubmission/get");
    expect(get?.[1].properties).toContain(SEND_WHEN_SIGNED_OUT);

    connect(false, handlers);
    await listScheduled();
    const plain = scripted.calls.find(([m]) => m === "EmailSubmission/get");
    expect(plain?.[1].properties).not.toContain(SEND_WHEN_SIGNED_OUT);
  });

  it("a new time keeps it", async () => {
    const at = new Date(Date.now() + 3 * HOUR);
    connect(true, {
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
    await rescheduleEmail(scheduled(true), at);
    const [, again] = scripted.calls.filter(
      ([method]) => method === "EmailSubmission/set"
    );
    expect(createdIn(again?.[1], "again")).toHaveProperty(
      SEND_WHEN_SIGNED_OUT,
      true
    );
  });

  it("changes one message with the escaped patch key", async () => {
    connect(true, {
      "EmailSubmission/set": (_args, callId) => [
        ["EmailSubmission/set", { ...setBase, updated: { s1: null } }, callId],
      ],
    });
    await setSendWhenSignedOut(
      await scripted.connect(),
      EmailSubmissionId.parse("s1"),
      true
    );
    const [set] = scripted.calls;
    expect(set?.[1]).toMatchObject({
      update: { s1: { [SEND_WHEN_SIGNED_OUT_PATCH]: true } },
    });
    expect(scripted.usings[0]).toContain(CAPABILITIES.bridge);
  });

  it("says why the server would not change it", async () => {
    connect(true, {
      "EmailSubmission/set": (_args, callId) => [
        [
          "EmailSubmission/set",
          {
            ...setBase,
            notUpdated: {
              s1: {
                description: "the message is no longer waiting to be sent",
                properties: [SEND_WHEN_SIGNED_OUT],
                type: "invalidProperties",
              },
            },
          },
          callId,
        ],
      ],
    });
    await expect(
      setSendWhenSignedOut(
        await scripted.connect(),
        EmailSubmissionId.parse("s1"),
        false
      )
    ).rejects.toBeInstanceOf(SignedOutChangeError);
  });

  it("the logout warns only about scheduled messages that would wait", () => {
    const waiting = scheduled(false);
    expect(
      waitingForSignIn([waiting, scheduled(true), scheduled(false, false)])
    ).toStrictEqual([waiting]);
  });
});
