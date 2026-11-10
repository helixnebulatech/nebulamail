// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Contact photos chosen from the device, stored in the card as a small
// `data:` JPEG (vCard PHOTO with a data URI, as other clients do; the
// bridge's CardDAV servers keep it as it is). The browser decodes and
// redraws it, so whatever was in the file (metadata, location, a polyglot)
// doesn't survive: only pixels do. Stalwart hands inline photos back as
// blobs (card.ts): those are redrawn the same way.

import { MAX_PHOTO_DATA_URL } from "./card";

const ACCEPTED = new Set([
  "image/png",
  "image/jpeg",
  "image/webp",
  "image/gif",
]);

/** For `<input type="file" accept>`. Never SVG. */
export const PHOTO_ACCEPT = [...ACCEPTED].join(",");

/** Longest side of the stored photo. */
const SIZE = 256;

/** Files bigger than this aren't even decoded (nor downloaded). */
export const MAX_PHOTO_FILE_BYTES = 10 * 1024 * 1024;

export type PhotoProblem = "type" | "too-big" | "unreadable";

export class PhotoError extends Error {
  readonly problem: PhotoProblem;

  constructor(problem: PhotoProblem) {
    super(problem);
    this.name = "PhotoError";
    this.problem = problem;
  }
}

/** Square-ish crop size and position to cover a SIZE×SIZE box. */
export const coverCrop = (
  width: number,
  height: number
): { readonly sx: number; readonly sy: number; readonly side: number } => {
  const side = Math.min(width, height);
  return {
    side,
    sx: Math.floor((width - side) / 2),
    sy: Math.floor((height - side) / 2),
  };
};

/**
 * Decodes, crops to a square and re-encodes as a small JPEG data: URL. The
 * blob's `type` must be a raster type (a chosen file, or a downloaded
 * photo typed with its card's `mediaType`).
 */
export const photoFromFile = async (file: Blob): Promise<string> => {
  if (!ACCEPTED.has(file.type)) {
    throw new PhotoError("type");
  }
  if (file.size > MAX_PHOTO_FILE_BYTES) {
    throw new PhotoError("too-big");
  }
  let bitmap: ImageBitmap;
  try {
    bitmap = await createImageBitmap(file);
  } catch {
    throw new PhotoError("unreadable");
  }
  const canvas = document.createElement("canvas");
  canvas.width = SIZE;
  canvas.height = SIZE;
  const context = canvas.getContext("2d");
  if (context === null) {
    bitmap.close();
    throw new PhotoError("unreadable");
  }
  const { side, sx, sy } = coverCrop(bitmap.width, bitmap.height);
  context.fillStyle = "#ffffff";
  context.fillRect(0, 0, SIZE, SIZE);
  context.drawImage(bitmap, sx, sy, side, side, 0, 0, SIZE, SIZE);
  bitmap.close();
  for (const quality of [0.85, 0.7, 0.5]) {
    const url = canvas.toDataURL("image/jpeg", quality);
    if (url.length <= MAX_PHOTO_DATA_URL) {
      return url;
    }
  }
  throw new PhotoError("too-big");
};
