// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// "Assign to" in the task panel: people
// from the address book or a typed address, as chips with how far each
// one is (no answer, accepted, in progress, done, declined…). Adding or
// removing someone saves at once and the server or the bridge mails them
// (REQUEST / CANCEL). On a task someone assigned to the user: who did,
// and Accept / Decline (a REPLY the app mails), or propose another due /
// pass it on (task-more-options.tsx). Hidden where the server can't
// schedule (no calendars or no sending) and on shared lists.

import { cleanDisplayText } from "@nebulamail/mail-render";
import { Avatar, Badge, Button, IconButton } from "@nebulamail/ui";
import type { BadgeTone } from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { Check, Users, X } from "lucide-react";
import { useId, useState } from "react";

import { useOwnCalendarAddresses } from "../../calendar/hooks";
import { m } from "../../paraglide/messages.js";
import type {
  AssigneeInput,
  AssigneeStatus,
  Assignment,
  TaskAssignee,
} from "../../tasks/assignment";
import { assignmentOf, mayDelegate } from "../../tasks/assignment";
import { useAssignableAccount } from "../../tasks/hooks";
import { repeats } from "../../tasks/itip-series";
import type { RawTask, Task } from "../../tasks/model";
import { shared } from "../mail/styles";
import { AssignInput } from "./assign-input";
import { assigneeStatusText } from "./task-format";
import {
  DelegateOption,
  ProposeDueOption,
  TaskMoreOptions,
} from "./task-more-options";
import type { TaskActions } from "./use-task-actions";

const styles = stylex.create({
  chip: {
    alignItems: "center",
    backgroundColor: colors.hover,
    borderRadius: radius.pill,
    columnGap: "6px",
    display: "inline-flex",
    fontSize: font.sizeSm,
    maxWidth: "100%",
    minHeight: "28px",
    paddingInlineEnd: "2px",
    paddingInlineStart: "4px",
  },
  chips: {
    display: "flex",
    flexWrap: "wrap",
    gap: "6px",
    listStyle: "none",
    margin: 0,
    padding: 0,
  },
  group: { display: "flex", flexDirection: "column", rowGap: "8px" },
  hint: { color: colors.fg3, fontSize: font.sizeSm, margin: 0 },
  label: {
    alignItems: "center",
    color: colors.fg2,
    columnGap: "6px",
    display: "flex",
    fontSize: font.sizeSm,
    fontWeight: font.weightMedium,
  },
  row: {
    alignItems: "center",
    columnGap: "8px",
    display: "flex",
    flexWrap: "wrap",
    rowGap: "8px",
  },
});

const TONES: Readonly<Record<AssigneeStatus, BadgeTone>> = {
  accepted: "info",
  completed: "ok",
  declined: "danger",
  delegated: "neutral",
  failed: "danger",
  "in-process": "brand",
  "needs-action": "neutral",
  tentative: "warn",
};

const StatusBadge = ({ status }: { readonly status: AssigneeStatus }) => (
  <Badge tone={TONES[status]}>{assigneeStatusText(status)}</Badge>
);

/** "Delegated" means the task is someone else's now: no more answers. */
const AnswerButtons = ({
  actions,
  status,
  task,
}: {
  readonly task: Task;
  readonly actions: TaskActions;
  readonly status: AssigneeStatus;
}) => (
  <div {...stylex.props(styles.row)}>
    <Button
      icon={Check}
      onClick={() => {
        actions.answer(task, "accepted");
      }}
      size="sm"
      variant={status === "accepted" ? "primary" : "secondary"}
    >
      {m.tasks_answer_accept()}
    </Button>
    <Button
      onClick={() => {
        actions.answer(task, "in-process");
      }}
      size="sm"
      variant={status === "in-process" ? "primary" : "secondary"}
    >
      {m.tasks_answer_in_process()}
    </Button>
    <Button
      icon={X}
      onClick={() => {
        actions.answer(task, "declined");
      }}
      size="sm"
      variant={status === "declined" ? "primary" : "secondary"}
    >
      {m.tasks_answer_decline()}
    </Button>
  </div>
);

const AssignedToMe = ({
  actions,
  assignment,
  canEdit,
  own,
  series,
  task,
}: {
  readonly task: Task;
  readonly series: RawTask;
  readonly actions: TaskActions;
  readonly canEdit: boolean;
  readonly assignment: Assignment & { readonly me: TaskAssignee };
  readonly own: readonly string[];
}) => {
  const { me, organizer } = assignment;
  const name = cleanDisplayText(organizer?.name ?? organizer?.email ?? "");
  const [proposing, setProposing] = useState(false);
  const [delegating, setDelegating] = useState(false);
  const nameOf = (email: string | null): string => {
    const person = assignment.assignees.find((item) => item.email === email);
    return cleanDisplayText(person?.name ?? email ?? "");
  };
  const delegated = me.status === "delegated";
  return (
    <div {...stylex.props(styles.group)}>
      <div {...stylex.props(styles.row)}>
        <span {...stylex.props(styles.label)}>
          <Users aria-hidden size={14} />
          <bdi>{m.tasks_assigned_by({ name })}</bdi>
        </span>
        <StatusBadge status={me.status} />
      </div>
      {me.delegatedFrom === null ? null : (
        <p {...stylex.props(styles.hint)}>
          <bdi>
            {m.tasks_delegated_from({ name: nameOf(me.delegatedFrom) })}
          </bdi>
        </p>
      )}
      {delegated && me.delegatedTo !== null ? (
        <p {...stylex.props(styles.hint)}>
          <bdi>{m.tasks_delegated_to({ name: nameOf(me.delegatedTo) })}</bdi>
        </p>
      ) : null}
      {canEdit && !delegated ? (
        <>
          <AnswerButtons actions={actions} status={me.status} task={task} />
          <TaskMoreOptions organizer={name}>
            <ProposeDueOption
              busy={proposing}
              due={task.due}
              onPropose={(due, comment) => {
                setProposing(true);
                void actions.propose(task, due, comment).finally(() => {
                  setProposing(false);
                });
              }}
            />
            {mayDelegate(series, own) ? (
              <DelegateOption
                busy={delegating}
                exclude={[
                  ...own,
                  ...assignment.assignees.map((person) => person.email),
                  ...(organizer === null ? [] : [organizer.email]),
                ]}
                onDelegate={(person, comment) => {
                  setDelegating(true);
                  void actions.delegate(task, person, comment).finally(() => {
                    setDelegating(false);
                  });
                }}
                repeating={repeats(series)}
              />
            ) : null}
          </TaskMoreOptions>
        </>
      ) : null}
    </div>
  );
};

const AssigneeChips = ({
  onRemove,
  people,
}: {
  readonly people: readonly TaskAssignee[];
  /** Null: the user can't take anyone off. */
  readonly onRemove: ((email: string) => void) | null;
}) => (
  <ul aria-label={m.tasks_assignees()} {...stylex.props(styles.chips)}>
    {people.map((person) => {
      const name = cleanDisplayText(person.name ?? person.email);
      return (
        <li
          key={person.email}
          title={person.email}
          {...stylex.props(styles.chip)}
        >
          <Avatar colorKey={person.email} name={name} size={20} />
          <span {...stylex.props(shared.ellipsis)}>
            <bdi>{name}</bdi>
          </span>
          <StatusBadge status={person.status} />
          {onRemove === null ? null : (
            <IconButton
              icon={X}
              label={m.tasks_assign_remove({ name })}
              onClick={() => {
                onRemove(person.email);
              }}
              size="xs"
              tooltip={false}
            />
          )}
        </li>
      );
    })}
  </ul>
);

/** The user organizes the task (or nobody else is in it yet). */
const Assign = ({
  actions,
  mayAssign,
  own,
  people,
  task,
}: {
  readonly task: Task;
  readonly actions: TaskActions;
  readonly people: readonly TaskAssignee[];
  readonly own: readonly string[];
  readonly mayAssign: boolean;
}) => {
  const id = useId();
  const current: AssigneeInput[] = people.map((person) => ({
    email: person.email,
    name: person.name,
  }));
  const add = (list: readonly AssigneeInput[]): void => {
    const known = new Set([...current.map((person) => person.email), ...own]);
    const added = list.filter((person) => !known.has(person.email));
    if (added.length > 0) {
      actions.assign(task, [...current, ...added]);
    }
  };
  return (
    <div {...stylex.props(styles.group)}>
      <label htmlFor={id} {...stylex.props(styles.label)}>
        <Users aria-hidden size={14} />
        {m.tasks_assign()}
      </label>
      {people.length > 0 ? (
        <AssigneeChips
          onRemove={
            mayAssign
              ? (email) => {
                  actions.assign(
                    task,
                    current.filter((item) => item.email !== email)
                  );
                }
              : null
          }
          people={people}
        />
      ) : null}
      {mayAssign ? (
        <AssignInput
          exclude={[...people.map((person) => person.email), ...own]}
          id={id}
          onAdd={add}
        />
      ) : null}
    </div>
  );
};

export const AssigneesField = ({
  actions,
  canEdit,
  series,
  task,
}: {
  readonly task: Task;
  readonly series: RawTask | undefined;
  readonly actions: TaskActions;
  readonly canEdit: boolean;
}) => {
  const own = useOwnCalendarAddresses().data;
  const assignable = useAssignableAccount();
  if (series === undefined || own === undefined) {
    return null;
  }
  const assignment = assignmentOf(series, own);
  // Assigned to the user by someone else: who, and the answer.
  if (
    assignment !== null &&
    !assignment.iAmOrganizer &&
    assignment.me !== null
  ) {
    return (
      <AssignedToMe
        actions={actions}
        assignment={{ ...assignment, me: assignment.me }}
        canEdit={canEdit}
        own={own}
        series={series}
        task={task}
      />
    );
  }
  const people = assignment?.assignees ?? [];
  const mayAssign =
    canEdit &&
    assignable === task.accountId &&
    (assignment === null || assignment.iAmOrganizer);
  if (!mayAssign && people.length === 0) {
    return null;
  }
  return (
    <Assign
      actions={actions}
      mayAssign={mayAssign}
      own={own}
      people={people}
      task={task}
    />
  );
};
