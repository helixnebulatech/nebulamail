// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

/**
 * An environment value, or undefined when it's unset *or empty*: varlock
 * passes an empty variable through as "" (e.g. `IMAGE_PROXY_KEYS=` set by
 * hand or by the browser tests, e2e/env.ts), and that means "off" too.
 */
export const envSetting = (value: string | undefined): string | undefined =>
  value === undefined || value.trim() === "" ? undefined : value;
