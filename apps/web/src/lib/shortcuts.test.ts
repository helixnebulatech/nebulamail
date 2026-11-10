// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import type { KeyInput } from "./shortcuts";
import {
  chordMatches,
  describeShortcut,
  firesWhileTyping,
  parseShortcut,
  sequenceMatches,
  shortcutKeys,
} from "./shortcuts";

const key = (
  value: string,
  modifiers: Partial<Omit<KeyInput, "key">> = {}
): KeyInput => ({
  altKey: false,
  ctrlKey: false,
  key: value,
  metaKey: false,
  shiftKey: false,
  ...modifiers,
});

const first = (spec: string) => {
  const [chord] = parseShortcut(spec);
  if (chord === undefined) {
    throw new Error("empty shortcut");
  }
  return chord;
};

describe(parseShortcut, () => {
  it("parses modifiers, named keys and sequences", () => {
    expect(parseShortcut("mod+k g i Escape")).toStrictEqual([
      { alt: false, key: "k", mod: true, shift: false },
      { alt: false, key: "g", mod: false, shift: false },
      { alt: false, key: "i", mod: false, shift: false },
      { alt: false, key: "escape", mod: false, shift: false },
    ]);
  });
});

describe(chordMatches, () => {
  it("maps mod to ⌘ on Apple and Ctrl elsewhere", () => {
    const chord = first("mod+k");

    expect([
      chordMatches(chord, key("k", { metaKey: true }), true),
      chordMatches(chord, key("k", { ctrlKey: true }), false),
      chordMatches(chord, key("k", { ctrlKey: true }), true),
      chordMatches(chord, key("k"), false),
    ]).toStrictEqual([true, true, false, false]);
  });

  it("requires the exact modifiers for letters", () => {
    const chord = first("e");

    expect([
      chordMatches(chord, key("e"), false),
      chordMatches(chord, key("E", { shiftKey: true }), false),
      chordMatches(chord, key("e", { altKey: true }), false),
      chordMatches(first("shift+u"), key("U", { shiftKey: true }), false),
    ]).toStrictEqual([true, false, false, true]);
  });

  it("ignores shift for symbols that need it on some layouts", () => {
    expect(
      chordMatches(first("#"), key("#", { shiftKey: true }), false)
    ).toBeTruthy();
  });
});

describe(sequenceMatches, () => {
  it("matches the end of the key history", () => {
    const shortcut = parseShortcut("g i");

    expect([
      sequenceMatches(shortcut, [key("g"), key("i")], false),
      sequenceMatches(shortcut, [key("x"), key("g"), key("i")], false),
      sequenceMatches(shortcut, [key("i")], false),
      sequenceMatches(shortcut, [key("g"), key("x")], false),
    ]).toStrictEqual([true, true, false, false]);
  });
});

describe(firesWhileTyping, () => {
  it("only lets modified keys and Escape through while typing", () => {
    expect(
      ["mod+k", "Escape", "c", "g i", "mod+enter"].map((spec) =>
        firesWhileTyping(parseShortcut(spec))
      )
    ).toStrictEqual([true, true, false, false, true]);
  });
});

describe(describeShortcut, () => {
  it("shows ⌘K on Apple and Ctrl K elsewhere", () => {
    expect([
      describeShortcut("mod+k", true),
      describeShortcut("mod+k", false),
      describeShortcut("g i", false),
      describeShortcut("mod+enter", true),
    ]).toStrictEqual([["⌘K"], ["Ctrl K"], ["G", "I"], ["⌘↵"]]);
  });
});

describe(shortcutKeys, () => {
  it("lists the keys one by one, per platform", () => {
    expect(shortcutKeys("mod+k", true)).toStrictEqual(["⌘", "K"]);
    expect(shortcutKeys("mod+enter", false)).toStrictEqual(["Ctrl", "↵"]);
    expect(shortcutKeys("g i", false)).toStrictEqual(["G", "I"]);
    expect(shortcutKeys("shift+u", true)).toStrictEqual(["⇧", "U"]);
  });
});
