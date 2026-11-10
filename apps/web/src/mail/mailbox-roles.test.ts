// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// @vitest-environment jsdom

// Drafts and Sent looked up once per client and account, forgotten when
// mailboxes change, retried once when a save names a stale mailbox.

import {
  EmailId,
  EmailSubmissionId,
  IdentityId,
} from "@nebulamail/jmap-client";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { methodError, scriptedJmap } from "../test/scripted-jmap";
import type { Handler } from "../test/scripted-jmap";
import type { Draft } from "./compose";
import { saveDraft, undoSend } from "./compose";
import {
  forgetMailboxRoles,
  mailboxRoles,
  mayBeStaleMailbox,
} from "./mailbox-roles";

const jmap = vi.hoisted(() => ({
  client: null as null | Promise<unknown>,
}));

vi.mock(import("./client"), async () => {
  const { JmapClient: Client } = await import("@nebulamail/jmap-client");
  return {
    getClient: async () => {
      const client = await jmap.client;
      if (!(client instanceof Client)) {
        throw new TypeError("no scripted client");
      }
      return client;
    },
    resetClient: vi.fn<() => void>(),
  };
});

const addr = (local: string): string => `${local}@example.com`;

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
  subject: "Hello",
  to: [{ email: addr("bob") }],
};

const setBase = { accountId: "a", newState: "2", oldState: "1" };

let draftsId = "drafts";

const mailboxes: Handler = (_args, callId) => [
  [
    "Mailbox/get",
    {
      accountId: "a",
      list: [
        { id: draftsId, role: "drafts" },
        { id: "sent", role: "sent" },
      ],
      notFound: [],
      state: "1",
    },
    callId,
  ],
];

/** Creates the draft only in the current Drafts. */
const emailSet: Handler = (args, callId) => {
  if (args.create === undefined) {
    return [["Email/set", { ...setBase, updated: { m1: null } }, callId]];
  }
  // The draft's mailboxIds, as JSON text: is the current Drafts in it?
  const ok = JSON.stringify(args.create).includes(`"${draftsId}":true`);
  return [
    [
      "Email/set",
      ok
        ? {
            ...setBase,
            created: {
              draft: { blobId: "b", id: "m-new", size: 1, threadId: "t" },
            },
          }
        : {
            ...setBase,
            notCreated: {
              draft: { properties: ["mailboxIds"], type: "invalidProperties" },
            },
          },
      callId,
    ],
  ];
};

let scripted = scriptedJmap({ handlers: {} });

const asked = (name: string): number =>
  scripted.calls.filter(([method]) => method === name).length;

describe("mailbox roles", () => {
  beforeEach(() => {
    forgetMailboxRoles();
    draftsId = "drafts";
    scripted = scriptedJmap({
      handlers: { "Email/set": emailSet, "Mailbox/get": mailboxes },
    });
    jmap.client = scripted.connect();
  });

  it("looks up Drafts and Sent once for every save", async () => {
    await saveDraft(draft);
    await saveDraft(draft);

    expect(asked("Mailbox/get")).toBe(1);
    expect(asked("Email/set")).toBe(2);
  });

  it("looks again after the mailboxes changed", async () => {
    await saveDraft(draft);
    forgetMailboxRoles();
    await saveDraft(draft);

    expect(asked("Mailbox/get")).toBe(2);
  });

  it("retries once when Drafts was replaced", async () => {
    await saveDraft(draft);
    draftsId = "drafts-2";

    await expect(saveDraft(draft)).resolves.toBe("m-new");
    expect(asked("Mailbox/get")).toBe(2);
    expect(asked("Email/set")).toBe(3);
  });

  it("doesn't retry when the mailboxes are the same", async () => {
    scripted.script = {
      handlers: {
        "Email/set": (_args, callId) => [
          [
            "Email/set",
            {
              ...setBase,
              notCreated: { draft: { type: "invalidProperties" } },
            },
            callId,
          ],
        ],
        "Mailbox/get": mailboxes,
      },
    };

    await expect(saveDraft(draft)).rejects.toThrow("invalidProperties");
    expect(asked("Email/set")).toBe(1);
  });

  it("doesn't remember a failed lookup", async () => {
    const client = await scripted.connect();
    scripted.script = {
      handlers: {
        "Mailbox/get": (_args, callId) => [methodError("serverFail", callId)],
      },
    };

    await expect(
      mailboxRoles(client, client.primaryAccountId())
    ).rejects.toThrow("serverFail");

    scripted.script = { handlers: { "Mailbox/get": mailboxes } };

    await expect(
      mailboxRoles(client, client.primaryAccountId())
    ).resolves.toStrictEqual({ drafts: "drafts", sent: "sent" });
  });

  it("undo looks the mailboxes up while cancelling, then moves it", async () => {
    scripted.script = {
      handlers: {
        "EmailSubmission/set": (_args, callId) => [
          [
            "EmailSubmission/set",
            { ...setBase, updated: { s1: null } },
            callId,
          ],
        ],
        "Email/set": emailSet,
        "Mailbox/get": mailboxes,
      },
    };

    await undoSend({
      emailId: EmailId.parse("m1"),
      submissionId: EmailSubmissionId.parse("s1"),
    });

    const names = scripted.calls.map(([name]) => name);
    expect(names.indexOf("Email/set")).toBeGreaterThan(
      names.indexOf("EmailSubmission/set")
    );
    expect(names).toContain("Mailbox/get");
  });

  it("only counts invalid mailbox ids as stale", () => {
    expect(mayBeStaleMailbox({ type: "invalidProperties" })).toBeTruthy();
    expect(
      mayBeStaleMailbox({
        properties: ["mailboxIds"],
        type: "invalidProperties",
      })
    ).toBeTruthy();
    expect(
      mayBeStaleMailbox({ properties: ["subject"], type: "invalidProperties" })
    ).toBeFalsy();
    expect(mayBeStaleMailbox({ type: "tooLarge" })).toBeFalsy();
    expect(mayBeStaleMailbox(undefined)).toBeFalsy();
  });
});
