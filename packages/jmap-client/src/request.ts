// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Typed request builder. One builder = one logical JMAP request (RFC 8620
// §3.3): several method calls, where later calls can use the results of
// earlier ones through result references (`#ids`, RFC 8620 §3.7).

import { z } from "zod";

import { CAPABILITIES } from "./capabilities";
import { JmapError, isJmapError, zodIssues } from "./errors";
import { METHODS } from "./methods";
import type {
  Invocation,
  MethodArgs,
  MethodName,
  ResponseFor,
} from "./methods";

// ---------------------------------------------------------------- JSON Pointer types

type Split<S extends string> = S extends `${infer Head}/${infer Tail}`
  ? [Head, ...Split<Tail>]
  : [S];

type Flat<X> = [X] extends [never]
  ? never
  : X extends readonly (infer Item)[]
    ? Item[]
    : X[];

type Walk<T, Path extends readonly string[]> = Path extends readonly []
  ? T
  : Path extends readonly [
        infer Head extends string,
        ...infer Rest extends readonly string[],
      ]
    ? Head extends "*"
      ? T extends readonly (infer Item)[]
        ? Flat<Walk<Item, Rest>>
        : never
      : T extends readonly unknown[]
        ? never
        : Head extends keyof T
          ? Walk<NonNullable<T[Head]>, Rest>
          : never
    : never;

/**
 * Type at JSON Pointer `P` inside `R`, with RFC 8620 §3.7 `*` semantics
 * (map over an array, flattening one level). `never` if the path is invalid.
 */
export type PointerValue<R, P extends string> = P extends `/${infer Rest}`
  ? Walk<R, Split<Rest>>
  : never;

declare const valueType: unique symbol;

/** `{ resultOf, name, path }`; `T` is the type the server will substitute. */
export interface ResultReference<T> {
  readonly resultOf: string;
  readonly name: string;
  readonly path: string;
  readonly [valueType]?: T;
}

type CheckPath<R, P extends string> = [PointerValue<R, P>] extends [never]
  ? "invalid result reference path for this response"
  : unknown;

/** Placeholder for one method call; resolved to `R` after the request. */
export interface CallHandle<R> {
  readonly callId: string;
  readonly method: string;
  readonly ref: <const P extends string>(
    path: P & CheckPath<R, P>
  ) => ResultReference<PointerValue<R, P>>;
  readonly [valueType]?: R;
}

// ---------------------------------------------------------------- argument types

type KeysOfUnion<T> = T extends unknown ? keyof T : never;

/** Arguments that may be passed as a result reference instead of a value. */
type ReferableKey<A> = Extract<keyof A, "emailIds" | "ids">;

type WithRef<A, K extends keyof A & string> = [K] extends [never]
  ? A
  : Omit<A, K> &
      (
        | (Pick<A, K> & { readonly [P in K as `#${P}`]?: never })
        | (Readonly<Partial<Record<K, never>>> & {
            readonly [P in K as `#${P}`]: ResultReference<NonNullable<A[P]>>;
          })
      );

/** Arguments accepted by `builder.call(M, …)`. */
export type CallArgs<M extends MethodName> = WithRef<
  MethodArgs<M>,
  ReferableKey<MethodArgs<M>> & string
>;

/** Makes unknown argument names a compile error despite generic inference. */
type NoExtra<A, Allowed> = Readonly<
  Record<Exclude<keyof A, KeysOfUnion<Allowed>>, never>
>;

// ---------------------------------------------------------------- results

export type Settled<T> =
  | { readonly ok: true; readonly value: T }
  | { readonly ok: false; readonly error: JmapError };

/** Replaces every `CallHandle<R>` in a tuple/object/handle with `R`. */
export type Resolved<T> =
  T extends CallHandle<infer R>
    ? R
    : { -readonly [K in keyof T]: T[K] extends CallHandle<infer R> ? R : T[K] };

/** Like `Resolved`, but each value is a `Settled<R>`. */
export type SettledResults<T> =
  T extends CallHandle<infer R>
    ? Settled<R>
    : {
        -readonly [K in keyof T]: T[K] extends CallHandle<infer R>
          ? Settled<R>
          : T[K];
      };

// ---------------------------------------------------------------- builder

interface PendingCall {
  readonly callId: string;
  readonly name: string;
  readonly args: Readonly<Record<string, unknown>>;
  readonly referencedCallIds: readonly string[];
  readonly parse: (
    raw: Record<string, unknown>,
    extra: readonly Invocation[]
  ) => unknown;
}

export interface WireRequest {
  readonly using: string[];
  readonly methodCalls: [string, Readonly<Record<string, unknown>>, string][];
}

export class RequestBuilder {
  readonly #calls: PendingCall[] = [];
  readonly #handles = new WeakSet<object>();
  readonly #refs = new WeakSet<object>();
  readonly #using = new Set<string>([CAPABILITIES.core]);

  /** Adds a typed method call. Its result is available after the request. */
  call<M extends MethodName, const A extends CallArgs<M>>(
    name: M,
    args: A & NoExtra<A, CallArgs<M>>
  ): CallHandle<ResponseFor<M, A>> {
    const spec = METHODS[name];
    for (const capability of spec.using) {
      this.#using.add(capability);
    }
    return this.#add(name, args, (raw, extra) => spec.parse(args, raw, extra));
  }

  /**
   * Adds a call to a method this package does not type (e.g. Sieve, Quota,
   * vendor extensions). The response is checked with the given zod schema.
   */
  extension<T>(
    name: string,
    args: Readonly<Record<string, unknown>>,
    options: {
      readonly using: readonly string[];
      readonly response: z.ZodType<T>;
    }
  ): CallHandle<T> {
    for (const capability of options.using) {
      this.#using.add(capability);
    }
    return this.#add(name, args, (raw) => {
      const result = options.response.safeParse(raw);
      if (result.success) {
        return result.data;
      }
      throw new JmapError({
        issues: zodIssues(result.error),
        kind: "invalidResponse",
      });
    });
  }

  /** Adds capabilities to `using` (normally inferred from method names). */
  using(...capabilities: readonly string[]): this {
    for (const capability of capabilities) {
      this.#using.add(capability);
    }
    return this;
  }

  /**
   * Whether `value` is a handle created by this builder.
   * @internal
   */
  isHandle(value: unknown): value is CallHandle<unknown> {
    return (
      typeof value === "object" && value !== null && this.#handles.has(value)
    );
  }

  /**
   * Splits into wire requests of at most `maxCalls` calls.
   * @internal
   */
  chunks(maxCalls: number): { request: WireRequest; calls: PendingCall[] }[] {
    const size = Math.max(1, maxCalls);
    const groups: PendingCall[][] = [];
    for (let start = 0; start < this.#calls.length; start += size) {
      groups.push(this.#calls.slice(start, start + size));
    }
    return groups.map((calls) => {
      const local = new Set(calls.map((call) => call.callId));
      const crossing = calls.some((call) =>
        call.referencedCallIds.some((id) => !local.has(id))
      );
      if (crossing) {
        throw new JmapError({
          actual: this.#calls.length,
          kind: "limit",
          limit: "maxCallsInRequest",
          max: size,
        });
      }
      return {
        calls,
        request: {
          methodCalls: calls.map((call) => [call.name, call.args, call.callId]),
          using: [...this.#using],
        },
      };
    });
  }

  #add<R>(
    name: string,
    args: Readonly<Record<string, unknown>>,
    parse: PendingCall["parse"]
  ): CallHandle<R> {
    const callId = `c${String(this.#calls.length)}`;
    this.#calls.push({
      args,
      callId,
      name,
      parse,
      referencedCallIds: this.#checkReferences(args),
    });
    const handle: CallHandle<R> = {
      callId,
      method: name,
      ref: (path) => {
        const reference = { name, path, resultOf: callId };
        this.#refs.add(reference);
        return reference;
      },
    };
    this.#handles.add(handle);
    return handle;
  }

  #checkReferences(args: Readonly<Record<string, unknown>>): string[] {
    const ids: string[] = [];
    for (const [key, value] of Object.entries(args)) {
      if (!key.startsWith("#")) {
        continue;
      }
      if (Object.hasOwn(args, key.slice(1))) {
        throw new JmapError({
          kind: "usage",
          reason: `both "${key.slice(1)}" and "${key}" given`,
        });
      }
      if (
        typeof value !== "object" ||
        value === null ||
        !this.#refs.has(value)
      ) {
        throw new JmapError({
          kind: "usage",
          reason: `"${key}" must be a reference from this request`,
        });
      }
      const target = Object.getOwnPropertyDescriptor(value, "resultOf");
      if (typeof target?.value === "string") {
        ids.push(target.value);
      }
    }
    return ids;
  }
}

// ---------------------------------------------------------------- responses

const InvocationSchema = z.tuple([
  z.string(),
  z.record(z.string(), z.unknown()),
  z.string(),
]);

export const ResponseSchema = z.looseObject({
  createdIds: z.record(z.string(), z.string()).optional(),
  methodResponses: z.array(InvocationSchema),
  sessionState: z.string(),
});

const MethodErrorSchema = z.looseObject({
  description: z.string().optional(),
  type: z.string(),
});

const settleOne = (
  call: PendingCall,
  primary: Invocation | undefined,
  extra: readonly Invocation[]
): Settled<unknown> => {
  if (primary === undefined) {
    return {
      error: new JmapError({
        issues: [`no response for ${call.callId}`],
        kind: "invalidResponse",
      }),
      ok: false,
    };
  }
  const [name, raw] = primary;
  if (name === "error") {
    const parsed = MethodErrorSchema.safeParse(raw);
    const type = parsed.success ? parsed.data.type : "unknown";
    const description = parsed.success ? parsed.data.description : undefined;
    return {
      error: new JmapError({
        arguments: raw,
        callId: call.callId,
        kind: "method",
        method: call.name,
        type,
        ...(description === undefined ? {} : { description }),
      }),
      ok: false,
    };
  }
  try {
    return { ok: true, value: call.parse(raw, extra) };
  } catch (error) {
    if (isJmapError(error)) {
      return { error, ok: false };
    }
    throw error;
  }
};

/** Turns raw method responses into one `Settled` per pending call. */
export const settleCalls = (
  calls: readonly PendingCall[],
  responses: readonly Invocation[]
): Map<string, Settled<unknown>> => {
  const byCallId = Map.groupBy(responses, (invocation) => invocation[2]);
  const settled = new Map<string, Settled<unknown>>();
  for (const call of calls) {
    const [primary, ...extra] = byCallId.get(call.callId) ?? [];
    settled.set(call.callId, settleOne(call, primary, extra));
  }
  return settled;
};
