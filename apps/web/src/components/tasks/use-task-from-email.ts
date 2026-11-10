// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// "Create task" from an email (reader menu, ⌘K, Shift+T):
// the subject as the title, "From: … · date" as notes, and two links:
// `mid:<message-id>` (RFC 2392) and this app's URL of the email, which the
// task panel opens here. The task goes to the default list; the toast
// opens it.

import type { AccountId, CalendarId } from "@nebulamail/jmap-client";
import { useToast } from "@nebulamail/ui";
import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";

import { useTimeZone } from "../../calendar/hooks";
import { formatFullDate } from "../../lib/format";
import type { EmailDetail } from "../../mail/api";
import { m } from "../../paraglide/messages.js";
import { getLocale } from "../../paraglide/runtime.js";
import {
  createTask,
  defaultTaskListKey,
  rememberTaskList,
  savedTaskLists,
} from "../../tasks/api";
import { refreshTasks, useTasksAvailable } from "../../tasks/hooks";
import { refParam, taskFromEmail } from "../../tasks/links";
import { titleText } from "../calendar/event-text";
import { useOwnTasksAccount } from "./use-tasks-nav";

export interface TaskFromEmail {
  /** The server keeps tasks (else the action isn't offered). */
  readonly available: boolean;
  readonly create: (email: EmailDetail, folder: string) => Promise<void>;
}

export const useTaskFromEmail = (): TaskFromEmail => {
  const available = useTasksAvailable() === true;
  const toast = useToast();
  const navigate = useNavigate();
  const zone = useTimeZone();
  const own = useOwnTasksAccount();
  const client = useQueryClient();
  return {
    available,
    create: async (email, folder) => {
      const locale = getLocale();
      const input = taskFromEmail(
        {
          emailId: email.id,
          folder,
          from: email.from[0] ?? null,
          messageIds: email.messageId,
          receivedAt: Date.parse(email.receivedAt),
          subject: email.subject,
        },
        globalThis.location.origin,
        {
          formatDate: (ms) =>
            formatFullDate(new Date(ms).toISOString(), locale),
          linkTitle: (subject) => m.tasks_email_link({ subject }),
          noSubject: m.tasks_no_subject(),
          notes: (sender, date) => m.tasks_from_email_notes({ date, sender }),
        }
      );
      try {
        const listKey = await defaultTaskListKey();
        const [accountId, calendarId] = listKey?.split("/") ?? [];
        if (
          listKey === null ||
          accountId === undefined ||
          calendarId === undefined
        ) {
          toast.show({ title: m.tasks_no_list_for_email(), tone: "danger" });
          return;
        }
        const created = await createTask(
          {
            // The key's parts are the server's own ids.
            // oxlint-disable-next-line typescript/no-unsafe-type-assertion
            accountId: accountId as AccountId,
            // oxlint-disable-next-line typescript/no-unsafe-type-assertion
            calendarId: calendarId as CalendarId,
            links: input.links,
            notes: input.notes,
            title: input.title,
          },
          zone
        );
        if (!savedTaskLists().includes(listKey)) {
          rememberTaskList(listKey, true);
        }
        void refreshTasks(client);
        toast.show({
          action: {
            label: m.tasks_open_task(),
            onClick: () => {
              void navigate({
                search: { task: refParam(accountId, created.id, own) },
                to: "/tasks",
              });
            },
          },
          title: m.tasks_created_from_email({ title: titleText(input.title) }),
        });
      } catch {
        toast.show({ title: m.tasks_error_save(), tone: "danger" });
      }
    },
  };
};
