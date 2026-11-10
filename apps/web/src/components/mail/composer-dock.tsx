// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The dock of the floating compose windows, bottom right. Light on
// purpose: the windows (composer.tsx, with the TipTap editor) are a chunk
// of their own, loaded right after the first render, so the mail,
// calendar and tasks screens render and hydrate without waiting for them.

import { Spinner } from "@nebulamail/ui";
import {
  colors,
  font,
  radius,
  shadow,
  zIndex,
} from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import {
  Suspense,
  createContext,
  lazy,
  useEffect,
  useRef,
  useState,
} from "react";
import type { PointerEvent } from "react";

import { whenIdle } from "../../lib/idle";
import { loadWhenOnline } from "../../lib/load-when-online";
import { m } from "../../paraglide/messages.js";
import { bp } from "./breakpoints.stylex";
import { useComposer } from "./compose-context";

const styles = stylex.create({
  // Floats 16px inside the panels' edges (shell.tsx: 10 px padding, 52 px
  // rail), so its border never lines up with the reader's and it reads as
  // a window over the page, with its shadow, not as a panel cut into it.
  // Phones: full screen.
  dock: {
    alignItems: "flex-end",
    bottom: { default: "26px", [bp.mobile]: 0 },
    columnGap: "10px",
    display: "flex",
    flexDirection: "row-reverse",
    // --nb-dock-shift: how far the user dragged the windows sideways.
    insetInlineEnd: {
      default: "calc(78px + var(--nb-dock-shift, 0px))",
      [bp.narrow]: "calc(26px + var(--nb-dock-shift, 0px))",
      [bp.mobile]: 0,
    },
    pointerEvents: "none",
    position: "fixed",
    zIndex: zIndex.popover,
  },
  // The window's chip while its code loads (same look as composer.tsx's).
  loading: {
    alignItems: "center",
    backgroundColor: colors.raised,
    borderColor: colors.line2,
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: "1px",
    boxShadow: shadow.sh2,
    columnGap: "6px",
    display: "flex",
    fontSize: font.sizeMd,
    fontWeight: font.weightSemibold,
    height: "40px",
    paddingInlineEnd: "6px",
    paddingInlineStart: "12px",
    pointerEvents: "auto",
    width: "260px",
  },
});

const dockShift = stylex.create({
  value: (px: string) => ({ "--nb-dock-shift": px }),
});

/** Room kept between the dragged windows and the screen edge. */
const DOCK_MARGIN = 8;

/** Starts dragging the dock from a window's title bar (see ComposerDock). */
export const DockDrag = createContext<
  ((event: PointerEvent<HTMLElement>) => void) | null
>(null);

const loadComposer = async () =>
  await loadWhenOnline(async () => await import("./composer"));

const ComposerWindow = lazy(async () => {
  const { ComposerWindow: Window } = await loadComposer();
  return { default: Window };
});

const Loading = () => (
  <div data-composer {...stylex.props(styles.loading)}>
    <Spinner size="sm" />
  </div>
);

/**
 * All compose windows, docked bottom right. Dragging a window's title bar
 * moves them all sideways, never off the screen; a new window size puts
 * them back in the corner.
 */
export const ComposerDock = () => {
  const { windows } = useComposer();
  const dock = useRef<HTMLElement>(null);
  const [shift, setShift] = useState(0);

  // The windows' code once the first screen is done, not in its way (the
  // editor is the biggest chunk of the app): a window still opens at once,
  // and still offline (pwa/sw.ts keeps the files it fetched).
  // Opening one sooner loads it then (ComposerWindow).
  useEffect(
    () =>
      whenIdle(() => {
        void loadComposer();
      }),
    []
  );

  useEffect(() => {
    const reset = (): void => {
      setShift(0);
    };
    globalThis.addEventListener("resize", reset);
    return () => {
      globalThis.removeEventListener("resize", reset);
    };
  }, []);

  // The context value: the React Compiler keeps it the same function (it
  // only uses refs and a state setter), so the title bars don't redraw.
  const startDrag = (event: PointerEvent<HTMLElement>): void => {
    const section = dock.current;
    const bar = event.currentTarget;
    if (
      section === null ||
      event.button !== 0 ||
      (event.target instanceof Element && event.target.closest("button"))
    ) {
      return;
    }
    const box = section.getBoundingClientRect();
    // Full width (phones): nothing to move.
    if (box.width >= globalThis.innerWidth - 1) {
      return;
    }
    event.preventDefault();
    bar.setPointerCapture(event.pointerId);
    const startX = event.clientX;
    // insetInlineEnd grows to the left in left-to-right pages.
    const sign = getComputedStyle(section).direction === "rtl" ? 1 : -1;
    // How far this drag has moved the dock so far: each move adds only
    // the difference, so the shift ends at (shift at the start) + dx
    // without the handler reading the state.
    let applied = 0;
    const move = (next: globalThis.PointerEvent): void => {
      const dx = Math.min(
        globalThis.innerWidth - DOCK_MARGIN - box.right,
        Math.max(DOCK_MARGIN - box.left, next.clientX - startX)
      );
      const step = sign * dx - applied;
      applied = sign * dx;
      setShift((current) => current + step);
    };
    const end = (): void => {
      bar.removeEventListener("pointermove", move);
      bar.removeEventListener("pointerup", end);
      bar.removeEventListener("pointercancel", end);
    };
    bar.addEventListener("pointermove", move);
    bar.addEventListener("pointerup", end);
    bar.addEventListener("pointercancel", end);
  };

  if (windows.length === 0) {
    return null;
  }
  return (
    <section
      aria-label={m.composer_windows()}
      ref={dock}
      {...stylex.props(styles.dock, dockShift.value(`${shift}px`))}
    >
      {/* oxlint-disable-next-line react/jsx-no-constructed-context-values -- the React Compiler memoizes startDrag (only refs and a state setter inside): made once. */}
      <DockDrag value={startDrag}>
        {/* Newest first (row-reverse puts it at the right edge). */}
        {windows.toReversed().map((window) => (
          <Suspense fallback={<Loading />} key={window.key}>
            <ComposerWindow window={window} />
          </Suspense>
        ))}
      </DockDrag>
    </section>
  );
};
