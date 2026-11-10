// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// zod compiles fast validators with `new Function`, which the CSP forbids
// (no 'unsafe-eval'); it falls back to the interpreter, but triggers a CSP
// violation first. Since zod 4.6 it decides when each schema is *created*,
// so this must run before any module defines one: router.tsx imports this
// file first (imports run in order, before the importing module's body).
import { z } from "zod";

z.config({ jitless: true });
