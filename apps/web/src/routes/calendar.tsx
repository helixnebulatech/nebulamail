// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { createFileRoute } from "@tanstack/react-router";
import { z } from "zod";

import { VIEWS } from "../calendar/layout";
import { CalendarApp } from "../components/calendar/calendar-app";
import { requireAuth } from "../lib/auth-guard";
import { prefetchCalendar } from "../lib/route-prefetch";
import { m } from "../paraglide/messages.js";

const Id = z.string().regex(/^[\w-]{1,255}$/u);

const LocalStart = z.string().regex(/^\d{4}-\d{2}-\d{2}(?:T\d{2}:\d{2})?$/u);

// `view` and `date` say what is shown; `event` + `series` the open event
// (occurrence id and its series) and `account` its JMAP account (absent:
// the user's own; set for calendars shared by others), `edit` its editor,
// `new` a new event's start ("2026-10-26T10:00" or a date for all-day) and
// `end` its end (a range dragged in the week view). Bad values are dropped.
const Search = z.object({
  account: Id.optional().catch(undefined),
  date: z
    .string()
    .regex(/^\d{4}-\d{2}-\d{2}$/u)
    .optional()
    .catch(undefined),
  edit: z.literal("1").optional().catch(undefined),
  end: LocalStart.optional().catch(undefined),
  event: Id.optional().catch(undefined),
  new: LocalStart.optional().catch(undefined),
  series: Id.optional().catch(undefined),
  view: z.enum(VIEWS).optional().catch(undefined),
});

export type CalendarSearch = z.infer<typeof Search>;

export const Route = createFileRoute("/calendar")({
  // Signed in? Once per page load, shared by every section (auth-guard.ts).
  beforeLoad: async ({ location }) => await requireAuth(location),
  // Its calendars and the user's addresses load with the route.
  loader: ({ context: { queryClient } }) => {
    prefetchCalendar(queryClient);
  },
  component: CalendarApp,
  // In the server render too: the tab has its name from the first paint.
  head: () => ({
    meta: [{ title: `${m.cal_title_app()} · ${m.app_name()}` }],
  }),
  validateSearch: Search,
});
