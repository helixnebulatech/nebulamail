// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// An invitation inside an email (iTIP over iMIP): what,
// when (in the user's time zone), who, where, and what clashes with it.
// Nothing reaches the calendar until the user presses a button: Accept /
// Maybe / Decline (and the reply email), remove a cancelled event, apply a
// guest's answer to the user's own event, or add an event sent to a list.

import { Badge, Button, useToast } from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import {
  CalendarCheck,
  CalendarDays,
  CalendarPlus,
  CalendarX,
  Check,
  HelpCircle,
  MapPin,
  TriangleAlert,
  Users,
  X,
} from "lucide-react";

import { calendarsAvailable } from "../../calendar/api";
import {
  calendarKeys,
  refreshEvents,
  useCalendars,
  useInvitation,
  useMe,
  useOwnCalendarAddresses,
  useTimeZone,
} from "../../calendar/hooks";
import type {
  Answer,
  InvitationView,
  Proposal,
  ReplyExtras,
} from "../../calendar/invitations";
import {
  acceptCounter,
  addInvitation,
  answerInvitation,
  applyReply,
  declineCounter,
  invitationPart,
  OccurrenceGoneError,
  proposeForInvitation,
  removeCancelled,
} from "../../calendar/invitations";
import { replyAuthor } from "../../calendar/itip";
import type { InvitationSender } from "../../calendar/itip";
import { timedRange } from "../../calendar/time";
import type { Attachment } from "../../mail/api";
import { m } from "../../paraglide/messages.js";
import { shared } from "../mail/styles";
import { SafeText, titleText } from "./event-text";
import { formatInZone, formatWhen, statusText, statusTone } from "./format";
import { ReplyOptions, useReplyDraft } from "./reply-options";
import { useDateLocale } from "./use-calendar-prefs";
import {
  counterTexts,
  declineCounterTexts,
  replyTexts,
  sendErrorText,
} from "./use-event-actions";

const styles = stylex.create({
  actions: {
    alignItems: "center",
    columnGap: "8px",
    display: "flex",
    flexWrap: "wrap",
    marginTop: "12px",
    rowGap: "8px",
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
  conflict: {
    alignItems: "flex-start",
    backgroundColor: colors.warnSoft,
    borderRadius: radius.md,
    color: colors.warn,
    columnGap: "8px",
    display: "flex",
    fontSize: font.sizeSm,
    marginTop: "10px",
    paddingBlock: "6px",
    paddingInline: "10px",
  },
  head: {
    alignItems: "center",
    color: colors.fg3,
    columnGap: "8px",
    display: "flex",
    flexWrap: "wrap",
    fontSize: font.sizeSm,
    rowGap: "4px",
  },
  icon: { color: colors.accText, flexShrink: 0 },
  line: {
    alignItems: "flex-start",
    color: colors.fg2,
    columnGap: "8px",
    display: "flex",
    fontSize: font.sizeMd,
    marginTop: "4px",
  },
  lineIcon: { color: colors.fg3, flexShrink: 0, marginTop: "3px" },
  muted: { color: colors.fg3, fontSize: font.sizeSm },
  note: { color: colors.fg3, fontSize: font.sizeSm, margin: "8px 0 0" },
  spacer: { flexGrow: 1 },
  title: {
    fontSize: font.sizeLg,
    fontWeight: font.weightBold,
    margin: "6px 0 2px",
    overflowWrap: "anywhere",
  },
  when: {
    color: colors.fg,
    fontSize: font.sizeMd,
    fontWeight: font.weightMedium,
  },
});

const ANSWERS: readonly {
  readonly status: Answer;
  readonly icon: typeof Check;
  readonly label: () => string;
}[] = [
  { icon: Check, label: m.cal_answer_yes, status: "accepted" },
  { icon: HelpCircle, label: m.cal_answer_maybe, status: "tentative" },
  { icon: X, label: m.cal_answer_no, status: "declined" },
];

const MAX_GUESTS = 4;

const DONE = {
  add: m.inv_added,
  apply: m.inv_applied,
  remove: m.inv_removed,
} as const;

const headline = (view: InvitationView): string => {
  const { invitation } = view;
  switch (invitation.method) {
    case "cancel": {
      return m.inv_cancelled();
    }
    case "reply": {
      const author = replyAuthor(invitation, view.stored, view.sender);
      return author === null
        ? m.inv_reply()
        : m.inv_reply_from({
            name: author.name ?? author.email,
            status: statusText(author.status).toLowerCase(),
          });
    }
    case "request": {
      return view.stored !== null &&
        invitation.sequence > (view.stored.sequence ?? 0)
        ? m.inv_updated()
        : m.inv_invitation();
    }
    case "counter": {
      const author = replyAuthor(invitation, view.stored, view.sender);
      return author === null
        ? m.inv_counter()
        : m.inv_counter_from({ name: author.name ?? author.email });
    }
    case "declinecounter": {
      return invitation.organizer === null
        ? m.inv_declinecounter()
        : m.inv_declinecounter_from({
            name: invitation.organizer.name ?? invitation.organizer.email,
          });
    }
    case "publish":
    case "other": {
      return m.inv_event();
    }
    default: {
      return invitation.method satisfies never;
    }
  }
};

const useCardActions = (view: InvitationView | null | undefined) => {
  const client = useQueryClient();
  const toast = useToast();
  const zone = useTimeZone();
  const calendars = useCalendars();
  const me = useMe();
  const own = useOwnCalendarAddresses();
  const options = {
    calendarId:
      calendars.data?.find(
        (calendar) => calendar.isDefault && calendar.mayWrite
      )?.id ??
      calendars.data?.find(
        (calendar) => calendar.mayWrite && calendar.owner === null
      )?.id ??
      null,
    calendars: calendars.data ?? [],
    own: own.data ?? [],
    zone,
  };
  const after = async (): Promise<void> => {
    await refreshEvents(client);
    // Stalwart indexes new events a moment after saving them: a search by
    // uid right away still misses the event we just added,
    // so look again shortly after.
    setTimeout(() => {
      void refreshEvents(client);
    }, 1000);
  };
  // The first one that isn't empty (an invitation may name us "").
  const myName =
    [
      view?.invitation.me?.name,
      me.data?.name,
      view?.invitation.me?.email,
      me.data?.email,
    ].find(
      (text) => text !== undefined && text !== null && text.trim() !== ""
    ) ?? "";
  const answer = useMutation({
    mutationFn: async ({
      extras,
      status,
    }: {
      readonly status: Answer;
      readonly extras: ReplyExtras;
    }) => {
      if (view === null || view === undefined) {
        return;
      }
      await answerInvitation(
        view,
        status,
        replyTexts(status, view.invitation.event.title ?? "", myName),
        options,
        extras
      );
    },
    onError: (error) => {
      toast.show({
        title: sendErrorText(error, m.cal_error_reply()),
        tone: "danger",
      });
    },
    onSuccess: async () => {
      toast.show({ title: m.cal_reply_sent() });
      await after();
    },
  });
  const other = useMutation({
    mutationFn: async (kind: "remove" | "apply" | "add") => {
      if (view === null || view === undefined) {
        return;
      }
      if (kind === "remove") {
        await removeCancelled(view, options);
      } else if (kind === "apply") {
        await applyReply(view, options);
      } else if (options.calendarId !== null) {
        await addInvitation(view, options.calendarId);
      }
    },
    onError: (error) => {
      toast.show({
        title:
          error instanceof OccurrenceGoneError
            ? m.inv_occurrence_gone()
            : m.cal_error_save(),
        tone: "danger",
      });
    },
    onSuccess: async (_result, kind) => {
      toast.show({ title: DONE[kind]() });
      await after();
    },
  });
  // Proposing another time (attendee) and answering a proposal (organizer).
  const propose = useMutation({
    mutationFn: async ({
      extras,
      proposal,
      when,
    }: {
      readonly proposal: Proposal;
      readonly when: string;
      readonly extras: ReplyExtras;
    }) => {
      if (view === null || view === undefined) {
        return;
      }
      await proposeForInvitation(
        view,
        proposal,
        counterTexts(view.invitation.event.title ?? "", myName, when),
        extras
      );
    },
    onError: (error) => {
      toast.show({
        title: sendErrorText(error, m.cal_error_reply()),
        tone: "danger",
      });
    },
    onSuccess: () => {
      toast.show({ title: m.cal_proposal_sent() });
    },
  });
  const counter = useMutation({
    mutationFn: async (choice: "accept" | "decline") => {
      if (view === null || view === undefined) {
        return;
      }
      await (choice === "accept"
        ? acceptCounter(view, options)
        : declineCounter(
            view,
            declineCounterTexts(
              view.invitation.event.title ?? "",
              me.data?.name ?? me.data?.email ?? ""
            )
          ));
    },
    onError: (error) => {
      toast.show({
        title: sendErrorText(error, m.cal_error_save()),
        tone: "danger",
      });
    },
    onSuccess: async (_result, choice) => {
      toast.show({
        title:
          choice === "accept"
            ? m.inv_counter_accepted()
            : m.inv_counter_declined(),
      });
      await after();
    },
  });
  return { answer, counter, other, propose };
};

/** Short list of names: "Ana, Bob, Carla and 3 more". */
const Names = ({
  names,
  max,
}: {
  readonly names: readonly { readonly key: string; readonly text: string }[];
  readonly max: number;
}) => (
  <span>
    {names.slice(0, max).map((name, index) => (
      <span key={name.key}>
        {index > 0 ? ", " : ""}
        <SafeText text={name.text} />
      </span>
    ))}
    {names.length > max
      ? ` ${m.inv_more_guests({ count: names.length - max })}`
      : ""}
  </span>
);

const CardHead = ({ view }: { readonly view: InvitationView }) => {
  const { invitation } = view;
  const showStatus =
    view.myStatus !== null &&
    invitation.method === "request" &&
    !invitation.iAmOrganizer;
  const fromLine =
    invitation.organizer !== null && invitation.method !== "reply";
  return (
    <div {...stylex.props(styles.head)}>
      {invitation.cancelled ? (
        <CalendarX aria-hidden size={16} {...stylex.props(styles.icon)} />
      ) : (
        <CalendarDays aria-hidden size={16} {...stylex.props(styles.icon)} />
      )}
      <span>{headline(view)}</span>
      {fromLine && invitation.organizer !== null ? (
        <span>
          · {m.inv_from()}{" "}
          <SafeText
            text={invitation.organizer.name ?? invitation.organizer.email}
          />
        </span>
      ) : null}
      <span {...stylex.props(styles.spacer)} />
      {showStatus && view.myStatus !== null ? (
        <Badge tone={statusTone(view.myStatus)}>
          {statusText(view.myStatus)}
        </Badge>
      ) : null}
      {view.stored === null ? null : (
        <Badge icon={CalendarCheck} tone="info">
          {m.inv_in_calendar()}
        </Badge>
      )}
    </div>
  );
};

const firstLocation = (view: InvitationView): string | null => {
  for (const location of Object.values(view.invitation.event.locations ?? {})) {
    const name = location.name?.trim() ?? "";
    if (name !== "") {
      return name;
    }
  }
  return null;
};

/** For a proposal: when the event is now, to compare. */
const CurrentTime = ({ view }: { readonly view: InvitationView }) => {
  const zone = useTimeZone();
  const locale = useDateLocale();
  const { stored } = view;
  const range =
    stored === null
      ? null
      : timedRange(
          {
            duration: stored.duration,
            start: stored.start,
            timeZone: stored.timeZone,
          },
          zone
        );
  return range === null ? null : (
    <p {...stylex.props(styles.note)}>
      {m.inv_counter_now({
        time: formatWhen({ allDay: false, ...range }, zone, locale),
      })}
    </p>
  );
};

const CardDetails = ({ view }: { readonly view: InvitationView }) => {
  const { invitation } = view;
  const location = firstLocation(view);
  const guests = invitation.attendees
    .filter((attendee) => !attendee.isOrganizer)
    .map((guest) => ({
      key: guest.key,
      text: `${guest.name ?? guest.email}${guest.isMe ? ` ${m.cal_you()}` : ""}`,
    }));
  const conflicts =
    invitation.method === "request" && !invitation.cancelled
      ? view.conflicts.map((conflict) => ({
          key: conflict.id,
          text: titleText(conflict.title),
        }))
      : [];
  return (
    <>
      {location === null ? null : (
        <div {...stylex.props(styles.line)}>
          <MapPin aria-hidden size={14} {...stylex.props(styles.lineIcon)} />
          <span dir="auto">
            <SafeText text={location} />
          </span>
        </div>
      )}
      {guests.length > 0 ? (
        <div {...stylex.props(styles.line)}>
          <Users aria-hidden size={14} {...stylex.props(styles.lineIcon)} />
          <Names max={MAX_GUESTS} names={guests} />
        </div>
      ) : null}
      {conflicts.length > 0 ? (
        <div role="note" {...stylex.props(styles.conflict)}>
          <TriangleAlert aria-hidden size={14} />
          <span>
            {m.inv_conflicts()} <Names max={3} names={conflicts} />
          </span>
        </div>
      ) : null}
      {view.outdated ? (
        <p {...stylex.props(styles.note)}>{m.inv_outdated()}</p>
      ) : null}
      {view.senderBlocked && !view.outdated ? (
        <p role="note" {...stylex.props(styles.note)}>
          {m.inv_sender_unverified()}
        </p>
      ) : null}
      {invitation.comment === null ? null : (
        <blockquote
          aria-label={m.inv_comment()}
          dir="auto"
          {...stylex.props(styles.comment)}
        >
          <SafeText text={invitation.comment} />
        </blockquote>
      )}
      {view.action === "counter" ? <CurrentTime view={view} /> : null}
    </>
  );
};

const OTHER_ACTIONS = {
  add: {
    icon: CalendarPlus,
    kind: "add",
    label: m.inv_add,
    variant: "secondary",
  },
  "apply-reply": {
    icon: CalendarCheck,
    kind: "apply",
    label: m.inv_apply,
    variant: "secondary",
  },
  remove: {
    icon: CalendarX,
    kind: "remove",
    label: m.inv_remove,
    variant: "dangerSoft",
  },
} as const;

const CardActions = ({ view }: { readonly view: InvitationView }) => {
  const actions = useCardActions(view);
  const draft = useReplyDraft();
  const zone = useTimeZone();
  const locale = useDateLocale();
  const busy =
    actions.answer.isPending ||
    actions.other.isPending ||
    actions.counter.isPending;
  const other =
    view.action === "add" ||
    view.action === "apply-reply" ||
    view.action === "remove"
      ? OTHER_ACTIONS[view.action]
      : null;
  const { occurrence } = view;
  const clearDraft = {
    onSuccess: () => {
      draft.setOpen(false);
      draft.setComment("");
    },
  };
  return (
    <>
      <div {...stylex.props(styles.actions)}>
        {view.action === "counter" ? (
          <>
            <Button
              disabled={busy}
              icon={CalendarCheck}
              loading={
                actions.counter.isPending &&
                actions.counter.variables === "accept"
              }
              onClick={() => {
                actions.counter.mutate("accept");
              }}
              size="sm"
              variant="primary"
            >
              {m.inv_counter_accept()}
            </Button>
            <Button
              disabled={busy}
              icon={X}
              loading={
                actions.counter.isPending &&
                actions.counter.variables === "decline"
              }
              onClick={() => {
                actions.counter.mutate("decline");
              }}
              size="sm"
              variant="secondary"
            >
              {m.inv_counter_decline()}
            </Button>
          </>
        ) : null}
        {view.action === "rsvp" && !view.outdated
          ? ANSWERS.map((answer) => (
              <Button
                disabled={busy}
                icon={answer.icon}
                key={answer.status}
                loading={
                  actions.answer.isPending &&
                  actions.answer.variables.status === answer.status
                }
                onClick={() => {
                  actions.answer.mutate(
                    { extras: draft.extras, status: answer.status },
                    clearDraft
                  );
                }}
                pressed={view.myStatus === answer.status}
                size="sm"
                variant={
                  view.myStatus === answer.status ? "primary" : "secondary"
                }
              >
                {answer.label()}
              </Button>
            ))
          : null}
        {other === null ? null : (
          <Button
            disabled={busy}
            icon={other.icon}
            loading={actions.other.isPending}
            onClick={() => {
              actions.other.mutate(other.kind);
            }}
            size="sm"
            variant={other.variant}
          >
            {other.label()}
          </Button>
        )}
        <span {...stylex.props(styles.spacer)} />
        {view.stored === null ? null : (
          <Link
            search={{
              date:
                occurrence.firstDay ??
                new Date(occurrence.start).toISOString().slice(0, 10),
              event: view.stored.id,
            }}
            to="/calendar"
            {...stylex.props(shared.focusRing)}
          >
            {m.inv_open_calendar()}
          </Link>
        )}
      </div>
      {view.action === "rsvp" &&
      !view.outdated &&
      view.invitation.organizer !== null ? (
        <ReplyOptions
          draft={draft}
          end={occurrence.end}
          onPropose={(proposal) => {
            actions.propose.mutate(
              {
                extras: draft.extras,
                proposal,
                when: formatWhen(
                  { allDay: false, end: proposal.end, start: proposal.start },
                  zone,
                  locale
                ),
              },
              clearDraft
            );
          }}
          proposing={actions.propose.isPending}
          start={occurrence.start}
        />
      ) : null}
    </>
  );
};

const Card = ({ view }: { readonly view: InvitationView }) => {
  const zone = useTimeZone();
  const locale = useDateLocale();
  const { invitation, occurrence } = view;
  const elsewhere = occurrence.allDay
    ? null
    : formatInZone(occurrence, invitation.event.timeZone ?? null, zone, locale);
  return (
    <section aria-label={m.inv_label()} {...stylex.props(styles.card)}>
      <CardHead view={view} />
      <h3 dir="auto" {...stylex.props(styles.title)}>
        {titleText(invitation.event.title ?? "")}
      </h3>
      <div {...stylex.props(styles.when)}>
        {formatWhen(occurrence, zone, locale)}
      </div>
      {elsewhere === null ? null : (
        <div {...stylex.props(styles.muted)}>
          {m.inv_their_time({ time: elsewhere })}
        </div>
      )}
      <CardDetails view={view} />
      <CardActions view={view} />
      {view.action === "rsvp" && invitation.organizer !== null ? (
        <p {...stylex.props(styles.note)}>
          {view.stored === null
            ? m.inv_reply_note()
            : m.inv_reply_note_stored()}
        </p>
      ) : null}
    </section>
  );
};

/** The invitation card of an email, if it has a calendar part. */
export const InvitationCard = ({
  attachments,
  sender,
}: {
  readonly attachments: readonly Attachment[];
  /** Who sent the email (`invitationSender`). */
  readonly sender: InvitationSender;
}) => {
  const part = invitationPart(attachments);
  // Only servers with calendars can read invitations.
  const enabled =
    useQuery({
      enabled: part !== null,
      queryFn: calendarsAvailable,
      queryKey: [...calendarKeys.all, "available"],
      staleTime: Infinity,
    }).data === true;
  const query = useInvitation(
    part?.blobId ?? null,
    sender,
    enabled && part !== null
  );
  if (
    part === null ||
    !enabled ||
    query.data === null ||
    query.data === undefined
  ) {
    return null;
  }
  return <Card view={query.data} />;
};
