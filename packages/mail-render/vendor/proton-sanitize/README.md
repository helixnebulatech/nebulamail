<!-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# Proton sanitizer (vendored)

**Do not modify these files. Our changes live in `packages/mail-render/src/`** (`proton.ts` is the adapter).

|  |  |
| --- | --- |
| Source repository | https://github.com/ProtonMail/WebClients |
| Path | `packages/sanitize/src/` |
| Commit | `1062e77185efa2cd2ea8b9127856f9c292115480` |
| Files | `src/purify.ts`, `src/escape.ts`, `src/purify.test.ts`, `src/escape.test.ts` (the whole `src/` directory at that commit) |
| Licence | GPL-3.0-or-later (`LICENSE`, copied from the repository root at the same commit) |
| Upstream dependency | `dompurify` (upstream pins `^3.4.17`; we use `3.4.17`) |

`purify.ts` imports only `dompurify` and `./escape`, so nothing outside `packages/sanitize` (no `@proton/*`) is needed and there are no shims.

SHA-256 of the copied files:

```
6c2c6abbe6c6a62a7c9cb79a616da00c4ade9df3dd0b4abbcb8a3edeb55c145f  src/purify.ts
dbe6f2498f8a3e0cdb63ba6101f322025b0014e9a0bf5cab17c55dfd47bc40a8  src/escape.ts
6219d91b33657ae8c64758f1ead1c642d73d4134de0e99846d89da405d35e28d  src/purify.test.ts
1c472c3c066eea425601c36b6ec7b33a971e8b9f93399c5f4fd229ae154002e7  src/escape.test.ts
1b3782ccad7b8614100cda30d3faf42fc39f2e97932908c543005053b654ca68  LICENSE
```

The code type-checks unmodified under our strict tsconfig. Upstream's own jest tests run unmodified through vitest (project `proton-upstream` in `vitest.config.ts`, jsdom + globals) to check that the copy behaves as upstream expects with our DOMPurify.

## Updating

Review monthly and immediately on DOMPurify/Proton security advisories:

1. Download `packages/sanitize/src/*` and the root `LICENSE` at the new commit into a fresh empty temporary directory.
2. Diff against these files, read the changes, copy them over unmodified.
3. Update the commit and hashes above, run `bun run test` in `packages/mail-render`.
