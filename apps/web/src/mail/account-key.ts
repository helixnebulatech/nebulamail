// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { getClient } from "./client";
import { loadIdentities } from "./identity-cache";

/**
 * Which account data on this device belongs to: the JMAP server plus the
 * username (account ids like "c" repeat across servers). Needs the session.
 */
export const connectedAccountKey = async (): Promise<string> => {
  const client = await getClient();
  return `${client.session.apiUrl.origin} ${client.session.username}`;
};

/**
 * The same, but without a connection (the app opened offline) it is the
 * account that connected last on this device (identity-cache.ts), so a
 * draft written offline is still kept. Device data is wiped on logout, so
 * that is the signed-in user unless someone else signed in without the
 * previous user logging out; the next connection puts things right (the
 * identities are saved again and a wrong sender is replaced).
 */
export const accountKey = async (): Promise<string> => {
  try {
    return await connectedAccountKey();
  } catch (error) {
    const saved = await loadIdentities().catch(() => null);
    if (saved === null) {
      throw error;
    }
    return saved.account;
  }
};
