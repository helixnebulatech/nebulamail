// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { Avatar as BaseAvatar } from "@base-ui/react/avatar";
import * as stylex from "@stylexjs/stylex";

import { Mark } from "./brand";
import type { Icon } from "./button";
import { cls } from "./internal";
import { colors, font, tint } from "./tokens.stylex";

// First code point (not UTF-16 unit), so "Ñ" or "Ó" stay whole.
const firstChar = (text: string): string => {
  const code = text.codePointAt(0);
  return code === undefined ? "" : String.fromCodePoint(code);
};

/** Up to two initials from the capitalised words of a name ("Marta Iglesias" → "MI"). */
export const initials = (name: string): string => {
  const words = name
    .split(/\s+/u)
    .filter((word) => /^\p{Lu}/u.test(word))
    .slice(0, 2)
    .map((word) => firstChar(word));
  const joined = words.join("");
  return joined === "" ? (firstChar(name.trim()) || "?").toUpperCase() : joined;
};

/** Deterministic hue (0–359) for a string, e.g. the sender's domain. */
export const hueFor = (key: string): number => {
  // Polynomial string hash: tiny, stable across sessions and devices.
  let hash = 7;
  for (const char of key.toLowerCase()) {
    hash = (hash * 31 + (char.codePointAt(0) ?? 0)) % 1_000_000_007;
  }
  return hash % 360;
};

const styles = stylex.create({
  dot: {
    backgroundColor: colors.acc,
    borderColor: colors.bg,
    borderRadius: "50%",
    borderStyle: "solid",
    borderWidth: "2px",
    height: "10px",
    insetInlineEnd: "-2px",
    position: "absolute",
    top: "-2px",
    width: "10px",
  },
  fallback: {
    alignItems: "center",
    display: "inline-grid",
    height: "100%",
    justifyContent: "center",
    width: "100%",
  },
  hue: (hue: number) => ({
    backgroundColor: `oklch(${tint.avatarBg} ${hue})`,
    color: `oklch(${tint.avatarFg} ${hue})`,
  }),
  image: { height: "100%", objectFit: "cover", width: "100%" },
  official: { backgroundColor: "#14152A" },
  root: {
    alignItems: "center",
    display: "inline-grid",
    flexShrink: 0,
    fontFamily: font.sans,
    fontWeight: 650,
    justifyContent: "center",
    letterSpacing: "-0.02em",
    lineHeight: 1,
    overflow: "hidden",
    userSelect: "none",
    verticalAlign: "middle",
  },
  size: (size: number) => ({
    borderRadius: `${Math.round(size * 0.34)}px`,
    fontSize: `${Math.round(size * 0.38)}px`,
    height: `${size}px`,
    width: `${size}px`,
  }),
  wrap: { display: "inline-flex", flexShrink: 0, position: "relative" },
});

export interface AvatarProps {
  /** Display name: initials and accessible name. */
  readonly name: string;
  /** Colour key (default: name). Use the sender's domain for stable colours. */
  readonly colorKey?: string;
  /** Image URL. Only proxied/local URLs (CSP img-src 'self' data: blob:). */
  readonly src?: string;
  readonly size?: number;
  /** Our own notices: night tile with the mark. */
  readonly official?: boolean;
  /** Accent unread dot. */
  readonly unread?: boolean;
  /** Decorative (name shown next to it). Default true. */
  readonly decorative?: boolean;
  /** Instead of the initials (a group: several people). */
  readonly icon?: Icon;
}

/**
 * Sender avatar: initials on a soft colour derived from the name or domain,
 * or a proxied image. No remote logos by default (they leak the IP).
 */
export const Avatar = ({
  name,
  colorKey,
  src,
  size = 32,
  official = false,
  unread = false,
  decorative = true,
  icon: IconShown,
}: AvatarProps) => {
  const a11y = decorative
    ? ({ "aria-hidden": true } as const)
    : ({ "aria-label": name, role: "img" } as const);
  // Dynamic styles: StyleX returns a class plus custom properties in `style`.
  const rootProps = stylex.props(
    styles.root,
    styles.hue(hueFor(colorKey ?? name)),
    styles.size(size)
  );
  const avatar = official ? (
    <span
      {...a11y}
      {...stylex.props(styles.root, styles.size(size), styles.official)}
    >
      <Mark label={null} size={Math.round(size * 0.78)} tone="onDark" />
    </span>
  ) : (
    <BaseAvatar.Root {...a11y} {...rootProps}>
      {src === undefined ? null : (
        <BaseAvatar.Image alt="" className={cls(styles.image)} src={src} />
      )}
      <BaseAvatar.Fallback className={cls(styles.fallback)}>
        {IconShown === undefined ? (
          initials(name)
        ) : (
          <IconShown aria-hidden size={Math.round(size * 0.55)} />
        )}
      </BaseAvatar.Fallback>
    </BaseAvatar.Root>
  );
  return unread ? (
    <span {...stylex.props(styles.wrap)}>
      {avatar}
      <span {...stylex.props(styles.dot)} />
    </span>
  ) : (
    avatar
  );
};
