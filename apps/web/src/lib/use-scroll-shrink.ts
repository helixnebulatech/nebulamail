// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Phones' floating buttons ("New message"…) shrink to their icon while a
// list scrolls down, so they cover less of the rows, and grow back as soon
// as it scrolls up or returns to the top (Gmail's pattern).

import { useEffect, useState } from "react";

/** Scrolled less than this from the top: always the full button. */
const TOP = 48;
/** Ignore jitter smaller than this between two scroll events. */
const STEP = 6;

export const useScrollShrink = (): boolean => {
  const [shrunk, setShrunk] = useState(false);
  useEffect(() => {
    const last = new WeakMap<Element, number>();
    const onScroll = (event: Event): void => {
      const { target } = event;
      if (!(target instanceof Element)) {
        return;
      }
      const top = target.scrollTop;
      const before = last.get(target) ?? 0;
      if (top < TOP) {
        last.set(target, top);
        setShrunk(false);
        return;
      }
      // Slow scrolling moves a pixel or two per event: compare with the
      // last position that counted, not the last event.
      if (Math.abs(top - before) < STEP) {
        return;
      }
      last.set(target, top);
      setShrunk(top > before);
    };
    // Any scroller of the page (the list, the task list…): scroll events
    // don't bubble, so capture them.
    document.addEventListener("scroll", onScroll, {
      capture: true,
      passive: true,
    });
    return () => {
      document.removeEventListener("scroll", onScroll, { capture: true });
    };
  }, []);
  return shrunk;
};
