// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { useEffect } from "react";

import type { RemoteImages } from "./images";

/**
 * Batch-mode images are `blob:` URLs held in memory: they are freed when
 * replaced by others or when the component closes (an external resource, so
 * an effect cleanup is the right place).
 */
export const useFreeImages = (images: RemoteImages | null): void => {
  useEffect(
    () => () => {
      if (images?.kind === "batch") {
        images.revoke();
      }
    },
    [images]
  );
};
