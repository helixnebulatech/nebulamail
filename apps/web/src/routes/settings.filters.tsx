// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { createFileRoute } from "@tanstack/react-router";
import { z } from "zod";

import { FiltersSection } from "../components/settings/filters-section";

// `from`: an email id, for "Create filter from this email" (reader menu).
const Search = z.object({
  from: z
    .string()
    .max(255)
    .regex(/^[\w-]+$/u)
    .optional()
    .catch(undefined),
});

export const Route = createFileRoute("/settings/filters")({
  component: FiltersSection,
  validateSearch: Search,
});
