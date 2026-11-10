<!-- SPDX-FileCopyrightText: 2026 Nebula Mail contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# MailTrackerBlocker tracker list (vendored)

**Do not modify these files.** The list is converted to `src/trackers/data.generated.ts` by `scripts/generate-trackers.ts`.

|  |  |
| --- | --- |
| Source repository | https://github.com/apparition47/MailTrackerBlocker |
| File | `Source/MTBBlockedMessage.m` (the `+getTrackerDict` dictionary) |
| Commit | `7463682a39af055a488539a3d0aa7898437b33f5` (2026-09-12) |
| Licence | BSD-3-Clause (`LICENSE`, copied from the repository root; attribution required) |
| Size | 320 services, 468 patterns |

SHA-256:

```
d1da0e46549ec250bbb48532c1308cf76f865fb88a94eaac2977f8115d44f674  MTBBlockedMessage.m
f56cff9d2633fa9f4970f236daeb49391ab21eaee1aa2e1f074a95db6a0d7fcb  LICENSE
```

## How we use it

- Upstream matches each ICU pattern, case-insensitively, against the whole `<img …>` tag. We match it against the image **URL** only, so the browser, the BFF and the image proxy reach the same decision whatever the HTML attributes are.
- Patterns are compiled as JavaScript regexes with flags `iu`. The generator refuses patterns JavaScript cannot compile.
- Commented-out upstream rules are skipped (the generator tokenizes the Objective-C literal).
- Upstream's "generic" spy-pixel regex and the extra CSS rules are not used: our own spy-pixel rule (`src/trackers/spy-pixel.ts`) replaces them. We keep upstream's spacer exceptions (`cid:`, `spacer`, `transparent.gif`…).
- `/CI0/.+/.+=[0-9][0-9][0-9]` (Amazon SES) backtracks cubically. `src/trackers/match.ts` swaps it for an equivalent linear check (`LINEAR_EQUIVALENTS`, tested against the regex). The adversarial timing test in `test/trackers.test.ts` catches new slow patterns after a regeneration.

## Regenerating (monthly)

1. Download `Source/MTBBlockedMessage.m` and `LICENSE` at the new commit into a fresh empty temporary directory, review the diff, copy them here.
2. Update the commit in this README and in `scripts/generate-trackers.ts` (`UPSTREAM_COMMIT`).
3. `bun run generate:trackers` in `packages/mail-render`, then `bun run test`. In CI, `bun scripts/generate-trackers.ts --check` fails if the generated file is stale.
