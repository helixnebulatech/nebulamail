// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Keyboard shortcuts (keyboard-first, Superhuman-like).
//
// Specs: "c", "e", "#", "/", "Escape", "mod+k" (⌘ on Apple, Ctrl elsewhere),
// "shift+u", and sequences like "g i" (press g, then i within a second).
// Single-key shortcuts never fire while typing in a field; ones with a
// modifier (mod+k, mod+Enter) and Escape do.

import { useEffect, useEffectEvent } from "react";

export interface KeyChord {
  readonly key: string;
  readonly mod: boolean;
  readonly shift: boolean;
  readonly alt: boolean;
}

export type Shortcut = readonly KeyChord[];

/** The subset of KeyboardEvent we need (testable without a DOM). */
export interface KeyInput {
  readonly key: string;
  readonly metaKey: boolean;
  readonly ctrlKey: boolean;
  readonly shiftKey: boolean;
  readonly altKey: boolean;
}

const NAMED: Readonly<Record<string, string>> = {
  enter: "enter",
  esc: "escape",
  escape: "escape",
  space: " ",
};

export const parseShortcut = (spec: string): Shortcut =>
  spec
    .trim()
    .split(/\s+/u)
    .map((part) => {
      const pieces = part.toLowerCase().split("+");
      const key = pieces.at(-1) ?? "";
      const modifiers = new Set(pieces.slice(0, -1));
      return {
        alt: modifiers.has("alt"),
        key: NAMED[key] ?? key,
        mod: modifiers.has("mod"),
        shift: modifiers.has("shift"),
      };
    });

/**
 * `mod` is ⌘ on Apple platforms and Ctrl elsewhere. Shift is ignored for
 * symbols that need it on some layouts ("#", "?", "/"), whose `key` already
 * says what was typed.
 */
export const chordMatches = (
  chord: KeyChord,
  input: KeyInput,
  apple: boolean
): boolean => {
  const mod = apple ? input.metaKey : input.ctrlKey;
  const other = apple ? input.ctrlKey : input.metaKey;
  const key = input.key.toLowerCase();
  const symbol = key.length === 1 && !/[\p{L}\p{N}]/u.test(key);
  return (
    key === chord.key &&
    mod === chord.mod &&
    !other &&
    input.altKey === chord.alt &&
    (symbol || input.shiftKey === chord.shift)
  );
};

/** Whether a shortcut may fire while the user types in a field. */
export const firesWhileTyping = (shortcut: Shortcut): boolean =>
  shortcut.length === 1 &&
  shortcut[0] !== undefined &&
  (shortcut[0].mod || shortcut[0].key === "escape");

export const isTypingTarget = (target: EventTarget | null): boolean => {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  return (
    target.isContentEditable ||
    ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName)
  );
};

export const isApplePlatform = (): boolean =>
  typeof navigator !== "undefined" &&
  /mac|iphone|ipad|ipod/iu.test(navigator.userAgent);

const SEQUENCE_TIMEOUT_MS = 1000;

interface Binding {
  readonly shortcut: Shortcut;
  readonly handler: (event: KeyboardEvent) => void;
  readonly enabled: () => boolean;
}

const bindings = new Set<Binding>();
let pending: { readonly keys: KeyInput[]; readonly at: number } | null = null;
let listening = false;

/** Does `history` (oldest first) end with the whole shortcut? */
export const sequenceMatches = (
  shortcut: Shortcut,
  history: readonly KeyInput[],
  apple: boolean
): boolean => {
  if (history.length < shortcut.length) {
    return false;
  }
  const tail = history.slice(-shortcut.length);
  return shortcut.every((chord, index) => {
    const input = tail[index];
    return input !== undefined && chordMatches(chord, input, apple);
  });
};

const onKeyDown = (event: KeyboardEvent): void => {
  if (event.defaultPrevented || event.isComposing) {
    return;
  }
  const apple = isApplePlatform();
  const now = Date.now();
  const previous =
    pending !== null && now - pending.at < SEQUENCE_TIMEOUT_MS
      ? pending.keys
      : [];
  const history = [...previous, event].slice(-3);
  const typing = isTypingTarget(event.target);
  // Longest first: "g c" (go to calendar) wins over "c" (compose) when the
  // c ends that sequence; a lone c still composes.
  const byLength = [...bindings].toSorted(
    (a, b) => b.shortcut.length - a.shortcut.length
  );
  for (const binding of byLength) {
    if (
      binding.enabled() &&
      (!typing || firesWhileTyping(binding.shortcut)) &&
      sequenceMatches(binding.shortcut, history, apple)
    ) {
      event.preventDefault();
      pending = null;
      binding.handler(event);
      return;
    }
  }
  pending = typing ? null : { at: now, keys: history };
};

/**
 * Registers a shortcut while the component is mounted. The handler can
 * change on every render; the binding stays the same.
 */
export const useShortcut = (
  spec: string,
  handler: (event: KeyboardEvent) => void,
  { enabled = true }: { readonly enabled?: boolean } = {}
): void => {
  const run = useEffectEvent((event: KeyboardEvent) => {
    handler(event);
  });
  const isEnabled = useEffectEvent(() => enabled);

  useEffect(() => {
    if (!listening) {
      globalThis.addEventListener("keydown", onKeyDown);
      listening = true;
    }
    const binding: Binding = {
      enabled: () => isEnabled(),
      handler: (event) => {
        run(event);
      },
      shortcut: parseShortcut(spec),
    };
    bindings.add(binding);
    return () => {
      bindings.delete(binding);
    };
  }, [spec]);
};

const keyLabel = (key: string): string => {
  switch (key) {
    case " ": {
      return "Space";
    }
    case "escape": {
      return "Esc";
    }
    case "enter": {
      return "↵";
    }
    default: {
      return key.toUpperCase();
    }
  }
};

/** How to show a shortcut to the user ("⌘K" on Apple, "Ctrl K" elsewhere). */
export const describeShortcut = (spec: string, apple: boolean): string[] =>
  parseShortcut(spec).map((chord) => {
    const parts: string[] = [];
    if (chord.mod) {
      parts.push(apple ? "⌘" : "Ctrl");
    }
    if (chord.shift) {
      parts.push(apple ? "⇧" : "Shift");
    }
    if (chord.alt) {
      parts.push(apple ? "⌥" : "Alt");
    }
    parts.push(keyLabel(chord.key));
    return parts.join(apple ? "" : " ");
  });

/**
 * The keys of a shortcut one by one, for `<Shortcut keys>`: "mod+k" →
 * ["⌘", "K"] on Apple, ["Ctrl", "K"] elsewhere; "g i" → ["G", "I"].
 */
export const shortcutKeys = (spec: string, apple: boolean): string[] =>
  parseShortcut(spec).flatMap((chord) => [
    ...(chord.mod ? [apple ? "⌘" : "Ctrl"] : []),
    ...(chord.shift ? [apple ? "⇧" : "Shift"] : []),
    ...(chord.alt ? [apple ? "⌥" : "Alt"] : []),
    keyLabel(chord.key),
  ]);
