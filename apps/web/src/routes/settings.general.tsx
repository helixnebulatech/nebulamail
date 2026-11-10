// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { createFileRoute } from "@tanstack/react-router";

import { GeneralSection } from "../components/settings/general-section";

export const Route = createFileRoute("/settings/general")({
  component: GeneralSection,
});
