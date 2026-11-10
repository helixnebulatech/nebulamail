// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import * as stylex from "@stylexjs/stylex";
import { useState } from "react";
import type { ReactNode } from "react";

import { PortalContainerContext } from "./internal";

const styles = stylex.create({
  host: { display: "contents" },
});

export interface PortalScopeProps {
  readonly children: ReactNode;
}

/**
 * Menus, dialogs and tooltips opened inside portal here instead of into
 * <body>. Needed inside a native modal <dialog> (a phone drawer): the
 * rest of the page is inert while it's open, so popups in <body> can't be
 * clicked. Same theme as the page (unlike ThemeScope).
 */
export const PortalScope = ({ children }: PortalScopeProps) => {
  // State, not a ref: portals that mount right away must re-render once
  // the host exists.
  const [host, setHost] = useState<HTMLDivElement | null>(null);
  return (
    <>
      <PortalContainerContext value={host}>{children}</PortalContainerContext>
      <div ref={setHost} {...stylex.props(styles.host)} />
    </>
  );
};
