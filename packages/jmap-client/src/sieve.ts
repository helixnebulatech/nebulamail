// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// JMAP for Sieve Scripts (RFC 9661). A script's content is a blob: upload
// it, then create or update the SieveScript with its `blobId`. Only one
// script per account is active at a time. Checked against Stalwart:
//
// - VacationResponse (RFC 8621 §8) is a script named "vacation" that the
//   server writes. Turning the vacation response on activates it (and turns
//   the active script off); activating another script turns it off. The
//   name is reserved and the script can't be changed with SieveScript/set.
// - Validation errors say "… at line N, column M." (`sieveErrorPosition`).

// See ids.ts for why each id is a schema and a type with the same name.
/* oxlint-disable no-redeclare */

import { z } from "zod";

import { CAPABILITIES } from "./capabilities";
import { parseOrThrow } from "./errors";
import { AccountId, BlobId } from "./ids";
import type { Session } from "./session";
import {
  SetErrorSchema,
  UnsignedInt,
  getResponseSchema,
  queryResponseSchema,
  setResponseSchema,
} from "./standard";
import type {
  GetResponse,
  QueryArgs,
  QueryResponse,
  SetArgs,
  SetError,
  SetResponse,
} from "./standard";

export const SieveScriptId = z
  .string()
  .min(1)
  .max(255)
  .regex(/^[\w-]+$/u, "Not a valid JMAP Id")
  .brand<"SieveScriptId">();
export type SieveScriptId = z.infer<typeof SieveScriptId>;

export const SieveScriptSchema = z.looseObject({
  blobId: BlobId,
  id: SieveScriptId,
  isActive: z.boolean(),
  name: z.string().nullable(),
});
export type SieveScript = z.infer<typeof SieveScriptSchema>;

/** The name Stalwart gives the VacationResponse script (reserved). */
export const VACATION_SCRIPT_NAME = "vacation";

/** RFC 9661 §2: the account capability. */
export const SieveAccountCapabilitySchema = z.looseObject({
  maxNumberRedirects: UnsignedInt.nullish(),
  maxNumberScripts: UnsignedInt.nullish(),
  maxSizeScript: UnsignedInt.nullish(),
  maxSizeScriptName: UnsignedInt.nullish(),
  sieveExtensions: z.array(z.string()).catch([]),
});
export type SieveAccountCapability = z.infer<
  typeof SieveAccountCapabilitySchema
>;

/** The Sieve capability of an account, if it has one. */
export const sieveCapability = (
  session: Session,
  accountId: AccountId
): SieveAccountCapability | undefined => {
  const raw =
    session.accounts[accountId]?.accountCapabilities[CAPABILITIES.sieve];
  const parsed = SieveAccountCapabilitySchema.safeParse(raw);
  return parsed.success ? parsed.data : undefined;
};

/** SieveScript/get always returns every property (the object is tiny). */
export interface SieveScriptGetArgs {
  readonly accountId: AccountId;
  readonly ids?: readonly SieveScriptId[] | null;
}

export interface SieveScriptCreate {
  readonly name: string;
  readonly blobId: BlobId;
}

export interface SieveScriptPatch {
  readonly name?: string;
  readonly blobId?: BlobId;
}

export interface SieveScriptSetArgs extends SetArgs<
  SieveScriptId,
  SieveScriptCreate,
  SieveScriptPatch
> {
  /** Id or `#creationId` to activate after the changes (RFC 9661 §2.2). */
  readonly onSuccessActivateScript?: string | null;
  /** Deactivate the active script (ignored with the option above). */
  readonly onSuccessDeactivateScript?: boolean;
}

export interface SieveScriptFilterCondition {
  readonly name?: string;
  readonly isActive?: boolean;
}

export type SieveScriptQueryArgs = QueryArgs<
  SieveScriptFilterCondition,
  { readonly property: "name" | "isActive"; readonly isAscending?: boolean }
>;

export interface SieveScriptValidateArgs {
  readonly accountId: AccountId;
  readonly blobId: BlobId;
}

export interface SieveScriptValidateResponse {
  readonly accountId: AccountId;
  /**
   * `null`: the script is valid. Otherwise type `invalidSieve` (RFC 9661
   * §2.6, the bridge) or `invalidScript` (Stalwart 1.0.1): accept both.
   */
  readonly error: SetError | null;
}

export type SieveScriptGetResponse = GetResponse<SieveScript, SieveScriptId>;
export type SieveScriptSetResponse = SetResponse<SieveScript, SieveScriptId>;

const SieveScriptGetSchema = getResponseSchema(
  SieveScriptSchema,
  SieveScriptId
);
const SieveScriptSetSchema = setResponseSchema(
  SieveScriptSchema.partial().extend({ id: SieveScriptId }),
  SieveScriptId,
  SieveScriptSchema.partial()
);
const SieveScriptValidateSchema = z.looseObject({
  accountId: AccountId,
  error: SetErrorSchema.nullish().transform((value) => value ?? null),
});

interface Spec<A, R> {
  readonly args: A;
  readonly response: R;
}

/** Merged into the method registry (methods.ts). */
export interface SieveMethodMap {
  "SieveScript/get": Spec<SieveScriptGetArgs, SieveScriptGetResponse>;
  "SieveScript/query": Spec<SieveScriptQueryArgs, QueryResponse<SieveScriptId>>;
  "SieveScript/set": Spec<SieveScriptSetArgs, SieveScriptSetResponse>;
  "SieveScript/validate": Spec<
    SieveScriptValidateArgs,
    SieveScriptValidateResponse
  >;
}

const parser =
  <T>(schema: z.ZodType<T>) =>
  (_args: unknown, raw: Record<string, unknown>): T =>
    parseOrThrow(schema, raw);

const USING = [CAPABILITIES.sieve];

export const SIEVE_METHODS = {
  "SieveScript/get": { parse: parser(SieveScriptGetSchema), using: USING },
  "SieveScript/query": {
    parse: parser(queryResponseSchema(SieveScriptId)),
    using: USING,
  },
  "SieveScript/set": { parse: parser(SieveScriptSetSchema), using: USING },
  "SieveScript/validate": {
    parse: parser(SieveScriptValidateSchema),
    using: USING,
  },
} as const;

export interface SieveErrorPosition {
  readonly line: number;
  readonly column: number | null;
}

/**
 * Line (and column) of a validation error, from Stalwart's "… at line 5,
 * column 1." Other servers word it differently: `null` then.
 */
export const sieveErrorPosition = (
  description: string | null | undefined
): SieveErrorPosition | null => {
  const match = /\bline (?<line>\d+)(?:, column (?<column>\d+))?/iu.exec(
    description ?? ""
  );
  if (match === null) {
    return null;
  }
  const line = Number(match.groups?.line);
  const column =
    match.groups?.column === undefined ? null : Number(match.groups.column);
  return Number.isSafeInteger(line) ? { column, line } : null;
};
