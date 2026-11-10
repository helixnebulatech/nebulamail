// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// An open event: when, where, who, the description, and what the user can
// do (edit, delete, answer an invitation). Shown next to the calendar, or
// instead of it on narrow screens.

import {
  AlertDialog,
  Avatar,
  Badge,
  Button,
  IconButton,
  Spinner,
  useToast,
} from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import {
  ArrowLeft,
  Bell,
  CalendarDays,
  Check,
  Clock,
  HelpCircle,
  MapPin,
  PenLine,
  Repeat,
  Trash2,
  Users,
  Video,
  X,
} from "lucide-react";
import { useState } from "react";

import type { EventWithSeries } from "../../calendar/api";
import { isTooManyToNotify } from "../../calendar/api";
import {
  useCalendars,
  useEventDetail,
  useMe,
  useTimeZone,
} from "../../calendar/hooks";
import type { Answer } from "../../calendar/invitations";
import type { EventDetail, ParticipationStatus } from "../../calendar/model";
import { describeRecurrence } from "../../calendar/recurrence";
import { useShortcut } from "../../lib/shortcuts";
import { m } from "../../paraglide/messages.js";
import { isInOverlay } from "../mail/mail-ui";
import { shared } from "../mail/styles";
import { colorVars } from "./event-chip";
import {
  EventDescription,
  SafeText,
  titleText,
  useEventLinks,
} from "./event-text";
import {
  alertText,
  formatInZone,
  formatWhen,
  recurrenceWords,
  statusText,
  statusTone,
} from "./format";
import { ReplyOptions, useReplyDraft } from "./reply-options";
import { ScopeDialog } from "./scope-dialog";
import type { EventTarget } from "./use-calendar-nav";
import { useDateLocale } from "./use-calendar-prefs";
import type { Scope } from "./use-event-actions";
import { useEventActions } from "./use-event-actions";

export const panelStyles = stylex.create({
  body: {
    display: "flex",
    flexDirection: "column",
    flexGrow: 1,
    minHeight: 0,
    overflowY: "auto",
    paddingBlock: "4px 20px",
    paddingInline: "20px",
    rowGap: "16px",
  },
  center: {
    alignItems: "center",
    display: "flex",
    flexGrow: 1,
    justifyContent: "center",
  },
  header: {
    alignItems: "center",
    columnGap: "4px",
    display: "flex",
    flexShrink: 0,
    paddingBlock: "10px",
    paddingInline: "12px",
  },
  spacer: { flexGrow: 1 },
});

const styles = stylex.create({
  answer: {
    clear: "both",
    columnGap: "6px",
    display: "flex",
    flexWrap: "wrap",
    rowGap: "6px",
  },
  answerLabel: {
    float: "inline-start",
    padding: 0,
    width: "100%",
    color: colors.fg2,
    fontSize: font.sizeSm,
    fontWeight: font.weightSemibold,
    marginBottom: "6px",
  },
  answerBox: {
    backgroundColor: colors.subtle,
    minWidth: 0,
    borderStyle: "none",
    margin: 0,
    borderRadius: radius.lg,
    paddingBlock: "10px",
    paddingInline: "12px",
  },
  colorBlock: {
    backgroundColor: "var(--event-color)",
    borderRadius: "4px",
    flexShrink: 0,
    height: "14px",
    marginTop: "6px",
    width: "14px",
  },
  item: {
    alignItems: "flex-start",
    color: colors.fg,
    columnGap: "12px",
    display: "flex",
    fontSize: font.sizeMd,
    lineHeight: font.lineHeight,
  },
  itemIcon: { color: colors.fg3, flexShrink: 0, marginTop: "2px" },
  itemText: {
    display: "flex",
    flexDirection: "column",
    minWidth: 0,
    rowGap: "2px",
  },
  muted: { color: colors.fg3, fontSize: font.sizeSm },
  people: { display: "flex", flexDirection: "column", rowGap: "8px" },
  person: { alignItems: "center", columnGap: "10px", display: "flex" },
  personName: {
    display: "flex",
    flexDirection: "column",
    flexGrow: 1,
    fontSize: font.sizeMd,
    minWidth: 0,
  },
  title: {
    fontSize: font.sizeXl,
    fontWeight: font.weightBold,
    letterSpacing: font.trackingTight,
    lineHeight: font.lineHeightTight,
    margin: 0,
    overflowWrap: "anywhere",
  },
  titleRow: { alignItems: "flex-start", columnGap: "12px", display: "flex" },
  cancelled: {
    color: colors.danger,
    fontSize: font.sizeSm,
    fontWeight: font.weightSemibold,
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

const deleteBody = (notifyGuests: boolean, invited: boolean): string => {
  if (notifyGuests) {
    return m.cal_delete_body_guests();
  }
  return invited ? m.cal_delete_body_invited() : m.cal_delete_body();
};

/** "Going?" Yes / Maybe / No, with a note or another time to propose. */
const AnswerBox = ({
  detail,
  status,
}: {
  readonly detail: EventDetail;
  readonly status: ParticipationStatus;
}) => {
  const zone = useTimeZone();
  const locale = useDateLocale();
  const me = useMe();
  const actions = useEventActions();
  const draft = useReplyDraft();
  // A note or another time only goes to an organizer we know.
  const organizerPresent =
    detail.uid !== null &&
    detail.participants.some((person) => person.isOrganizer && !person.isMe);
  const clearDraft = {
    onSuccess: () => {
      draft.setOpen(false);
      draft.setComment("");
    },
  };
  return (
    <fieldset {...stylex.props(styles.answerBox)}>
      <legend {...stylex.props(styles.answerLabel)}>{m.cal_going()}</legend>
      <div {...stylex.props(styles.answer)}>
        {ANSWERS.map((answer) => (
          <Button
            icon={answer.icon}
            key={answer.status}
            loading={
              actions.answer.isPending &&
              actions.answer.variables.status === answer.status
            }
            onClick={() => {
              if (me.data !== undefined) {
                actions.answer.mutate(
                  {
                    detail,
                    extras: draft.extras,
                    me: me.data,
                    status: answer.status,
                  },
                  clearDraft
                );
              }
            }}
            pressed={status === answer.status}
            size="sm"
            variant={status === answer.status ? "primary" : "secondary"}
          >
            {answer.label()}
          </Button>
        ))}
      </div>
      {organizerPresent ? (
        <ReplyOptions
          draft={draft}
          end={detail.end}
          onPropose={(proposal) => {
            if (me.data !== undefined) {
              actions.propose.mutate(
                {
                  detail,
                  extras: draft.extras,
                  me: me.data,
                  proposal,
                  when: formatWhen(
                    { allDay: false, end: proposal.end, start: proposal.start },
                    zone,
                    locale
                  ),
                },
                clearDraft
              );
            }
          }}
          proposing={actions.propose.isPending}
          start={detail.start}
        />
      ) : null}
    </fieldset>
  );
};

const Details = ({
  data,
  onClose,
  onEdit,
}: {
  readonly data: EventWithSeries;
  readonly onClose: () => void;
  readonly onEdit: () => void;
}) => {
  const { detail } = data;
  const zone = useTimeZone();
  const locale = useDateLocale();
  const calendars = useCalendars();
  const actions = useEventActions();
  const toast = useToast();
  const links = useEventLinks();
  const [confirm, setConfirm] = useState<"delete" | "scope" | null>(null);
  const calendar = calendars.data?.find(
    (item) => item.key === detail.calendarKey
  );
  const writable = calendar?.mayWrite !== false;
  // Invitations are answered, not edited: only the organizer changes them.
  const editable =
    writable && (detail.isOrganizer || detail.participants.length === 0);
  const mine = detail.participants.find(
    (person) => person.isMe && !person.isOrganizer
  );
  const guests = detail.participants.length > 1;
  const notifyGuests = detail.isOrganizer && guests;
  const recurrence = describeRecurrence(
    data.recurrence,
    locale,
    recurrenceWords
  );
  const title = titleText(detail.title);

  const remove = (scope: Scope, notify = notifyGuests): void => {
    setConfirm(null);
    actions.remove.mutate(
      { detail, notify, scope },
      {
        onError: (error) => {
          // More guests than the bridge mails a cancellation to: offer to
          // delete without telling them.
          if (notify && isTooManyToNotify(error)) {
            toast.show({
              action: {
                label: m.cal_delete_without_notifying(),
                onClick: () => {
                  remove(scope, false);
                },
              },
              closeLabel: m.close(),
              countdown: false,
              priority: "high",
              timeout: 0,
              title: m.cal_error_too_many_to_notify(),
              tone: "danger",
            });
          }
        },
        onSuccess: onClose,
      }
    );
  };

  const askDelete = (): void => {
    setConfirm(detail.recurring ? "scope" : "delete");
  };

  useShortcut("e", (event) => {
    if (!isInOverlay(event.target) && editable) {
      onEdit();
    }
  });
  useShortcut("#", (event) => {
    if (!isInOverlay(event.target) && writable) {
      askDelete();
    }
  });
  useShortcut("escape", (event) => {
    if (!isInOverlay(event.target)) {
      onClose();
    }
  });

  const when = formatWhen(detail, zone, locale);
  const elsewhere = detail.allDay
    ? null
    : formatInZone(detail, detail.timeZone, zone, locale);

  return (
    <>
      <div {...stylex.props(panelStyles.header)}>
        <IconButton
          icon={ArrowLeft}
          label={m.cal_back()}
          onClick={onClose}
          shortcut={["Esc"]}
        />
        <span {...stylex.props(panelStyles.spacer)} />
        {editable ? (
          <IconButton
            icon={PenLine}
            label={m.cal_edit()}
            onClick={onEdit}
            shortcut={["E"]}
          />
        ) : null}
        {writable ? (
          <IconButton
            icon={Trash2}
            label={m.cal_delete()}
            onClick={askDelete}
            shortcut={["#"]}
          />
        ) : null}
      </div>
      <div {...stylex.props(panelStyles.body)}>
        <div {...stylex.props(styles.titleRow, colorVars.color(detail.color))}>
          <span aria-hidden {...stylex.props(styles.colorBlock)} />
          <div {...stylex.props(styles.itemText)}>
            <h2 dir="auto" {...stylex.props(styles.title)}>
              {title}
            </h2>
            {detail.status === "cancelled" ? (
              <span {...stylex.props(styles.cancelled)}>
                {m.cal_status_cancelled()}
              </span>
            ) : null}
          </div>
        </div>
        <div {...stylex.props(styles.item)}>
          <Clock aria-hidden size={16} {...stylex.props(styles.itemIcon)} />
          <div {...stylex.props(styles.itemText)}>
            <span>{detail.allDay ? `${when} · ${m.cal_all_day()}` : when}</span>
            {elsewhere === null ? null : (
              <span {...stylex.props(styles.muted)}>{elsewhere}</span>
            )}
            {recurrence === null ? null : (
              <span {...stylex.props(styles.muted)}>
                <Repeat aria-hidden size={12} /> {recurrence}
              </span>
            )}
          </div>
        </div>
        {detail.location === null ? null : (
          <div {...stylex.props(styles.item)}>
            <MapPin aria-hidden size={16} {...stylex.props(styles.itemIcon)} />
            <span dir="auto">
              <SafeText text={detail.location} />
            </span>
          </div>
        )}
        {detail.links.map((link) => (
          <div key={link.uri} {...stylex.props(styles.item)}>
            <Video aria-hidden size={16} {...stylex.props(styles.itemIcon)} />
            <Button
              onClick={(event) => {
                links.handleOpen(
                  link.uri,
                  link.name ?? link.uri,
                  event.currentTarget
                );
              }}
              size="sm"
              variant="secondary"
            >
              {link.name === null ? (
                m.cal_join()
              ) : (
                <SafeText text={m.cal_join_named({ name: link.name })} />
              )}
            </Button>
          </div>
        ))}
        {mine === undefined ? null : (
          <AnswerBox detail={detail} status={mine.status} />
        )}
        {detail.participants.length > 0 ? (
          <div {...stylex.props(styles.item)}>
            <Users aria-hidden size={16} {...stylex.props(styles.itemIcon)} />
            <ul
              {...stylex.props(shared.resetList, styles.people)}
              aria-label={m.cal_guests()}
            >
              {detail.participants.map((person) => (
                <li key={person.key} {...stylex.props(styles.person)}>
                  <Avatar name={person.name ?? person.email} size={28} />
                  <span {...stylex.props(styles.personName)}>
                    <span {...stylex.props(shared.ellipsis)}>
                      <SafeText text={person.name ?? person.email} />
                      {person.isMe ? ` ${m.cal_you()}` : ""}
                    </span>
                    {person.name === null ? null : (
                      <span {...stylex.props(styles.muted, shared.ellipsis)}>
                        <SafeText text={person.email} />
                      </span>
                    )}
                  </span>
                  {person.isOrganizer ? (
                    <Badge tone="brand">{m.cal_organizer()}</Badge>
                  ) : (
                    <Badge tone={statusTone(person.status)}>
                      {statusText(person.status)}
                    </Badge>
                  )}
                </li>
              ))}
            </ul>
          </div>
        ) : null}
        {detail.alerts.length > 0 ? (
          <div {...stylex.props(styles.item)}>
            <Bell aria-hidden size={16} {...stylex.props(styles.itemIcon)} />
            <span>
              {new Intl.ListFormat(locale, { type: "conjunction" }).format(
                detail.alerts.map((alert) => alertText(alert.minutesBefore))
              )}
            </span>
          </div>
        ) : null}
        {calendar === undefined ? null : (
          <div {...stylex.props(styles.item)}>
            <CalendarDays
              aria-hidden
              size={16}
              {...stylex.props(styles.itemIcon)}
            />
            <span>
              <SafeText text={calendar.name} />
            </span>
          </div>
        )}
        {detail.description === null ? null : (
          <EventDescription
            html={detail.descriptionIsHtml}
            text={detail.description}
            title={m.cal_description_of({ title })}
          />
        )}
      </div>
      <ScopeDialog
        onCancel={() => {
          setConfirm(null);
        }}
        onChoose={remove}
        open={confirm === "scope"}
        title={m.cal_delete_repeating()}
      />
      {confirm === "delete" ? (
        <AlertDialog
          cancelLabel={m.cancel()}
          confirmLabel={m.cal_delete()}
          description={deleteBody(notifyGuests, mine !== undefined)}
          onConfirm={() => {
            remove("all");
          }}
          onOpenChange={(open) => {
            if (!open) {
              setConfirm(null);
            }
          }}
          open
          title={m.cal_delete_title({ title })}
          tone="danger"
        />
      ) : null}
    </>
  );
};

export const EventPanel = ({
  onClose,
  onEdit,
  target,
}: {
  readonly target: EventTarget;
  readonly onClose: () => void;
  readonly onEdit: () => void;
}) => {
  const query = useEventDetail(target);
  if (query.isPending) {
    return (
      <div {...stylex.props(panelStyles.center)}>
        <Spinner label={m.cal_loading()} />
      </div>
    );
  }
  if (query.data === null || query.data === undefined) {
    return (
      <>
        <div {...stylex.props(panelStyles.header)}>
          <IconButton icon={ArrowLeft} label={m.cal_back()} onClick={onClose} />
        </div>
        <div {...stylex.props(panelStyles.center)}>
          <p>{query.isError ? m.cal_error_load() : m.cal_event_gone()}</p>
        </div>
      </>
    );
  }
  return <Details data={query.data} onClose={onClose} onEdit={onEdit} />;
};
