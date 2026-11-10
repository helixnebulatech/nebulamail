// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { createFileRoute } from "@tanstack/react-router";

import { ReaderPlaceholder } from "../components/mail/reader-placeholder";

export const Route = createFileRoute("/mail/$mailbox/")({
  component: ReaderPlaceholder,
});
