// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Which content types a storage keeps: one rule for every storage (disk,
// S3, Vercel Blob, R2) instead of a copy in each. Allowed raster images, and
// the preload records of email tokens (`preload/records.ts`), which only
// `getRecord` reads: `/i/` and `/batch` still pass `checkImage`, which
// refuses anything that is not an image.

import { allowedImageType, mediaType } from "@nebulamail/net-guard";

import { PRELOAD_RECORD_TYPE } from "./preload/records";

/** The canonical type a storage keeps for `contentType`, or `undefined` (drop it). */
export const storedType = (
  contentType: string | undefined
): string | undefined =>
  allowedImageType(contentType) ??
  (mediaType(contentType) === PRELOAD_RECORD_TYPE
    ? PRELOAD_RECORD_TYPE
    : undefined);
