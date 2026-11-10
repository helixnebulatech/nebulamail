// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { isMethodError } from "@nebulamail/jmap-client";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { methodError, scriptedJmap } from "../../test/scripted-jmap";
import type { Handler } from "../../test/scripted-jmap";
import { previewRule } from "./api";
import type { Rule } from "./model";

const jmap = vi.hoisted(() => ({
  connect: null as null | (() => Promise<unknown>),
}));

vi.mock(import("../../mail/client"), async () => {
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

const rule = (value: string, op: "contains" | "regex" = "contains"): Rule => ({
  actions: [{ kind: "markRead" }],
  conditions: [
    {
      field: "subject",
      header: "",
      kind: "match",
      negate: false,
      op,
      value,
    },
  ],
  enabled: true,
  id: "r1",
  match: "all",
  name: "",
});

const found: Handler = (_args, callId) => [
  [
    "Email/query",
    {
      accountId: "a",
      canCalculateChanges: false,
      ids: ["m1"],
      position: 0,
      queryState: "q",
      total: 1,
    },
    callId,
  ],
];

const got: Handler = (_args, callId) => [
  [
    "Email/get",
    {
      accountId: "a",
      list: [
        {
          from: [{ email: "news", name: "News" }],
          id: "m1",
          receivedAt: "2026-11-01T10:00:00Z",
          subject: "Hello",
        },
      ],
      notFound: [],
      state: "s",
    },
    callId,
  ],
];

const brokenReference: Handler = (_args, callId) => [
  methodError("invalidResultReference", callId),
];

let scripted = scriptedJmap({ handlers: {} });

describe(previewRule, () => {
  beforeEach(() => {
    scripted = scriptedJmap({ handlers: {} });
    jmap.connect = scripted.connect;
  });

  it("lists what the server found", async () => {
    scripted.script = { handlers: { "Email/get": got, "Email/query": found } };

    await expect(previewRule(rule("hello"))).resolves.toStrictEqual({
      kind: "done",
      result: {
        approximate: false,
        emails: [
          {
            from: "News",
            id: "m1",
            receivedAt: "2026-11-01T10:00:00Z",
            subject: "Hello",
          },
        ],
        total: 1,
      },
    });
  });

  it("says «no preview» when the server's search refuses the filter", async () => {
    scripted.script = {
      handlers: {
        "Email/get": brokenReference,
        "Email/query": (_args, callId) => [
          methodError("unsupportedFilter", callId),
        ],
      },
    };

    await expect(previewRule(rule("x".repeat(1100)))).resolves.toStrictEqual({
      kind: "tooComplex",
    });
  });

  it("still fails on any other error", async () => {
    scripted.script = {
      handlers: {
        "Email/get": brokenReference,
        "Email/query": (_args, callId) => [methodError("serverFail", callId)],
      },
    };

    const error: unknown = await previewRule(rule("hello")).catch(
      (error_: unknown) => error_
    );

    expect(isMethodError(error, "serverFail")).toBeTruthy();
  });

  it("doesn't ask the server for a rule it can't search", async () => {
    await expect(previewRule(rule("^a.*$", "regex"))).resolves.toStrictEqual({
      kind: "unsearchable",
    });
    expect(scripted.calls).toStrictEqual([]);
  });
});
