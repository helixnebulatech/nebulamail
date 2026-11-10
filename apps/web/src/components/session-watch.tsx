// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";

import { watchSession } from "../lib/session-expired";

/**
 * Session gone (refresh token expired or revoked): back to the login, from
 * any page. Mounted once, in the root route.
 */
export const SessionWatch = (): null => {
  const queryClient = useQueryClient();
  useEffect(() => watchSession(queryClient), [queryClient]);
  return null;
};
