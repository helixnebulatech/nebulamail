// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { Separator as BaseSeparator } from "@base-ui/react/separator";
import * as stylex from "@stylexjs/stylex";

import { cls } from "./internal";
import { colors } from "./tokens.stylex";

const styles = stylex.create({
  horizontal: { backgroundColor: colors.line, height: "1px", width: "100%" },
  vertical: {
    alignSelf: "center",
    backgroundColor: colors.line,
    flexShrink: 0,
    height: "18px",
    marginInline: "6px",
    width: "1px",
  },
});

export interface SeparatorProps {
  readonly orientation?: "horizontal" | "vertical";
}

/** Hairline divider. Vertical: 18 px, for toolbars. */
export const Separator = ({ orientation = "horizontal" }: SeparatorProps) => (
  <BaseSeparator
    className={cls(styles[orientation])}
    orientation={orientation}
  />
);
