// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// zod compiles fast validators with `new Function`. Workers forbid code
// generation (zod falls back, but only after trying). Since zod 4.6 the
// choice is made when each schema is created, so entry points import this
// module before anything else.

import { z } from "zod";

z.config({ jitless: true });
