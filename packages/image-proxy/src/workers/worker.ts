// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Worker entry point (`main` in wrangler.example.toml).

// First: zod must not try `new Function` (Workers forbid it), and it decides
// when each schema is created, so this runs before any other import.
import "../zod-jitless";
import { createWorkersProxy } from "./proxy";

export default createWorkersProxy();
