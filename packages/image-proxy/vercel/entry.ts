// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Vercel Function (Node.js runtime). `bun scripts/vercel-build.ts` bundles
// it as `/api/proxy` (Build Output API); every path is routed here. It is
// not in `api/` on purpose: Vercel would build that folder itself.
// As shipped: batch mode, or cache mode with S3 (env only). Vercel Blob and
// preload need a module passed in here (README, "Vercel" and "Preload").

import { createVercelProxy } from "../src/vercel/index";

const proxy = createVercelProxy();

export default { fetch: proxy.fetch };
