// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// RFC 9116 contacts (src/server/security-txt.ts). A route instead of a file
// in public/: the Canonical field needs this instance's APP_URL, and the
// global middleware adds the usual security headers.

import { createFileRoute } from "@tanstack/react-router";

import { ENV } from "../../env";
import { securityTxtResponse } from "../../server/security-txt";

export const Route = createFileRoute("/.well-known/security.txt")({
  server: {
    handlers: {
      GET: () => securityTxtResponse(ENV.APP_URL),
    },
  },
});
