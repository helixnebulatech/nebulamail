// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import * as stylex from "@stylexjs/stylex";
import { useState } from "react";
import type { ReactNode } from "react";

import { PortalContainerContext } from "./internal";
import { themeProps } from "./themes";
import type { Density, ThemeMode } from "./themes";

const styles = stylex.create({
  host: { display: "contents" },
});

export interface ThemeScopeProps {
  readonly mode: ThemeMode;
  readonly density?: Density;
  readonly children: ReactNode;
}

/**
 * Applies a theme to a subtree (e.g. a dark preview inside a light page).
 * Menus, dialogs, tooltips and toasts opened inside portal into the scope,
 * so they use its theme too. The page theme itself goes on <html> with
 * `themeProps` (server-rendered, no flash).
 */
export const ThemeScope = ({
  mode,
  density = "compact",
  children,
}: ThemeScopeProps) => {
  // State, not a ref: portals that mount right away (the toast viewport)
  // must re-render once the host exists.
  const [host, setHost] = useState<HTMLDivElement | null>(null);
  return (
    <div {...themeProps(mode, density)}>
      <PortalContainerContext value={host}>{children}</PortalContainerContext>
      <div ref={setHost} {...stylex.props(styles.host)} />
    </div>
  );
};
