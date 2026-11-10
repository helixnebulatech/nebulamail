// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { createFileRoute, redirect } from "@tanstack/react-router";

// The app starts in the inbox; /mail checks the session.
export const Route = createFileRoute("/")({
  beforeLoad: () => {
    throw redirect({ params: { mailbox: "inbox" }, to: "/mail/$mailbox" });
  },
});
