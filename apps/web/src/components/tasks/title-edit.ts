// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// `e` in the list asks the task panel (which may not be open yet) to put
// the cursor in the title: the request waits until a panel takes it.

import { useEffect, useEffectEvent } from "react";

const EVENT = "wm-task-edit-title";
let pending = false;

/** Asks the open (or opening) task panel to focus its title. */
export const requestTitleEdit = (): void => {
  pending = true;
  globalThis.dispatchEvent(new Event(EVENT));
};

/** Calls `focus` when the title should be edited (now or later). */
export const useTitleEditRequest = (focus: () => void): void => {
  const run = useEffectEvent(focus);
  useEffect(() => {
    const take = (): void => {
      if (pending) {
        pending = false;
        // After the panel has painted its inputs.
        requestAnimationFrame(() => {
          run();
        });
      }
    };
    take();
    globalThis.addEventListener(EVENT, take);
    return () => {
      globalThis.removeEventListener(EVENT, take);
    };
  }, []);
};
