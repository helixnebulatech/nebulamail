// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Words of the Tasks app: views, priorities, due chips, reminders,
// estimated time, assignees.

import type { ReplyTexts } from "../../calendar/invitations";
import { formatDate, formatTime, parseLocalDate } from "../../calendar/time";
import { m } from "../../paraglide/messages.js";
import type { AssigneeStatus } from "../../tasks/assignment";
import type { TaskReplyTexts } from "../../tasks/assignment-api";
import type { DueLabel } from "../../tasks/due";
import type { TaskDue, TaskPriority } from "../../tasks/model";
import { dueInstant } from "../../tasks/model";
import type { TaskViewName } from "../../tasks/nav";
import { titleText } from "../calendar/event-text";
import { alertText } from "../calendar/format";

export const viewLabel = (view: TaskViewName): string => {
  switch (view) {
    case "today": {
      return m.tasks_view_today();
    }
    case "upcoming": {
      return m.tasks_view_upcoming();
    }
    case "all": {
      return m.tasks_view_all();
    }
    case "done": {
      return m.tasks_view_done();
    }
    default: {
      return view satisfies never;
    }
  }
};

export const PRIORITIES: readonly TaskPriority[] = [
  "none",
  "low",
  "medium",
  "high",
];

/** `!` + 0–3 sets these. */
export const PRIORITY_KEYS: Readonly<Record<TaskPriority, string>> = {
  high: "3",
  low: "1",
  medium: "2",
  none: "0",
};

export const priorityLabel = (priority: TaskPriority): string => {
  switch (priority) {
    case "none": {
      return m.tasks_priority_none();
    }
    case "low": {
      return m.tasks_priority_low();
    }
    case "medium": {
      return m.tasks_priority_medium();
    }
    case "high": {
      return m.tasks_priority_high();
    }
    default: {
      return priority satisfies never;
    }
  }
};

/** An estimated time: "45 min", "2 h", "1 h 30 min". */
export const estimateText = (minutes: number): string => {
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  if (hours === 0) {
    return m.tasks_estimate_m({ minutes: rest });
  }
  return rest === 0
    ? m.tasks_estimate_h({ hours })
    : m.tasks_estimate_hm({ hours, minutes: rest });
};

/** A day for headings: "Tomorrow", "Thursday 5 November". */
export const dayHeading = (
  day: string,
  today: string,
  locale: string
): string => {
  const date = parseLocalDate(day);
  const now = parseLocalDate(today);
  if (date === null || now === null) {
    return day;
  }
  const offset = date.compare(now);
  if (offset === 0) {
    return m.tasks_today();
  }
  if (offset === 1) {
    return m.tasks_tomorrow();
  }
  return formatDate(date, locale, {
    day: "numeric",
    month: "long",
    weekday: "long",
    ...(date.year === now.year ? {} : { year: "numeric" }),
  });
};

/** The chip's words: "Today 17:30", "Tomorrow", "Thu", "6 Nov 2027". */
export const dueText = (
  label: DueLabel,
  locale: string,
  zone: string
): string => {
  const date = parseLocalDate(label.day);
  let { day } = label;
  switch (label.when) {
    case "yesterday": {
      day = m.tasks_yesterday();
      break;
    }
    case "today": {
      day = m.tasks_today();
      break;
    }
    case "tomorrow": {
      day = m.tasks_tomorrow();
      break;
    }
    case "this-week": {
      day =
        date === null
          ? label.day
          : formatDate(date, locale, { weekday: "long" });
      break;
    }
    case "date": {
      day =
        date === null
          ? label.day
          : formatDate(date, locale, {
              day: "numeric",
              month: "short",
              ...(label.otherYear ? { year: "numeric" } : {}),
            });
      break;
    }
    default: {
      day = label.when satisfies never;
    }
  }
  return label.time === null
    ? day
    : `${day} ${formatTime(label.time, zone, locale)}`;
};

/** A task's reminder: "At the due time", "10 minutes before". */
export const taskAlertText = (minutes: number): string =>
  minutes === 0 ? m.tasks_alert_at_due() : alertText(minutes);

/** How far an assignee is with a task. */
export const assigneeStatusText = (status: AssigneeStatus): string => {
  switch (status) {
    case "needs-action": {
      return m.tasks_status_needs_action();
    }
    case "accepted": {
      return m.tasks_status_accepted();
    }
    case "tentative": {
      return m.tasks_status_tentative();
    }
    case "declined": {
      return m.tasks_status_declined();
    }
    case "delegated": {
      return m.tasks_status_delegated();
    }
    case "in-process": {
      return m.tasks_status_in_process();
    }
    case "completed": {
      return m.tasks_status_completed();
    }
    case "failed": {
      return m.tasks_status_failed();
    }
    default: {
      return status satisfies never;
    }
  }
};

/** The words of an assignee's REPLY email, in the user's language. */
export const taskReplyTexts: TaskReplyTexts = (answer, title, name) => {
  const clean = titleText(title);
  switch (answer) {
    case "accepted": {
      return {
        subject: m.tasks_reply_subject_accepted({ title: clean }),
        text: m.tasks_reply_text_accepted({ name, title: clean }),
      };
    }
    case "declined": {
      return {
        subject: m.tasks_reply_subject_declined({ title: clean }),
        text: m.tasks_reply_text_declined({ name, title: clean }),
      };
    }
    case "in-process": {
      return {
        subject: m.tasks_reply_subject_in_process({ title: clean }),
        text: m.tasks_reply_text_in_process({ name, title: clean }),
      };
    }
    case "completed": {
      return {
        subject: m.tasks_reply_subject_completed({ title: clean }),
        text: m.tasks_reply_text_completed({ name, title: clean }),
      };
    }
    default: {
      return answer satisfies never;
    }
  }
};

/** A due in full: "Friday, 6 November 2026" or with its time. */
export const fullDueText = (
  due: TaskDue,
  zone: string,
  locale: string
): string => {
  if (due.kind === "date") {
    const date = parseLocalDate(due.date);
    return date === null
      ? due.date
      : formatDate(date, locale, { dateStyle: "full" });
  }
  const at = dueInstant(due, zone);
  return at === null
    ? due.local
    : `${new Intl.DateTimeFormat(locale, { dateStyle: "full", timeZone: zone }).format(at)} ${formatTime(at, zone, locale)}`;
};

/** An assignee proposes another due (COUNTER). */
export const taskCounterTexts = (
  title: string,
  name: string,
  date: string
): ReplyTexts => {
  const clean = titleText(title);
  return {
    subject: m.tasks_counter_subject({ title: clean }),
    text: m.tasks_counter_text({ date, name, title: clean }),
  };
};

/** The organizer keeps the due (DECLINECOUNTER). */
export const taskDeclineCounterTexts = (
  title: string,
  name: string
): ReplyTexts => {
  const clean = titleText(title);
  return {
    subject: m.tasks_decline_counter_subject({ title: clean }),
    text: m.tasks_decline_counter_text({ name, title: clean }),
  };
};

/** Passing a task on: the REPLY to the organizer, the REQUEST to the delegate. */
export const taskDelegationTexts = (input: {
  readonly title: string;
  readonly name: string;
  readonly delegate: string;
  readonly organizer: string;
}): { readonly reply: ReplyTexts; readonly request: ReplyTexts } => {
  const title = titleText(input.title);
  return {
    reply: {
      subject: m.tasks_delegate_reply_subject({ title }),
      text: m.tasks_delegate_reply_text({
        delegate: input.delegate,
        name: input.name,
        title,
      }),
    },
    request: {
      subject: m.tasks_delegate_request_subject({ title }),
      text: m.tasks_delegate_request_text({
        name: input.name,
        organizer: input.organizer,
        title,
      }),
    },
  };
};
