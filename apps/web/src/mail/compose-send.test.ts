// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// @vitest-environment jsdom

// sendEmail's outcomes against a scripted server: sent, sent but not
// filed, unknown (may have gone out) and failed (surely didn't).

import { EmailId, IdentityId } from "@nebulamail/jmap-client";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { methodError, scriptedJmap } from "../test/scripted-jmap";
import type { Handler, Invocation, Script } from "../test/scripted-jmap";
import type { Draft } from "./compose";
import { sendEmail } from "./compose";
import { serverRefusalOf } from "./refusal";
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

/** Builds an address (keeps the test free of address literals). */
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

const submitted =
  (implicit: readonly Invocation[] | "filed"): Handler =>
  (_args, callId) => [
    [
      "EmailSubmission/set",
      {
        ...setBase,
        created: { send: { id: "s1", sendAt: "2026-11-02T10:00:00Z" } },
      },
      callId,
    ],
    ...(implicit === "filed"
      ? [
          [
            "Email/set",
            { ...setBase, updated: { [DRAFT_ID]: null } },
            callId,
          ] as const,
        ]
      : implicit.map(([name, args]): Invocation => [name, args, callId])),
  ];

const notSent: Handler = (_args, callId) => [
  [
    "EmailSubmission/set",
    {
      ...setBase,
      notCreated: {
        send: {
          description: "may have been sent; check before sending again",
          type: "forbiddenToSend",
        },
      },
    },
    callId,
  ],
];

const submissionRow = (emailId: string, id = "s9") => ({
  deliveryStatus: null,
  emailId,
  id,
  sendAt: "2026-11-02T10:00:00Z",
  undoStatus: "final",
});

/** EmailSubmission/query + /get that find these rows. */
const lookup = (
  rows: readonly ReturnType<typeof submissionRow>[]
): Record<string, Handler> => ({
  "EmailSubmission/get": (_args, callId) => [
    [
      "EmailSubmission/get",
      { accountId: "a", list: rows, notFound: [], state: "1" },
      callId,
    ],
  ],
  "EmailSubmission/query": (_args, callId) => [
    [
      "EmailSubmission/query",
      {
        accountId: "a",
        canCalculateChanges: false,
        ids: rows.map((row) => row.id),
        position: 0,
        queryState: "1",
      },
      callId,
    ],
  ],
});

/** An Email/set answer that doesn't match RFC 8621. */
const broken: Handler = (_args, callId) => [
  ["Email/set", { created: "nope" }, callId],
];

let scripted = scriptedJmap({ handlers: {} });

const serve = (handlers: Script["handlers"], respond?: Script["respond"]) => {
  scripted.script = {
    handlers: { "Mailbox/get": mailboxes, ...handlers },
    ...(respond === undefined ? {} : { respond }),
  };
};

const failure = async (): Promise<unknown> =>
  await sendEmail(draft).then(
    () => null,
    (error: unknown) => error
  );

const asked = (name: string): number =>
  scripted.calls.filter(([method]) => method === name).length;

describe(sendEmail, () => {
  beforeEach(() => {
    scripted = scriptedJmap({ handlers: {} });
    jmap.connect = scripted.connect;
  });

  it("is sent and filed, with the id of the draft it created", async () => {
    serve({
      "EmailSubmission/set": submitted("filed"),
      "Email/set": draftCreated,
    });

    await expect(sendEmail(draft)).resolves.toStrictEqual({
      kind: "sent",
      sent: {
        emailId: DRAFT_ID,
        sendAt: "2026-11-02T10:00:00Z",
        submissionId: "s1",
        undoable: false,
      },
    });
  });

  it("is sent but not filed when the implicit Email/set failed or is missing", async () => {
    const outcomes = [];
    for (const implicit of [
      [
        [
          "Email/set",
          { ...setBase, notUpdated: { [DRAFT_ID]: { type: "serverFail" } } },
          "",
        ],
      ],
      [methodError("serverFail", "")],
      [["Email/set", { updated: "nope" }, ""]],
      [],
    ] satisfies Invocation[][]) {
      serve({
        "EmailSubmission/set": submitted(implicit),
        "Email/set": draftCreated,
      });
      // oxlint-disable-next-line no-await-in-loop -- one script at a time.
      outcomes.push(await sendEmail(draft));
    }

    expect(outcomes.map((outcome) => outcome.kind)).toStrictEqual([
      "sentNotFiled",
      "sentNotFiled",
      "sentNotFiled",
      "sentNotFiled",
    ]);
    expect(outcomes[0]).toMatchObject({ sent: { emailId: DRAFT_ID } });
  });

  it("is unknown when the server didn't confirm but kept the submission", async () => {
    serve({
      "EmailSubmission/set": notSent,
      "Email/set": draftCreated,
      ...lookup([submissionRow(DRAFT_ID)]),
    });

    await expect(sendEmail(draft)).resolves.toStrictEqual({
      emailId: DRAFT_ID,
      kind: "unknown",
    });
  });

  it("fails, keeping the new draft, when there is no submission of it", async () => {
    serve({
      "EmailSubmission/set": notSent,
      "Email/set": draftCreated,
      ...lookup([]),
    });

    const error = await failure();

    expect(error).toBeInstanceOf(SendFailedError);
    expect(error).toMatchObject({ draftId: DRAFT_ID });
  });

  it("looks through all submissions without /query, counting only this email's", async () => {
    const getAll = (rows: readonly ReturnType<typeof submissionRow>[]) => ({
      "EmailSubmission/get": lookup(rows)["EmailSubmission/get"] ?? (() => []),
      "EmailSubmission/query": ((_args, callId) => [
        methodError("unknownMethod", callId),
      ]) satisfies Handler,
    });
    serve({
      "EmailSubmission/set": notSent,
      "Email/set": draftCreated,
      ...getAll([submissionRow("m-older", "s1"), submissionRow(DRAFT_ID)]),
    });

    await expect(sendEmail(draft)).resolves.toStrictEqual({
      emailId: DRAFT_ID,
      kind: "unknown",
    });

    // Another email's send doesn't turn a sure failure into "unknown".
    serve({
      "EmailSubmission/set": notSent,
      "Email/set": draftCreated,
      ...getAll([submissionRow("m-older", "s1")]),
    });

    await expect(failure()).resolves.toBeInstanceOf(SendFailedError);
  });

  it("is unknown when the check itself fails", async () => {
    serve({
      "EmailSubmission/query": (_args, callId) => [
        methodError("serverFail", callId),
      ],
      "EmailSubmission/set": notSent,
      "Email/set": draftCreated,
    });

    await expect(sendEmail(draft)).resolves.toStrictEqual({
      emailId: DRAFT_ID,
      kind: "unknown",
    });
  });

  it("checks after a submission error that may hide a send", async () => {
    for (const answer of [
      ((_args, callId) => [
        methodError("serverFail", callId),
      ]) satisfies Handler,
      // No answer for the call: the request's other answers came back.
      (() => []) satisfies Handler,
    ]) {
      serve({
        "EmailSubmission/set": answer,
        "Email/set": draftCreated,
        ...lookup([submissionRow(DRAFT_ID)]),
      });
      // oxlint-disable-next-line no-await-in-loop -- one script at a time.
      await expect(sendEmail(draft)).resolves.toStrictEqual({
        emailId: DRAFT_ID,
        kind: "unknown",
      });
    }
  });

  it("fails without checking when the submission error says nothing was done", async () => {
    serve({
      "EmailSubmission/set": (_args, callId) => [
        methodError("invalidArguments", callId),
      ],
      "Email/set": draftCreated,
      ...lookup([submissionRow(DRAFT_ID)]),
    });

    const error = await failure();

    expect(error).toBeInstanceOf(SendFailedError);
    expect(error).toMatchObject({ draftId: DRAFT_ID });
    expect(asked("EmailSubmission/query")).toBe(0);
  });

  it("fails with nothing saved when the draft is refused", async () => {
    serve({
      "EmailSubmission/set": (_args, callId) => [
        methodError("invalidResultReference", callId),
      ],
      "Email/set": (_args, callId) => [
        [
          "Email/set",
          { ...setBase, notCreated: { draft: { type: "tooLarge" } } },
          callId,
        ],
      ],
    });

    const error = await failure();

    expect(error).toBeInstanceOf(SendFailedError);
    expect(error).toMatchObject({ draftId: null });
    expect(serverRefusalOf(error)).toBe("tooLarge");
  });

  it("fails when Email/set answers a method error", async () => {
    serve({
      "EmailSubmission/set": (_args, callId) => [
        methodError("invalidResultReference", callId),
      ],
      "Email/set": (_args, callId) => [methodError("serverFail", callId)],
    });

    await expect(failure()).resolves.toBeInstanceOf(SendFailedError);
  });

  it("is unknown when Email/set's answer is broken (the draft may exist)", async () => {
    serve({ "EmailSubmission/set": submitted("filed"), "Email/set": broken });

    await expect(sendEmail(draft)).resolves.toStrictEqual({
      emailId: null,
      kind: "unknown",
    });

    serve({ "EmailSubmission/set": notSent, "Email/set": broken });

    await expect(sendEmail(draft)).resolves.toStrictEqual({
      emailId: null,
      kind: "unknown",
    });
  });

  it("is unknown when the answer is lost after the request went out", async () => {
    const lost = [
      async () => await Promise.reject(new TypeError("Failed to fetch")),
      async () =>
        await Promise.resolve(new Response("Bad gateway", { status: 502 })),
      async () =>
        await Promise.resolve(new Response("<html>", { status: 200 })),
    ];
    const outcomes = [];
    for (const respond of lost) {
      serve({}, async (calls) =>
        calls.some(([name]) => name === "Mailbox/get")
          ? Response.json({
              methodResponses: calls.flatMap(([, args, callId]) =>
                mailboxes(args, callId)
              ),
              sessionState: "s1",
            })
          : await respond()
      );
      // oxlint-disable-next-line no-await-in-loop -- one script at a time.
      outcomes.push(await sendEmail(draft));
    }

    expect(outcomes).toStrictEqual([
      { emailId: null, kind: "unknown" },
      { emailId: null, kind: "unknown" },
      { emailId: null, kind: "unknown" },
    ]);
  });

  it("fails when the server refused the whole request (4xx)", async () => {
    serve(
      {},
      async (calls) =>
        await Promise.resolve(
          calls.some(([name]) => name === "Mailbox/get")
            ? Response.json({
                methodResponses: calls.flatMap(([, args, callId]) =>
                  mailboxes(args, callId)
                ),
                sessionState: "s1",
              })
            : Response.json(
                {
                  detail: "Too big",
                  limit: "maxSizeRequest",
                  status: 400,
                  type: "urn:ietf:params:jmap:error:limit",
                },
                {
                  headers: { "Content-Type": "application/problem+json" },
                  status: 400,
                }
              )
        )
    );

    const error = await failure();

    expect(error).toBeInstanceOf(SendFailedError);
    expect(error).toMatchObject({ draftId: null });
  });
});
