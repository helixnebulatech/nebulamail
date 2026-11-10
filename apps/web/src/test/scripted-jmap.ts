// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Unit tests only: a JmapClient on the browser tests' fake session whose
// API answers come from a script, one handler per method name. Each
// handler gets the call's arguments and callId and returns the
// invocations to send back (several for implicit calls, none to leave the
// call unanswered). Requests are recorded for assertions.

import { JmapClient } from "@nebulamail/jmap-client";

import { session } from "../../e2e/fake-jmap";

export type Invocation = readonly [string, Record<string, unknown>, string];

export type Handler = (
  args: Record<string, unknown>,
  callId: string
) => readonly Invocation[];

export interface Script {
  readonly handlers: Readonly<Record<string, Handler>>;
  /** Replaces the whole API answer (transport errors, HTTP errors). */
  readonly respond?: (calls: readonly Invocation[]) => Promise<Response>;
}

export interface Scripted {
  readonly connect: () => Promise<JmapClient>;
  /** Every method call sent to the API, in order. */
  readonly calls: Invocation[];
  /** The `using` of every API request, in order. */
  readonly usings: (readonly string[])[];
  script: Script;
}

const SESSION_URL = "http://jmap.test/.well-known/jmap";

const isInvocation = (value: unknown): value is Invocation =>
  Array.isArray(value) &&
  value.length === 3 &&
  typeof value[0] === "string" &&
  typeof value[2] === "string" &&
  typeof value[1] === "object" &&
  value[1] !== null;

const usingOf = (init: RequestInit | undefined): string[] => {
  const body: unknown =
    typeof init?.body === "string" ? JSON.parse(init.body) : null;
  if (typeof body !== "object" || body === null || !("using" in body)) {
    return [];
  }
  return Array.isArray(body.using)
    ? body.using.filter((item): item is string => typeof item === "string")
    : [];
};

const methodCallsOf = async (init: RequestInit | undefined) => {
  const body: unknown =
    typeof init?.body === "string" ? JSON.parse(init.body) : null;
  if (typeof body !== "object" || body === null || !("methodCalls" in body)) {
    return [];
  }
  const { methodCalls } = body;
  return await Promise.resolve(
    Array.isArray(methodCalls) ? methodCalls.filter(isInvocation) : []
  );
};

/** A method error answer, `["error", {type}, callId]`. */
export const methodError = (
  type: string,
  callId: string,
  extra: Record<string, unknown> = {}
): Invocation => ["error", { type, ...extra }, callId];

export const scriptedJmap = (
  script: Script,
  {
    futureRelease = false,
    maxObjectsInSet,
    sendWhenSignedOut = false,
  }: {
    /** The core capability's `maxObjectsInSet` (500 if not given). */
    readonly maxObjectsInSet?: number;
    /** The session can hold messages (undo send, scheduled send). */
    readonly futureRelease?: boolean;
    /**
     * The server is the bridge and sends scheduled mail after sign-out
     * (its capability says `sendWhenSignedOut`).
     */
    readonly sendWhenSignedOut?: boolean;
  } = {}
): Scripted => {
  const sessionJson = () => {
    const fake = session(false, false, false, futureRelease);
    const core = fake.capabilities["urn:ietf:params:jmap:core"];
    const base =
      maxObjectsInSet === undefined
        ? fake
        : {
            ...fake,
            capabilities: {
              ...fake.capabilities,
              "urn:ietf:params:jmap:core": { ...core, maxObjectsInSet },
            },
          };
    return sendWhenSignedOut
      ? {
          ...base,
          capabilities: {
            ...base.capabilities,
            "https://helixnebula.tech/jmap/bridge": { sendWhenSignedOut: true },
          },
        }
      : base;
  };
  const scripted: Scripted = {
    calls: [],
    usings: [],
    connect: async () =>
      await JmapClient.connect({
        fetch: async (input, init) => {
          const url = input instanceof Request ? input.url : String(input);
          if (url === SESSION_URL) {
            return Response.json(sessionJson());
          }
          scripted.usings.push(usingOf(init));
          const calls = await methodCallsOf(init);
          scripted.calls.push(...calls);
          const { handlers, respond } = scripted.script;
          if (respond !== undefined) {
            return await respond(calls);
          }
          const methodResponses = calls.flatMap(([name, args, callId]) => {
            const handler = handlers[name];
            return handler === undefined
              ? [methodError("unknownMethod", callId)]
              : handler(args, callId);
          });
          return Response.json({ methodResponses, sessionState: "s1" });
        },
        getAccessToken: async () => await Promise.resolve("token"),
        sessionUrl: SESSION_URL,
      }),
    script,
  };
  return scripted;
};
