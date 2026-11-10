// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// JMAP Quotas (RFC 9425, `urn:ietf:params:jmap:quota`). Stalwart: an
// "octets" quota per account with `hardLimit` (the account's disk quota),
// and an empty list when the account has none.

import { z } from "zod";

import { QuotaId } from "./ids";
import { UnsignedInt } from "./standard";
import type { GetArgs } from "./standard";

export const QuotaSchema = z.looseObject({
  description: z.string().nullish(),
  hardLimit: UnsignedInt,
  id: QuotaId,
  name: z.string(),
  /** "count" (number of objects) or "octets" (bytes). */
  resourceType: z.string(),
  /** "account", "domain" or "global". */
  scope: z.string(),
  softLimit: UnsignedInt.nullish(),
  /** Data types it counts ("Email", "ContactCard"…). */
  types: z.array(z.string()),
  used: UnsignedInt,
  warnLimit: UnsignedInt.nullish(),
});
export type Quota = z.infer<typeof QuotaSchema>;
export type QuotaProperty = keyof typeof QuotaSchema.shape;

export type QuotaGetArgs = GetArgs<QuotaId, QuotaProperty>;
