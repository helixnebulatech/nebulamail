// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Cache keys: per account, never containing the URL or the
// account id in clear. Format: `v1:<sha256(accountId)>:<sha256(url)>`, hex.

import { hashUrl } from "./tickets/ticket";

/** Matches every key the core produces; storages may assert it. */
export const STORAGE_KEY_PATTERN = /^v1:[0-9a-f]{64}:[0-9a-f]{64}$/u;

export const storageKey = async (
  accountId: string,
  urlHash: string
): Promise<string> => `v1:${await hashUrl(accountId)}:${urlHash}`;
