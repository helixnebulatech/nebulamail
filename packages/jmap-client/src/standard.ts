// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Generic shapes of the RFC 8620 §5 standard methods (/get, /changes, /set,
// /query, /queryChanges), as TypeScript types for arguments and zod schemas
// for responses. Response schemas use `looseObject` so vendor fields pass
// through untouched instead of failing the parse.

import { z } from "zod";

import { AccountId, State } from "./ids";

export const UnsignedInt = z.number().int().nonnegative();

/**
 * Some properties of `T` (what zod's `.partial()` produces; explicit
 * `undefined` is allowed because the tsconfig uses exactOptionalPropertyTypes).
 */
export type Lenient<T> = { [K in keyof T]?: T[K] | undefined };

// ---------------------------------------------------------------- /get

export interface GetArgs<Id, Property extends string> {
  readonly accountId: AccountId;
  /** `null`/omitted means "all" (only sensible for small types). */
  readonly ids?: readonly Id[] | null;
  readonly properties?: readonly Property[];
}

export interface GetResponse<T, Id> {
  readonly accountId: AccountId;
  readonly state: State;
  readonly list: T[];
  readonly notFound: Id[];
}

export const getResponseSchema = <T extends z.ZodType, I extends z.ZodType>(
  item: T,
  id: I
) =>
  z.looseObject({
    accountId: AccountId,
    list: z.array(item),
    notFound: z
      .array(id)
      .nullish()
      .transform((ids) => ids ?? []),
    state: State,
  });

// ---------------------------------------------------------------- /changes

export interface ChangesArgs {
  readonly accountId: AccountId;
  readonly sinceState: State;
  readonly maxChanges?: number;
}

export interface ChangesResponse<Id> {
  readonly accountId: AccountId;
  readonly oldState: State;
  readonly newState: State;
  readonly hasMoreChanges: boolean;
  readonly created: Id[];
  readonly updated: Id[];
  readonly destroyed: Id[];
}

export const changesResponseSchema = <I extends z.ZodType>(id: I) =>
  z.looseObject({
    accountId: AccountId,
    created: z.array(id),
    destroyed: z.array(id),
    hasMoreChanges: z.boolean(),
    newState: State,
    oldState: State,
    updated: z.array(id),
  });

// ---------------------------------------------------------------- /set

/** RFC 8620 §5.3 SetError. */
export const SetErrorSchema = z.looseObject({
  description: z.string().nullish(),
  properties: z.array(z.string()).nullish(),
  type: z.string(),
});
export type SetError = z.infer<typeof SetErrorSchema>;

/**
 * RFC 8620 §5.3 PatchObject: keys are JSON Pointer paths relative to the
 * object (`"keywords/$seen"`), values replace the target; `null` removes.
 */
export type PatchObject = Readonly<Record<string, unknown>>;

export interface SetArgs<Id, Create, Patch = PatchObject> {
  readonly accountId: AccountId;
  readonly ifInState?: State;
  /** Keys are creation ids; reference them elsewhere as `#key`. */
  readonly create?: Readonly<Record<string, Create>>;
  readonly update?: Readonly<Partial<Record<Id & string, Patch>>>;
  readonly destroy?: readonly Id[];
}

export interface SetResponse<T, Id> {
  readonly accountId: AccountId;
  readonly oldState: State | null;
  /**
   * null when the server left it out: Stalwart 1.0.1 does when it refused
   * everything (nothing changed; RFC 8620 §5.3 says it's required).
   */
  readonly newState: State | null;
  /** Server-set properties of created objects (always includes `id`). */
  readonly created: Partial<Record<string, Lenient<T> & { readonly id: Id }>>;
  readonly updated: Partial<Record<Id & string, Lenient<T> | null>>;
  readonly destroyed: Id[];
  readonly notCreated: Partial<Record<string, SetError>>;
  readonly notUpdated: Partial<Record<Id & string, SetError>>;
  readonly notDestroyed: Partial<Record<Id & string, SetError>>;
}

const orEmptyRecord = <V extends z.ZodType>(value: V) =>
  z
    .record(z.string(), value)
    .nullish()
    .transform((record) => record ?? {});

/**
 * `partialItem` checks `created` (which has the id); `updatedItem` the
 * values of `updated`, which only hold what the server changed, without
 * the id (RFC 8620 §5.3; Stalwart 1.0.1 sends a FileNode's new size
 * there): pass the item's partial schema without a required id.
 */
export const setResponseSchema = <
  T extends z.ZodType,
  I extends z.ZodType,
  U extends z.ZodType = T,
>(
  partialItem: T,
  id: I,
  updatedItem?: U
) =>
  z.looseObject({
    accountId: AccountId,
    created: orEmptyRecord(partialItem),
    destroyed: z
      .array(id)
      .nullish()
      .transform((ids) => ids ?? []),
    // Stalwart 1.0.1 leaves it out when it refused everything (checked
    // 2026-11-02 with a refused HOLDUNTIL): the refusals must still be read.
    newState: State.nullish().transform((state) => state ?? null),
    notCreated: orEmptyRecord(SetErrorSchema),
    notDestroyed: orEmptyRecord(SetErrorSchema),
    notUpdated: orEmptyRecord(SetErrorSchema),
    oldState: State.nullish().transform((state) => state ?? null),
    updated: orEmptyRecord((updatedItem ?? partialItem).nullable()),
  });

// ---------------------------------------------------------------- /query

export interface FilterOperator<Condition> {
  readonly operator: "AND" | "NOT" | "OR";
  readonly conditions: readonly Filter<Condition>[];
}

export type Filter<Condition> = Condition | FilterOperator<Condition>;

export interface Comparator<Property extends string> {
  readonly property: Property;
  readonly isAscending?: boolean;
  readonly collation?: string;
}

export interface QueryArgs<Condition, Sort> {
  readonly accountId: AccountId;
  readonly filter?: Filter<Condition> | null;
  readonly sort?: readonly Sort[] | null;
  readonly position?: number;
  readonly anchor?: string;
  readonly anchorOffset?: number;
  readonly limit?: number;
  readonly calculateTotal?: boolean;
}

export interface QueryResponse<Id> {
  readonly accountId: AccountId;
  readonly queryState: State;
  readonly canCalculateChanges: boolean;
  readonly position: number;
  readonly ids: Id[];
  readonly total?: number | undefined;
  readonly limit?: number | undefined;
}

export const queryResponseSchema = <I extends z.ZodType>(id: I) =>
  z.looseObject({
    accountId: AccountId,
    canCalculateChanges: z.boolean(),
    ids: z.array(id),
    limit: UnsignedInt.optional(),
    position: UnsignedInt,
    queryState: State,
    total: UnsignedInt.optional(),
  });

// ---------------------------------------------------------------- /queryChanges

export interface QueryChangesArgs<Condition, Sort> {
  readonly accountId: AccountId;
  readonly filter?: Filter<Condition> | null;
  readonly sort?: readonly Sort[] | null;
  readonly sinceQueryState: State;
  readonly maxChanges?: number;
  readonly upToId?: string;
  readonly calculateTotal?: boolean;
}

export interface QueryChangesResponse<Id> {
  readonly accountId: AccountId;
  readonly oldQueryState: State;
  readonly newQueryState: State;
  readonly total?: number | undefined;
  readonly removed: Id[];
  readonly added: { readonly id: Id; readonly index: number }[];
}

export const queryChangesResponseSchema = <I extends z.ZodType>(id: I) =>
  z.looseObject({
    accountId: AccountId,
    added: z.array(z.looseObject({ id, index: UnsignedInt })),
    newQueryState: State,
    oldQueryState: State,
    removed: z.array(id),
    total: UnsignedInt.optional(),
  });
