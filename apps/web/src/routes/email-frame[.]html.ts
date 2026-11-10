// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The page an email is shown in when its images load straight from the
// sender (lib/frame-page.ts). Its own CSP, as a header and
// in the page, is the one mail-render gives such an email: images from
// `https:`, nothing that runs. Only this app may frame it (start.ts).

import { createFileRoute } from "@tanstack/react-router";

import { framePageResponse } from "../server/security-headers";

export const Route = createFileRoute("/email-frame.html")({
  server: {
    handlers: {
      GET: () => framePageResponse(),
    },
  },
});
