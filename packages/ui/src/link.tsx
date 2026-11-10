// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { mergeProps } from "@base-ui/react/merge-props";
import { useRender } from "@base-ui/react/use-render";
import * as stylex from "@stylexjs/stylex";
import type { ReactElement, ReactNode } from "react";

import { cls, shared } from "./internal";
import { colors, font } from "./tokens.stylex";

const styles = stylex.create({
  link: {
    borderRadius: "4px",
    color: colors.accText,
    cursor: "pointer",
    fontWeight: font.weightMedium,
    textDecorationColor: `color-mix(in srgb, ${colors.accText} 40%, transparent)`,
    textDecorationLine: { ":hover": "underline", default: "none" },
    textUnderlineOffset: "2px",
  },
  // A <button> that reads as a link: no box, inherits the text around it.
  button: {
    appearance: "none",
    backgroundColor: "transparent",
    borderWidth: 0,
    fontFamily: "inherit",
    fontSize: "inherit",
    lineHeight: "inherit",
    padding: 0,
    textAlign: "start",
  },
});

export interface TextLinkProps {
  readonly children: ReactNode;
  readonly href?: string;
  /** Router link element, e.g. `<Link to="/settings" />`. */
  readonly render?: ReactElement;
}

/** Inline brand-coloured link (underlined on hover). */
export const TextLink = ({ children, href, render }: TextLinkProps) =>
  useRender({
    defaultTagName: "a",
    props: mergeProps<"a">(
      {
        className: cls(styles.link, shared.focusRing),
        ...(href === undefined ? {} : { href }),
      },
      { children }
    ),
    ...(render === undefined ? {} : { render }),
  });

export interface LinkButtonProps {
  readonly children: ReactNode;
  readonly onClick: () => void;
}

/**
 * An action that sits inline in running text (e.g. "What are the risks?"
 * at the end of a hint): styled like {@link TextLink}, but a real button,
 * since it opens something instead of navigating.
 */
export const LinkButton = ({ children, onClick }: LinkButtonProps) => (
  <button
    onClick={onClick}
    type="button"
    className={cls(styles.link, styles.button, shared.focusRing)}
  >
    {children}
  </button>
);
