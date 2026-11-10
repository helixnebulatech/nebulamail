// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { createFileRoute, redirect } from "@tanstack/react-router";

export const Route = createFileRoute("/mail/")({
  beforeLoad: () => {
    throw redirect({ params: { mailbox: "inbox" }, to: "/mail/$mailbox" });
  },
});
