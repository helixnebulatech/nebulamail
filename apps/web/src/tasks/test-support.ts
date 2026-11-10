// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// A Task for unit tests (only what a test sets differs).

import { AccountId, CalendarEventId } from "@nebulamail/jmap-client";

import type { Task } from "./model";

export const makeTask = (id: string, extra: Partial<Task> = {}): Task => ({
  accountId: AccountId.parse("c"),
  alerts: [],
  baseId: CalendarEventId.parse(id),
  calendarId: null,
  completed: false,
  completedAt: null,
  created: null,
  due: null,
  dueAt: null,
  estimatedMinutes: null,
  id: CalendarEventId.parse(id),
  keywords: [],
  links: [],
  listKey: "c/l1",
  notes: null,
  parentUid: null,
  priority: "none",
  progress: "needs-action",
  recurrenceId: null,
  recurring: false,
  sortOrder: null,
  title: id,
  uid: `${id}@uid`,
  updated: null,
  ...extra,
});
