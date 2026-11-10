// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { CSPProvider } from "@base-ui/react/csp-provider";
import { Tooltip as BaseTooltip } from "@base-ui/react/tooltip";
import type { ReactNode } from "react";

import { ToastProvider } from "./toast";

export interface UIProviderProps {
  /** The per-request CSP nonce (for any inline <script> Base UI renders). */
  readonly nonce?: string;
  readonly children: ReactNode;
}

/**
 * Put once at the root (inside <body>). Sets the CSP configuration for Base
 * UI (no inline <style> elements at all: our StyleX CSS covers them; the
 * nonce goes to opt-in inline scripts), shares tooltip delays and provides
 * toasts (`useToast`).
 */
export const UIProvider = ({ nonce, children }: UIProviderProps) => (
  <CSPProvider disableStyleElements {...(nonce === undefined ? {} : { nonce })}>
    <BaseTooltip.Provider delay={400}>
      <ToastProvider>{children}</ToastProvider>
    </BaseTooltip.Provider>
  </CSPProvider>
);
