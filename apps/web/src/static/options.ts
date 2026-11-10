// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Build options of the static build (docs/static.md), checked before
// they reach HTML, scripts and the CSP.

import { validateProxyOrigin } from "@nebulamail/mail-render/document";

// NEBULA_BASE_PATH is checked in plugin.ts (`staticBasePath`), with the
// rules of Stalwart 1.0's `urlPrefix`.

/** The image proxy's origin (https), or "" for none. */
export const staticProxyOrigin = (value = ""): string =>
  value === "" ? "" : validateProxyOrigin(value);
