// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Storage used and its limit (RFC 9425 JMAP Quotas), for the bar in the
// sidebar. Hidden when the server or the account has no quota. Stalwart
// has one "octets" quota per account (the account's disk quota) and no
// warn or soft limits unless the admin sets them.

import type { Quota } from "@nebulamail/jmap-client";
import { CAPABILITIES, hasCapability } from "@nebulamail/jmap-client";

import { getClient } from "./client";

export type QuotaLevel = "ok" | "warn" | "full";

export interface StorageQuota {
  readonly used: number;
  readonly limit: number;
  readonly level: QuotaLevel;
}

/** Without a warn limit from the server: warn at 80 %, "full" at 95 %. */
const WARN_RATIO = 0.8;
const FULL_RATIO = 0.95;

type QuotaFields = Pick<
  Quota,
  | "resourceType"
  | "used"
  | "hardLimit"
  | "warnLimit"
  | "softLimit"
  | "scope"
  | "types"
>;

export const quotaLevel = (quota: QuotaFields): QuotaLevel => {
  const full = quota.softLimit ?? quota.hardLimit * FULL_RATIO;
  const warn = quota.warnLimit ?? quota.hardLimit * WARN_RATIO;
  if (quota.used >= full) {
    return "full";
  }
  return quota.used >= warn ? "warn" : "ok";
};

/**
 * The storage quota to show: bytes ("octets") with a limit that counts
 * emails (RFC 9425 §4.1 `types` has "Email"; a Calendar- or file-only quota
 * isn't mail storage); of several, the one closest to full (an account
 * quota inside a domain quota…).
 */
export const pickQuota = (
  quotas: readonly QuotaFields[]
): StorageQuota | null => {
  const [closest] = quotas
    .filter(
      (quota) =>
        quota.resourceType === "octets" &&
        quota.hardLimit > 0 &&
        quota.types.includes("Email")
    )
    .toSorted((a, b) => b.used / b.hardLimit - a.used / a.hardLimit);
  return closest === undefined
    ? null
    : {
        level: quotaLevel(closest),
        limit: closest.hardLimit,
        used: closest.used,
      };
};

/** null: no quota capability, or no storage limit for this account. */
export const getStorageQuota = async (): Promise<StorageQuota | null> => {
  const client = await getClient();
  const accountId = client.primaryAccountId();
  if (!hasCapability(client.session, CAPABILITIES.quota, accountId)) {
    return null;
  }
  const [result] = await client.request((b) => [
    b.call("Quota/get", {
      accountId,
      ids: null,
      properties: [
        "resourceType",
        "used",
        "hardLimit",
        "warnLimit",
        "softLimit",
        "scope",
        "types",
      ],
    }),
  ]);
  return pickQuota(result.list);
};
