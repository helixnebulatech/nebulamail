// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { useRouteContext } from "@tanstack/react-router";

import type { ReadingPane } from "../../lib/preferences";

/**
 * Where emails open (Settings → Open emails): "side" next to the list, or
 * "full" in place of it, at any width. Narrow screens always show one
 * panel at a time.
 */
export const useReadingPane = (): ReadingPane =>
  useRouteContext({ from: "__root__" }).preferences.pane;
