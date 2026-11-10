// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { createFileRoute } from "@tanstack/react-router";
import { z } from "zod";

import { TasksApp } from "../components/tasks/tasks-app";
import { requireAuth } from "../lib/auth-guard";
import { prefetchTasks } from "../lib/route-prefetch";
import { m } from "../paraglide/messages.js";
import { TASK_VIEWS } from "../tasks/nav";

/** An id, or `<account>.<id>` for a shared account (tasks/links.ts). */
const Ref = z.string().regex(/^[\w-]{1,255}(?:\.[\w-]{1,255})?$/u);

// `view` (Today by default) or `list` say what is listed; `task` the open
// task (an occurrence's id for a repeating one). Bad values are dropped.
const Search = z.object({
  list: Ref.optional().catch(undefined),
  task: Ref.optional().catch(undefined),
  view: z.enum(TASK_VIEWS).optional().catch(undefined),
});

export type TasksSearch = z.infer<typeof Search>;

export const Route = createFileRoute("/tasks")({
  // Signed in? Once per page load, shared by every section (auth-guard.ts).
  beforeLoad: async ({ location }) => await requireAuth(location),
  // The tasks load with the route, not after it mounts.
  loader: ({ context: { queryClient } }) => {
    prefetchTasks(queryClient);
  },
  component: TasksApp,
  // In the server render too: the tab has its name from the first paint.
  head: () => ({
    meta: [{ title: `${m.tasks_title_app()} · ${m.app_name()}` }],
  }),
  validateSearch: Search,
});
