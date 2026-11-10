// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// JMAP Sharing (RFC 9670, `urn:ietf:params:jmap:principals`): the users and
// groups an object can be shared with. Mailbox sharing itself is
// `Mailbox.shareWith` (`urn:ietf:params:jmap:mail:share`, mail.ts).
//
// Stalwart: `Principal/query` lists every principal of the server (with
// `Sharing.allowDirectoryQueries`); the `text` and `name`
// filters match whole words only ("bo" doesn't find "Bob"), `email` an
// exact address.

import { z } from "zod";

import { PrincipalId } from "./ids";
import type { GetArgs, QueryArgs } from "./standard";

export const PrincipalSchema = z.looseObject({
  description: z.string().nullish(),
  email: z.string().nullish(),
  id: PrincipalId,
  name: z.string(),
  timeZone: z.string().nullish(),
  /** "individual", "group", "resource", "location" or "other". */
  type: z.string(),
});
export type Principal = z.infer<typeof PrincipalSchema>;
export type PrincipalProperty = keyof typeof PrincipalSchema.shape;

export type PrincipalGetArgs = GetArgs<PrincipalId, PrincipalProperty>;

/** RFC 9670 §3.3.1. */
export interface PrincipalFilterCondition {
  readonly accountIds?: readonly string[];
  readonly email?: string;
  readonly name?: string;
  readonly text?: string;
  readonly type?: string;
  readonly timeZone?: string;
}

export type PrincipalQueryArgs = QueryArgs<
  PrincipalFilterCondition,
  {
    readonly property: "name" | "email" | "type";
    readonly isAscending?: boolean;
  }
>;
