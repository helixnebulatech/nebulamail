// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// An assigned task inside an email (iTIP VTODO over iMIP)
// what, by when, who assigned it and to whom.
// Nothing reaches the user's tasks until they press a button: Accept /
// Decline (and the reply email), propose another due or pass the task on,
// remove a task taken back, write an assignee's answer into the user's own
// task, or accept / keep the due an assignee proposes. Event invitations
// have their own card (components/calendar/invitation-card.tsx).

import { cleanDisplayText } from "@nebulamail/mail-render";
import { Badge, Button, useToast } from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import {
  CalendarCheck,
  Check,
  ListTodo,
  Trash2,
  TriangleAlert,
  X,
} from "lucide-react";

import { calendarsAvailable } from "../../calendar/api";
import {
  calendarKeys,
  useMe,
  useOwnCalendarAddresses,
  useTimeZone,
} from "../../calendar/hooks";
import { invitationPart } from "../../calendar/invitations";
import type { InvitationSender } from "../../calendar/itip";
import { describeRecurrence, fromRule } from "../../calendar/recurrence";
import type { Attachment } from "../../mail/api";
import { m } from "../../paraglide/messages.js";
import type { AssigneeInput, TaskAnswer } from "../../tasks/assignment";
import { mayDelegate } from "../../tasks/assignment";
import type { TaskAssignmentView } from "../../tasks/assignment-mail";
import {
  DelegateTakenError,
  acceptTaskCounter,
  answerAssignment,
  applyAssignmentReply,
  declineTaskCounter,
  delegateAssignment,
  loadTaskAssignment,
  proposeForAssignment,
  removeAssignment,
} from "../../tasks/assignment-mail";
import { refreshTasks, taskKeys } from "../../tasks/hooks";
import { repeats } from "../../tasks/itip-series";
import type { TaskDue } from "../../tasks/model";
import { dueOf } from "../../tasks/model";
import { SafeText, titleText } from "../calendar/event-text";
import { recurrenceWords } from "../calendar/format";
import { useDateLocale } from "../calendar/use-calendar-prefs";
import { sendErrorText } from "../calendar/use-event-actions";
import {
  assigneeStatusText,
  fullDueText,
  taskCounterTexts,
  taskDeclineCounterTexts,
  taskDelegationTexts,
  taskReplyTexts,
} from "./task-format";
import {
  DelegateOption,
  ProposeDueOption,
  TaskMoreOptions,
} from "./task-more-options";

const styles = stylex.create({
  actions: {
    alignItems: "center",
    columnGap: "8px",
    display: "flex",
    flexWrap: "wrap",
    marginTop: "12px",
    rowGap: "8px",
  },
  comment: {
    borderInlineStartColor: colors.line,
    borderInlineStartStyle: "solid",
    borderInlineStartWidth: "3px",
    color: colors.fg2,
    fontSize: font.sizeMd,
    marginBlock: "10px 0",
    marginInline: 0,
    overflowWrap: "anywhere",
    paddingInlineStart: "10px",
    whiteSpace: "pre-line",
  },
  card: {
    backgroundColor: colors.bg,
    borderColor: colors.line,
    borderInlineStartColor: colors.acc,
    borderInlineStartWidth: "3px",
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: "1px",
    marginBlock: "12px",
    paddingBlock: "12px",
    paddingInline: "14px",
  },
  head: {
    alignItems: "center",
    color: colors.fg3,
    columnGap: "8px",
    display: "flex",
    flexWrap: "wrap",
    fontSize: font.sizeSm,
  },
  icon: { color: colors.accText, flexShrink: 0 },
  line: { color: colors.fg2, fontSize: font.sizeMd, marginTop: "4px" },
  muted: { color: colors.fg3, fontSize: font.sizeSm, margin: "8px 0 0" },
  people: {
    display: "flex",
    flexWrap: "wrap",
    gap: "6px",
    listStyle: "none",
    margin: "8px 0 0",
    padding: 0,
  },
  title: {
    fontSize: font.sizeLg,
    fontWeight: font.weightBold,
    margin: "6px 0 2px",
    overflowWrap: "anywhere",
  },
  warn: {
    alignItems: "center",
    color: colors.warn,
    columnGap: "6px",
    display: "flex",
    fontSize: font.sizeSm,
    marginTop: "8px",
  },
});

const clean = (text: string | null | undefined): string =>
  cleanDisplayText(text ?? "");

const headline = (view: TaskAssignmentView): string => {
  const { method, organizer } = view.invitation;
  const name = clean(organizer?.name ?? organizer?.email);
  switch (method) {
    case "cancel": {
      return m.tasks_card_cancel({ name });
    }
    case "reply": {
      return m.tasks_card_reply();
    }
    case "counter": {
      const author = clean(
        view.counterAuthor?.name ?? view.counterAuthor?.email
      );
      return author === ""
        ? m.tasks_card_counter_me()
        : m.tasks_card_counter({ name: author });
    }
    case "declinecounter": {
      return m.tasks_card_declinecounter({ name });
    }
    case "request":
    case "publish":
    case "other": {
      return name === ""
        ? m.tasks_card_request_me()
        : m.tasks_card_request({ name });
    }
    default: {
      return method satisfies never;
    }
  }
};

const DueLine = ({ view }: { readonly view: TaskAssignmentView }) => {
  const zone = useTimeZone();
  const locale = useDateLocale();
  const due = dueOf(view.task);
  if (view.action === "counter") {
    const now = view.stored === null ? null : dueOf(view.stored);
    return (
      <>
        <div {...stylex.props(styles.line)}>
          {m.tasks_card_proposed({
            date: due === null ? "—" : fullDueText(due, zone, locale),
          })}
        </div>
        <div {...stylex.props(styles.line)}>
          {m.tasks_card_now({
            date:
              now === null
                ? m.tasks_card_no_due()
                : fullDueText(now, zone, locale),
          })}
        </div>
      </>
    );
  }
  // A series: its first due, and how it repeats.
  const rule = describeRecurrence(
    fromRule(view.task.recurrenceRule, view.task.showWithoutTime === true),
    locale,
    recurrenceWords
  );
  return (
    <>
      <div {...stylex.props(styles.line)}>
        {due === null
          ? m.tasks_card_no_due()
          : m.tasks_card_due({ date: fullDueText(due, zone, locale) })}
      </div>
      {rule === null ? null : (
        <div {...stylex.props(styles.line)}>
          {m.tasks_card_repeats({ rule })}
        </div>
      )}
    </>
  );
};

const useActions = (view: TaskAssignmentView) => {
  const client = useQueryClient();
  const toast = useToast();
  const own = useOwnCalendarAddresses().data ?? [];
  const me = useMe().data;
  const zone = useTimeZone();
  const locale = useDateLocale();
  const taskTitle = view.task.title ?? "";
  const myName = me?.name ?? null;
  const organizerName =
    view.invitation.organizer?.name ?? view.invitation.organizer?.email ?? "";
  const after = async (title: string): Promise<void> => {
    toast.show({ title });
    // Tasks, and this card (refreshTasks reads assigned tasks' cards too).
    await refreshTasks(client);
  };
  const failed = (error: unknown): void => {
    toast.show({
      priority: "high",
      title: sendErrorText(
        error,
        error instanceof Error ? error.message : String(error)
      ),
      tone: "danger",
    });
  };
  const answer = useMutation({
    mutationFn: async (value: TaskAnswer) => {
      await answerAssignment(view, value, {
        name: me?.name ?? null,
        own,
        texts: taskReplyTexts,
      });
      return value;
    },
    onError: failed,
    onSuccess: async (value) => {
      await after(
        value === "declined"
          ? m.tasks_card_declined_toast()
          : m.tasks_card_accepted_toast()
      );
    },
  });
  const remove = useMutation({
    mutationFn: async () => {
      await removeAssignment(view);
    },
    onError: failed,
    onSuccess: async () => {
      await after(m.tasks_card_removed_toast());
    },
  });
  const apply = useMutation({
    mutationFn: async () => {
      await applyAssignmentReply(view, own);
    },
    onError: failed,
    onSuccess: async () => {
      await after(m.tasks_card_applied_toast());
    },
  });
  const propose = useMutation({
    mutationFn: async (input: {
      readonly due: TaskDue;
      readonly comment: string | undefined;
    }) => {
      await proposeForAssignment(view, input.due, {
        display: zone,
        name: myName,
        own,
        texts: taskCounterTexts(
          taskTitle,
          myName ?? me?.email ?? "",
          fullDueText(input.due, zone, locale)
        ),
        ...(input.comment === undefined ? {} : { comment: input.comment }),
      });
    },
    onError: failed,
    onSuccess: () => {
      toast.show({
        title: m.tasks_proposal_sent({ name: clean(organizerName) }),
      });
    },
  });
  const delegate = useMutation({
    mutationFn: async (input: {
      readonly person: AssigneeInput;
      readonly comment: string | undefined;
    }) => {
      await delegateAssignment(view, input.person, {
        display: zone,
        name: myName,
        own,
        texts: taskDelegationTexts({
          delegate: input.person.name ?? input.person.email,
          name: myName ?? me?.email ?? "",
          organizer: organizerName,
          title: taskTitle,
        }),
        ...(input.comment === undefined ? {} : { comment: input.comment }),
      });
      return input.person;
    },
    onError: (error) => {
      failed(
        error instanceof DelegateTakenError
          ? new Error(m.tasks_delegate_taken())
          : error
      );
    },
    onSuccess: async (person) => {
      await after(
        m.tasks_delegated_toast({ name: clean(person.name ?? person.email) })
      );
    },
  });
  const counter = useMutation({
    mutationFn: async (accept: boolean) => {
      await (accept
        ? acceptTaskCounter(view, zone)
        : declineTaskCounter(view, {
            own,
            texts: taskDeclineCounterTexts(
              taskTitle,
              myName ?? me?.email ?? ""
            ),
          }));
      return accept;
    },
    onError: failed,
    onSuccess: async (accepted) => {
      await after(
        accepted
          ? m.tasks_card_counter_accepted()
          : m.tasks_card_counter_declined({
              name: clean(
                view.counterAuthor?.name ?? view.counterAuthor?.email
              ),
            })
      );
    },
  });
  return { answer, apply, counter, delegate, propose, remove };
};

/** Propose another due or pass the task on, from the email. */
const More = ({
  actions,
  view,
}: {
  readonly view: TaskAssignmentView;
  readonly actions: ReturnType<typeof useActions>;
}) => {
  const own = useOwnCalendarAddresses().data ?? [];
  const { organizer } = view.invitation;
  const source = view.stored ?? view.task;
  return (
    <TaskMoreOptions organizer={clean(organizer?.name ?? organizer?.email)}>
      <ProposeDueOption
        busy={actions.propose.isPending}
        due={dueOf(view.task)}
        onPropose={(due, comment) => {
          actions.propose.mutate({ comment, due });
        }}
      />
      {mayDelegate(source, own) ? (
        <DelegateOption
          busy={actions.delegate.isPending}
          exclude={[
            ...own,
            ...(view.assignment?.assignees ?? []).map((person) => person.email),
            ...(organizer === null ? [] : [organizer.email]),
          ]}
          onDelegate={(person, comment) => {
            actions.delegate.mutate({ comment, person });
          }}
          repeating={repeats(source)}
        />
      ) : null}
    </TaskMoreOptions>
  );
};

/** Accept the proposed due, or keep the current one (DECLINECOUNTER). */
const CounterButtons = ({
  busy,
  counter,
}: {
  readonly busy: boolean;
  readonly counter: ReturnType<typeof useActions>["counter"];
}) => (
  <>
    <Button
      disabled={busy}
      icon={CalendarCheck}
      loading={counter.isPending && counter.variables}
      onClick={() => {
        counter.mutate(true);
      }}
      size="sm"
      variant="primary"
    >
      {m.tasks_card_counter_accept()}
    </Button>
    <Button
      disabled={busy}
      icon={X}
      loading={counter.isPending && !counter.variables}
      onClick={() => {
        counter.mutate(false);
      }}
      size="sm"
      variant="secondary"
    >
      {m.tasks_card_counter_decline()}
    </Button>
  </>
);

const personName = (person: {
  readonly name: string | null;
  readonly email: string;
}): string => cleanDisplayText(person.name ?? "") || person.email;

/** Who passed the task on, as the REPLY says (untrusted text). */
const delegatorNames = (view: TaskAssignmentView): string =>
  [
    ...new Set(
      view.delegates.flatMap((delegate) =>
        delegate.delegatedFrom === null ? [] : [delegate.delegatedFrom]
      )
    ),
  ].join(", ");

const Actions = ({ view }: { readonly view: TaskAssignmentView }) => {
  const actions = useActions(view);
  const { answer, apply, counter, delegate, remove } = actions;
  const myStatus = view.assignment?.me?.status ?? null;
  const busy =
    answer.isPending ||
    remove.isPending ||
    apply.isPending ||
    counter.isPending ||
    delegate.isPending;
  const more = view.action === "answer" && !view.outdated;
  return (
    <>
      <div {...stylex.props(styles.actions)}>
        {view.action === "answer" && !view.outdated ? (
          <>
            <Button
              disabled={busy}
              icon={Check}
              loading={answer.isPending && answer.variables === "accepted"}
              onClick={() => {
                answer.mutate("accepted");
              }}
              size="sm"
              variant="primary"
            >
              {m.tasks_answer_accept()}
            </Button>
            <Button
              disabled={busy}
              icon={X}
              loading={answer.isPending && answer.variables === "declined"}
              onClick={() => {
                answer.mutate("declined");
              }}
              size="sm"
              variant="secondary"
            >
              {m.tasks_answer_decline()}
            </Button>
          </>
        ) : null}
        {view.action === "counter" && dueOf(view.task) !== null ? (
          <CounterButtons busy={busy} counter={counter} />
        ) : null}
        {view.action === "remove" ? (
          <Button
            disabled={busy}
            icon={Trash2}
            loading={remove.isPending}
            onClick={() => {
              remove.mutate();
            }}
            size="sm"
            variant="secondary"
          >
            {m.tasks_card_remove()}
          </Button>
        ) : null}
        {view.action === "apply-reply" ? (
          <Button
            disabled={busy}
            icon={Check}
            loading={apply.isPending}
            onClick={() => {
              apply.mutate();
            }}
            size="sm"
            variant="primary"
          >
            {m.tasks_card_apply()}
          </Button>
        ) : null}
        {view.stored === null ? null : (
          <>
            <Badge tone="neutral">{m.tasks_card_in_tasks()}</Badge>
            {myStatus === null ? null : (
              <Badge tone="info">{assigneeStatusText(myStatus)}</Badge>
            )}
            <Link search={{ task: view.stored.id }} to="/tasks">
              {m.tasks_card_open()}
            </Link>
          </>
        )}
      </div>
      {more ? <More actions={actions} view={view} /> : null}
    </>
  );
};

const Card = ({ view }: { readonly view: TaskAssignmentView }) => {
  const { organizer } = view.invitation;
  const people = view.assignment?.assignees ?? [];
  return (
    <section aria-label={m.tasks_card_label()} {...stylex.props(styles.card)}>
      <div {...stylex.props(styles.head)}>
        <ListTodo aria-hidden size={16} {...stylex.props(styles.icon)} />
        <bdi>{headline(view)}</bdi>
      </div>
      <h3 dir="auto" {...stylex.props(styles.title)}>
        {titleText(view.task.title ?? "")}
      </h3>
      <DueLine view={view} />
      {view.invitation.comment === null ? null : (
        <blockquote
          aria-label={m.inv_comment()}
          dir="auto"
          {...stylex.props(styles.comment)}
        >
          <SafeText text={view.invitation.comment} />
        </blockquote>
      )}
      {people.length > 0 ? (
        <ul aria-label={m.tasks_assignees()} {...stylex.props(styles.people)}>
          {people.map((person) => (
            <li key={person.email}>
              <Badge tone={person.isMe ? "brand" : "neutral"}>
                <bdi>
                  {m.tasks_assignee_status({
                    name: clean(person.name ?? person.email),
                    status: assigneeStatusText(person.status),
                  })}
                </bdi>
              </Badge>
            </li>
          ))}
        </ul>
      ) : null}
      {view.outdated ? (
        <div {...stylex.props(styles.warn)}>
          <TriangleAlert aria-hidden size={14} />
          {m.tasks_card_outdated()}
        </div>
      ) : null}
      {view.senderBlocked && !view.outdated ? (
        <div role="note" {...stylex.props(styles.warn)}>
          <TriangleAlert aria-hidden size={14} />
          {m.tasks_card_sender_unverified()}
        </div>
      ) : null}
      {view.action === "apply-reply" && view.delegates.length > 0 ? (
        <div {...stylex.props(styles.warn)}>
          <TriangleAlert aria-hidden size={14} />
          <bdi>
            {m.tasks_card_apply_adds({
              from: delegatorNames(view),
              names: view.delegates.map(personName).join(", "),
            })}
          </bdi>
        </div>
      ) : null}
      <Actions view={view} />
      {view.action === "answer" && organizer !== null ? (
        <p {...stylex.props(styles.muted)}>
          {m.tasks_card_note({
            name: clean(organizer.name ?? organizer.email),
          })}
        </p>
      ) : null}
    </section>
  );
};

/** The assigned-task card of an email, if its calendar part is a task. */
export const TaskAssignmentCard = ({
  attachments,
  sender,
}: {
  readonly attachments: readonly Attachment[];
  /** Who sent the email (`invitationSender`). */
  readonly sender: InvitationSender;
}) => {
  const part = invitationPart(attachments);
  const enabled =
    useQuery({
      enabled: part !== null,
      queryFn: calendarsAvailable,
      queryKey: [...calendarKeys.all, "available"],
      staleTime: Infinity,
    }).data === true;
  const own = useOwnCalendarAddresses().data;
  const blobId = part?.blobId ?? null;
  const query = useQuery({
    enabled: enabled && blobId !== null && own !== undefined,
    queryFn: async () =>
      blobId === null || own === undefined
        ? null
        : await loadTaskAssignment(blobId, own, sender),
    queryKey: [
      ...taskKeys.assignments(),
      blobId ?? "",
      sender.email ?? "",
      sender.verdict ?? "",
    ],
    retry: false,
  });
  if (query.data === null || query.data === undefined) {
    return null;
  }
  return <Card view={query.data} />;
};
