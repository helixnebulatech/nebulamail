// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import * as stylex from "@stylexjs/stylex";
import type { ReactNode } from "react";

import { colors, font } from "./tokens.stylex";

const styles = stylex.create({
  action: { marginTop: "14px" },
  body: {
    color: colors.fg2,
    fontSize: font.sizeMd,
    lineHeight: font.lineHeight,
    margin: 0,
    marginInline: "auto",
    maxWidth: "340px",
    textWrap: "balance",
  },
  // A detail pane's resting state ("Pick a message", "Pick a contact"):
  // fills the pane and sits a little above the middle (about 40% down),
  // the same in every module.
  pane: {
    flexGrow: 1,
    justifyContent: "center",
    minHeight: 0,
    paddingBottom: "18vh",
  },
  root: {
    alignItems: "center",
    display: "flex",
    flexDirection: "column",
    fontFamily: font.sans,
    paddingBlock: "26px",
    paddingInline: "20px",
    textAlign: "center",
  },
  title: {
    color: colors.fg,
    fontSize: "16px",
    fontWeight: font.weightBold,
    letterSpacing: "-0.015em",
    marginBlock: "12px 4px",
    marginInline: 0,
    textWrap: "balance",
  },
});

export interface EmptyStateProps {
  readonly title: ReactNode;
  /** One or two calm sentences. */
  readonly children?: ReactNode;
  /** e.g. `<IllustrationInboxZero />`. */
  readonly illustration?: ReactNode;
  readonly action?: ReactNode;
  readonly headingLevel?: 2 | 3 | 4;
  /**
   * Fills a detail pane, centred a little above the middle (pass a compact
   * illustration).
   */
  readonly pane?: boolean;
}

/** Calm empty state: illustration, title, one line, optional action. */
export const EmptyState = ({
  title,
  children,
  illustration,
  action,
  headingLevel = 2,
  pane = false,
}: EmptyStateProps) => {
  const Heading = `h${headingLevel}` as const;
  return (
    <div {...stylex.props(styles.root, pane ? styles.pane : null)}>
      {illustration}
      <Heading {...stylex.props(styles.title)}>{title}</Heading>
      {children === undefined ? null : (
        <p {...stylex.props(styles.body)}>{children}</p>
      )}
      {action === undefined ? null : (
        <div {...stylex.props(styles.action)}>{action}</div>
      )}
    </div>
  );
};
