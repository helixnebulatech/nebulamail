// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import type { BlockReason } from "./types";

/** Internal error carrying a policy decision. Its message is only the reason: no URL or IP. */
export class GuardError extends Error {
  readonly reason: BlockReason;

  constructor(reason: BlockReason) {
    super(reason);
    this.name = "GuardError";
    this.reason = reason;
  }
}
