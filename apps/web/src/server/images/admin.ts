// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The admin's remote images settings from this server's environment
// (policy.ts), read once.

import { ENV } from "../../env.server";
import type { AdminImagePolicy } from "../../mail/image-policy";
import { envSetting } from "../env-setting";
import { imagePolicyFromEnv } from "./policy";

let policy: AdminImagePolicy | undefined;

/** Throws on a contradiction: server.ts calls it when the server starts. */
export const adminImagePolicy = (): AdminImagePolicy => {
  policy ??= imagePolicyFromEnv(
    ENV,
    envSetting(ENV.IMAGE_PROXY_ORIGIN) !== undefined &&
      envSetting(ENV.IMAGE_PROXY_KEYS) !== undefined
  );
  return policy;
};
