// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { createFileRoute } from "@tanstack/react-router";

import { VacationSection } from "../components/settings/vacation-section";

export const Route = createFileRoute("/settings/vacation")({
  component: VacationSection,
});
