// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { useEffect, useRef } from "react";
import type { RefObject } from "react";

/**
 * A native modal `<dialog>` (focus trap, Esc, inert page behind) shown
 * while `open`: put the returned ref on it. Opening a modal dialog is only
 * possible through its DOM method (`showModal`, not an attribute), so an
 * effect keeps it in step with the state.
 */
export const useModalDialog = (
  open: boolean
): RefObject<HTMLDialogElement | null> => {
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const element = dialog.current;
    if (element === null) {
      return;
    }
    if (open && !element.open) {
      element.showModal();
    } else if (!open && element.open) {
      element.close();
    }
  }, [open]);
  return dialog;
};
