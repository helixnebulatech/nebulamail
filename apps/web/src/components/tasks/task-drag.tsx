// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Dragging tasks: in a list to change the manual order, onto
// a list of the sidebar to move it there, and in the calendar to change its
// due. Pointer events, like the calendar's own dragging (no library): a
// mouse or pen drags once it moves a few pixels; a finger rests ~0.45 s
// first (moving earlier scrolls the page, as always), and the page doesn't
// scroll while it drags. Esc cancels. Never the only way: Alt + arrows,
// `d` (date) and `v` (list) do the same from the keyboard.

import {
  colors,
  font,
  radius,
  shadow,
  zIndex,
} from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useEffect, useEffectEvent, useSyncExternalStore } from "react";
import type { Ref, RefObject } from "react";
import { createPortal } from "react-dom";

/** How far a mouse or pen moves before a press becomes a drag. */
const DRAG_THRESHOLD = 4;
/** How long a finger rests before it drags (as drawing in the calendar). */
const LONG_PRESS_MS = 450;
/** A finger that moves more than this during the long press scrolls. */
const TOUCH_SLOP = 10;

export interface DragHandlers {
  /** Whether this element can be dragged now. */
  readonly enabled: boolean;
  /** The drag begins (at the pointer's position). */
  readonly onStart: (x: number, y: number) => void;
  readonly onMove: (x: number, y: number) => void;
  readonly onDrop: (x: number, y: number) => void;
  readonly onCancel: () => void;
}

/** Presses on these never drag (the tick, menus, other controls). */
const NO_DRAG = '[role="checkbox"], input, textarea, select, [data-no-drag]';

/** A press that may become a drag. */
interface Press {
  readonly id: number;
  readonly x: number;
  readonly y: number;
  readonly touch: boolean;
  timer: ReturnType<typeof setTimeout> | null;
}

interface DragEvents {
  readonly enabled: () => boolean;
  readonly start: (x: number, y: number) => void;
  readonly move: (x: number, y: number) => void;
  readonly drop: (x: number, y: number) => void;
  readonly cancel: () => void;
}

/** Listens on `element` for presses that become drags; returns the cleanup. */
const attach = (element: HTMLElement, events: DragEvents): (() => void) => {
  let press: Press | null = null;
  let active = false;
  /** A drag just ended: the click that follows isn't a click. */
  let swallowClick = false;
  /** The page-wide listeners of one press. */
  let listening: AbortController | null = null;

  const stop = (): void => {
    if (press?.timer !== null && press?.timer !== undefined) {
      clearTimeout(press.timer);
    }
    press = null;
    active = false;
    listening?.abort();
    listening = null;
  };
  const begin = (x: number, y: number): void => {
    active = true;
    globalThis.getSelection()?.removeAllRanges();
    events.start(x, y);
  };
  const onMove = (event: PointerEvent): void => {
    if (press === null || event.pointerId !== press.id) {
      return;
    }
    const dx = Math.abs(event.clientX - press.x);
    const dy = Math.abs(event.clientY - press.y);
    if (!active) {
      if (press.touch) {
        if (dx > TOUCH_SLOP || dy > TOUCH_SLOP) {
          // It scrolls.
          stop();
        }
        return;
      }
      if (dx <= DRAG_THRESHOLD && dy <= DRAG_THRESHOLD) {
        return;
      }
      begin(press.x, press.y);
    }
    events.move(event.clientX, event.clientY);
  };
  const onUp = (event: PointerEvent): void => {
    if (press === null || event.pointerId !== press.id) {
      return;
    }
    const wasActive = active;
    stop();
    if (wasActive) {
      swallowClick = true;
      setTimeout(() => {
        swallowClick = false;
      }, 0);
      events.drop(event.clientX, event.clientY);
    }
  };
  const onCancel = (): void => {
    const wasActive = active;
    stop();
    if (wasActive) {
      events.cancel();
    }
  };
  const onKey = (event: KeyboardEvent): void => {
    if (event.key === "Escape" && active) {
      event.preventDefault();
      event.stopPropagation();
      onCancel();
    }
  };
  const onDown = (event: PointerEvent): void => {
    const { target } = event;
    if (
      press !== null ||
      event.button !== 0 ||
      !events.enabled() ||
      (target instanceof Element && target.closest(NO_DRAG) !== null)
    ) {
      return;
    }
    const touch = event.pointerType === "touch";
    const current: Press = {
      id: event.pointerId,
      timer: null,
      touch,
      x: event.clientX,
      y: event.clientY,
    };
    press = current;
    if (touch) {
      current.timer = setTimeout(() => {
        current.timer = null;
        if (press === current) {
          begin(current.x, current.y);
        }
      }, LONG_PRESS_MS);
    }
    listening = new AbortController();
    const { signal } = listening;
    globalThis.addEventListener("pointermove", onMove, { signal });
    globalThis.addEventListener("pointerup", onUp, { signal });
    globalThis.addEventListener("pointercancel", onCancel, { signal });
    globalThis.addEventListener("keydown", onKey, { capture: true, signal });
  };
  const onTouchMove = (event: TouchEvent): void => {
    if (active) {
      event.preventDefault();
    }
  };
  const onContextMenu = (event: Event): void => {
    // A finger resting to drag: no context menu.
    if (press?.touch === true || active) {
      event.preventDefault();
    }
  };
  const onClick = (event: Event): void => {
    if (swallowClick) {
      event.preventDefault();
      event.stopPropagation();
    }
  };
  const own = new AbortController();
  const { signal } = own;
  element.addEventListener("pointerdown", onDown, { signal });
  element.addEventListener("touchmove", onTouchMove, {
    passive: false,
    signal,
  });
  element.addEventListener("contextmenu", onContextMenu, { signal });
  element.addEventListener("click", onClick, { capture: true, signal });
  return () => {
    stop();
    own.abort();
  };
};

/**
 * Makes `ref`'s element draggable. Listeners are native: a finger's
 * touchmove can only stop the page from scrolling when a non-passive
 * listener is there before the finger comes down.
 */
export const useDragSource = (
  ref: RefObject<HTMLElement | null>,
  handlers: DragHandlers
): void => {
  const isEnabled = useEffectEvent(() => handlers.enabled);
  const start = useEffectEvent((x: number, y: number) => {
    handlers.onStart(x, y);
  });
  const move = useEffectEvent((x: number, y: number) => {
    handlers.onMove(x, y);
  });
  const drop = useEffectEvent((x: number, y: number) => {
    handlers.onDrop(x, y);
  });
  const cancel = useEffectEvent(() => {
    handlers.onCancel();
  });

  useEffect(() => {
    const element = ref.current;
    return element === null
      ? undefined
      : attach(element, {
          cancel: () => {
            cancel();
          },
          drop: (x, y) => {
            drop(x, y);
          },
          enabled: () => isEnabled(),
          move: (x, y) => {
            move(x, y);
          },
          start: (x, y) => {
            start(x, y);
          },
        });
  }, [ref]);
};

// ---------------------------------------------------------------- shared state

/** What is being dragged, for the parts that react to it (the sidebar). */
export interface DragInfo {
  /** The list (`calendarKey`) under the pointer that takes the task. */
  readonly overList: string | null;
}

let info: DragInfo | null = null;
const listeners = new Set<() => void>();

export const setDragInfo = (next: DragInfo | null): void => {
  if (
    next?.overList === info?.overList &&
    (next === null) === (info === null)
  ) {
    return;
  }
  info = next;
  for (const listener of listeners) {
    listener();
  }
};

const subscribe = (listener: () => void): (() => void) => {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
};

/** The list a dragged task would drop on (null: none, or no drag). */
export const useDragOverList = (): string | null =>
  useSyncExternalStore(
    subscribe,
    () => info?.overList ?? null,
    () => null
  );

/** The list of the sidebar under a point (its `data-task-drop-list`). */
export const listAt = (x: number, y: number): string | null => {
  for (const element of document.elementsFromPoint(x, y)) {
    const found = element.closest<HTMLElement>("[data-task-drop-list]");
    if (found !== null) {
      return found.dataset.taskDropList ?? null;
    }
  }
  return null;
};

// ---------------------------------------------------------------- the ghost

const styles = stylex.create({
  ghost: {
    backgroundColor: colors.bg,
    borderColor: colors.line2,
    borderRadius: radius.md,
    borderStyle: "solid",
    borderWidth: "1px",
    boxShadow: shadow.sh2,
    color: colors.fg,
    fontSize: font.sizeSm,
    left: 0,
    maxWidth: "260px",
    overflow: "hidden",
    paddingBlock: "4px",
    paddingInline: "8px",
    pointerEvents: "none",
    position: "fixed",
    textOverflow: "ellipsis",
    top: 0,
    whiteSpace: "nowrap",
    zIndex: zIndex.tooltip,
  },
  hint: { color: colors.fg3, marginInlineStart: "6px" },
});

const ghostAt = (x: number, y: number): string =>
  `translate(${String(x + 14)}px, ${String(y + 10)}px)`;

const ghostVars = stylex.create({
  at: (x: number, y: number) => ({ transform: ghostAt(x, y) }),
});

/**
 * Moves a ghost to the pointer without React: its inline transform wins
 * over the one it was rendered with, and nothing else re-renders.
 */
export const placeGhost = (
  ghost: HTMLElement | null,
  x: number,
  y: number
): void => {
  if (ghost !== null) {
    ghost.style.transform = ghostAt(x, y);
  }
};

/**
 * Runs `run` with the last pointer position at most once per frame: a
 * pointer moves more often than the screen paints, and every move reads
 * the layout (elementsFromPoint, boxes).
 */
export const perFrame = (run: (x: number, y: number) => void) => {
  let frame: number | null = null;
  let x = 0;
  let y = 0;
  return {
    cancel: (): void => {
      if (frame !== null) {
        cancelAnimationFrame(frame);
        frame = null;
      }
    },
    move: (nextX: number, nextY: number): void => {
      x = nextX;
      y = nextY;
      frame ??= requestAnimationFrame(() => {
        frame = null;
        run(x, y);
      });
    },
  };
};

/** The title next to the pointer while dragging (and where it lands). */
export const DragGhost = ({
  hint,
  ref,
  title,
  x,
  y,
}: {
  readonly title: string;
  readonly hint: string | null;
  /** Where it starts; later moves go through `placeGhost` on `ref`. */
  readonly x: number;
  readonly y: number;
  readonly ref?: Ref<HTMLDivElement>;
}) =>
  // On the page itself: a dragged chip is faded, and so would its children.
  createPortal(
    <div
      aria-hidden
      ref={ref}
      {...stylex.props(styles.ghost, ghostVars.at(x, y))}
    >
      <bdi>{title}</bdi>
      {hint === null ? null : (
        <span {...stylex.props(styles.hint)}>{hint}</span>
      )}
    </div>,
    document.body
  );
