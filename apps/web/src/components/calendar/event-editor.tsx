// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Create or edit an event: title, times with their time zone, all day,
// repetition, place, guests (with their busy times when the server shares
// them), reminders and a plain-text description. ⌘/Ctrl + Enter saves,
// Esc cancels. An HTML description is kept as it is unless the user
// replaces it.

import type { CalendarDate, Time } from "@internationalized/date";
import { parseDate, parseDateTime, parseTime } from "@internationalized/date";
import {
  Badge,
  Button,
  Checkbox,
  DatePicker,
  Field,
  IconButton,
  Input,
  Select,
  Spinner,
  Switch,
  TimeField,
} from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { ArrowLeft, X } from "lucide-react";
import { useState } from "react";
import type { KeyboardEvent } from "react";

import type { EventWithSeries } from "../../calendar/api";
import {
  calendarOfForm,
  changedForm,
  newEventStart,
  prefillAlerts,
} from "../../calendar/defaults";
import type {
  EventForm,
  FormProblem,
  GuestInput,
} from "../../calendar/editing";
import {
  canChangeOneOccurrence,
  formOf,
  newForm,
  rangeMinutes,
  timingOf,
  validateForm,
} from "../../calendar/editing";
import {
  useAvailability,
  useCalendars,
  useEventDetail,
  useMe,
  useTimeZone,
} from "../../calendar/hooks";
import { addressOf } from "../../calendar/itip";
import { holdsEvents } from "../../calendar/model";
import { NO_RECURRENCE } from "../../calendar/recurrence";
import { accountRef } from "../../calendar/targets";
import { MINUTE_MS, timedRange } from "../../calendar/time";
import { useShortcut } from "../../lib/shortcuts";
import { m } from "../../paraglide/messages.js";
import { shared } from "../mail/styles";
import { useKeys } from "../mail/use-apple";
import { panelStyles } from "./event-panel";
import { titleText } from "./event-text";
import { RemindersField, RepeatField } from "./repeat-field";
import { ScopeDialog } from "./scope-dialog";
import { TimeZoneField } from "./time-zone-field";
import type { EventTarget } from "./use-calendar-nav";
import { useWeekStart } from "./use-calendar-prefs";
import type { Scope } from "./use-event-actions";
import { useEventActions } from "./use-event-actions";

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
    paddingInlineStart: "10px",
  },
  chips: { display: "flex", flexWrap: "wrap", gap: "6px", marginTop: "8px" },
  error: { color: colors.danger, fontSize: font.sizeSm, margin: 0 },
  form: { display: "flex", flexDirection: "column", rowGap: "14px" },
  hint: { color: colors.fg3, fontSize: font.sizeSm, margin: 0 },
  row: {
    alignItems: "end",
    columnGap: "8px",
    display: "grid",
    gridTemplateColumns: "minmax(0, 1.4fr) minmax(0, 1fr)",
    rowGap: "8px",
  },
  rowSingle: { gridTemplateColumns: "minmax(0, 1fr)" },
  textarea: {
    backgroundColor: colors.bg,
    borderColor: { ":focus": colors.acc, default: colors.line2 },
    borderRadius: radius.control,
    borderStyle: "solid",
    borderWidth: "1px",
    boxSizing: "border-box",
    color: colors.fg,
    fontFamily: font.sans,
    fontSize: font.sizeMd,
    lineHeight: font.lineHeight,
    minHeight: "96px",
    outline: "none",
    paddingBlock: "8px",
    paddingInline: "12px",
    resize: "vertical",
    width: "100%",
  },
  titleInput: {
    backgroundColor: "transparent",
    borderBottomColor: { ":focus": colors.acc, default: colors.line2 },
    borderBottomStyle: "solid",
    borderBottomWidth: "2px",
    borderInlineWidth: 0,
    borderTopWidth: 0,
    color: colors.fg,
    fontFamily: font.sans,
    fontSize: font.sizeXl,
    fontWeight: font.weightSemibold,
    outline: "none",
    paddingBlock: "6px",
    paddingInline: 0,
    width: "100%",
  },
});

const problemText = (problem: FormProblem): string => {
  switch (problem) {
    case "no-calendar": {
      return m.cal_problem_calendar();
    }
    case "invalid-start": {
      return m.cal_problem_start();
    }
    case "invalid-end": {
      return m.cal_problem_end();
    }
    case "end-before-start": {
      return m.cal_problem_order();
    }
    case "invalid-guest": {
      return m.cal_problem_guest();
    }
    default: {
      return problem satisfies never;
    }
  }
};

const safeDate = (value: string): CalendarDate | null => {
  try {
    return parseDate(value);
  } catch {
    return null;
  }
};

const safeTime = (value: string): Time | null => {
  try {
    return parseTime(value);
  } catch {
    return null;
  }
};

const pad = (value: number): string => String(value).padStart(2, "0");
const timeText = (time: Time): string =>
  `${pad(time.hour)}:${pad(time.minute)}`;

/** Minutes between the form's start and end (to keep the length when the start moves). */
const lengthOf = (form: EventForm): number => {
  try {
    const start = parseDateTime(`${form.startDate}T${form.startTime}`);
    const end = parseDateTime(`${form.endDate}T${form.endTime}`);
    return Math.round(
      (end.toDate("UTC").getTime() - start.toDate("UTC").getTime()) / MINUTE_MS
    );
  } catch {
    return 60;
  }
};

const withStart = (form: EventForm, date: string, time: string): EventForm => {
  if (form.allDay) {
    const first = safeDate(form.startDate);
    const last = safeDate(form.endDate);
    const next = safeDate(date);
    const days = first !== null && last !== null ? last.compare(first) : 0;
    return {
      ...form,
      endDate:
        next === null
          ? form.endDate
          : next.add({ days: Math.max(0, days) }).toString(),
      startDate: date,
    };
  }
  const length = lengthOf(form);
  try {
    const end = parseDateTime(`${date}T${time}`).add({
      minutes: Math.max(0, length),
    });
    return {
      ...form,
      endDate: end.toString().slice(0, 10),
      endTime: `${pad(end.hour)}:${pad(end.minute)}`,
      startDate: date,
      startTime: time,
    };
  } catch {
    return { ...form, startDate: date, startTime: time };
  }
};

/** Plain text of an HTML description (inert document: nothing runs or loads). */
const htmlToText = (html: string): string =>
  new DOMParser().parseFromString(html, "text/html").body.textContent?.trim() ??
  "";

interface EditorProps {
  readonly initial: EventForm;
  readonly existing: EventWithSeries | null;
  readonly onCancel: () => void;
  readonly onSaved: (target: EventTarget) => void;
}

/** Busy guests between the form's start and end (people who share it). */
const useBusyGuests = (form: EventForm): ReadonlySet<string> => {
  const zone = useTimeZone();
  const timing = timingOf(form);
  const range =
    typeof timing === "string"
      ? null
      : timedRange(
          { ...timing, timeZone: form.allDay ? null : form.timeZone },
          zone
        );
  const availability = useAvailability(
    range === null ? [] : form.guests.map((guest) => guest.email),
    range?.start ?? 0,
    range?.end ?? 0
  );
  const busy = new Set<string>();
  for (const [email, periods] of availability.data ?? []) {
    if (
      range !== null &&
      periods?.some(
        (period) => period.start < range.end && period.end > range.start
      ) === true
    ) {
      busy.add(email);
    }
  }
  return busy;
};

interface SectionProps {
  readonly form: EventForm;
  readonly update: (change: Partial<EventForm>) => void;
}

const TimesSection = ({
  form,
  invalidEnd,
  isOccurrence,
  setForm,
  update,
}: SectionProps & {
  readonly invalidEnd: boolean;
  readonly isOccurrence: boolean;
  readonly setForm: (change: (current: EventForm) => EventForm) => void;
}) => {
  const [, firstDayOfWeek] = useWeekStart();
  const startDate = safeDate(form.startDate);
  const weekStart = firstDayOfWeek === undefined ? {} : { firstDayOfWeek };
  const rowStyle = stylex.props(
    styles.row,
    form.allDay ? styles.rowSingle : null
  );
  return (
    <>
      <Switch
        checked={form.allDay}
        label={m.cal_all_day()}
        onCheckedChange={(allDay) => {
          update({ allDay, endDate: allDay ? form.startDate : form.endDate });
        }}
      />
      <div {...rowStyle}>
        <DatePicker
          calendarLabel={m.cal_pick_date()}
          label={m.cal_starts()}
          nextLabel={m.cal_next_month()}
          onChange={(date) => {
            if (date !== null) {
              setForm((current) =>
                withStart(current, date.toString(), current.startTime)
              );
            }
          }}
          previousLabel={m.cal_previous_month()}
          value={startDate}
          {...weekStart}
        />
        {form.allDay ? null : (
          <TimeField
            label={m.cal_start_time()}
            onChange={(time) => {
              if (time !== null) {
                setForm((current) =>
                  withStart(current, current.startDate, timeText(time))
                );
              }
            }}
            value={safeTime(form.startTime)}
          />
        )}
      </div>
      <div {...rowStyle}>
        <DatePicker
          calendarLabel={m.cal_pick_date()}
          isInvalid={invalidEnd}
          label={m.cal_ends()}
          nextLabel={m.cal_next_month()}
          onChange={(date) => {
            if (date !== null) {
              update({ endDate: date.toString() });
            }
          }}
          previousLabel={m.cal_previous_month()}
          value={safeDate(form.endDate)}
          {...(startDate === null ? {} : { minValue: startDate })}
          {...weekStart}
        />
        {form.allDay ? null : (
          <TimeField
            isInvalid={invalidEnd}
            label={m.cal_end_time()}
            onChange={(time) => {
              if (time !== null) {
                update({ endTime: timeText(time) });
              }
            }}
            value={safeTime(form.endTime)}
          />
        )}
      </div>
      {form.allDay || isOccurrence ? null : (
        <TimeZoneField
          label={m.cal_time_zone()}
          noneLabel={m.cal_floating()}
          onChange={(timeZone) => {
            update({ timeZone });
          }}
          value={form.timeZone}
        />
      )}
    </>
  );
};

const RepeatSection = ({ form, update }: SectionProps) => (
  <RepeatField
    onChange={(recurrence) => {
      update({ recurrence });
    }}
    recurrence={form.recurrence}
    startDate={form.startDate}
  />
);

const GuestsSection = ({
  form,
  guestText,
  isNew,
  notify,
  onAdd,
  onGuestText,
  onNotify,
  update,
}: SectionProps & {
  readonly guestText: string;
  readonly isNew: boolean;
  readonly notify: boolean;
  readonly onAdd: (text: string) => void;
  readonly onGuestText: (text: string) => void;
  readonly onNotify: (notify: boolean) => void;
}) => {
  const busy = useBusyGuests(form);
  return (
    <>
      <Field description={m.cal_guests_hint()} label={m.cal_guests()}>
        <Input
          onBlur={() => {
            if (guestText.trim() !== "") {
              onAdd(guestText);
            }
          }}
          onChange={(event) => {
            onGuestText(event.target.value);
          }}
          onKeyDown={(event: KeyboardEvent<HTMLInputElement>) => {
            // ⌘/Ctrl ↵ is "save" (the editor's shortcut): let it through.
            const plainEnter =
              event.key === "Enter" && !event.metaKey && !event.ctrlKey;
            if (plainEnter || event.key === ",") {
              event.preventDefault();
              onAdd(guestText);
            }
          }}
          placeholder={m.cal_guests_placeholder()}
          type="email"
          value={guestText}
        />
      </Field>
      {form.guests.length > 0 ? (
        <ul
          aria-label={m.cal_guests()}
          {...stylex.props(shared.resetList, styles.chips)}
        >
          {form.guests.map((guest) => (
            <li key={guest.email} {...stylex.props(styles.chip)}>
              <span {...stylex.props(shared.ellipsis)}>
                <bdi>{guest.name ?? guest.email}</bdi>
              </span>
              {busy.has(guest.email) ? (
                <Badge tone="warn">{m.cal_busy()}</Badge>
              ) : null}
              <IconButton
                icon={X}
                label={m.cal_remove_guest({ email: guest.email })}
                onClick={() => {
                  update({
                    guests: form.guests.filter(
                      (item) => item.email !== guest.email
                    ),
                  });
                }}
                size="xs"
                tooltip={false}
              />
            </li>
          ))}
        </ul>
      ) : null}
      {form.guests.length > 0 ? (
        <Checkbox
          checked={notify}
          label={isNew ? m.cal_send_invitations() : m.cal_send_updates()}
          onCheckedChange={onNotify}
        />
      ) : null}
    </>
  );
};

const RemindersSection = ({
  form,
  onTouched,
  update,
}: SectionProps & { readonly onTouched: () => void }) => (
  <RemindersField
    alerts={form.alerts}
    label={m.cal_reminders()}
    onChange={(alerts) => {
      onTouched();
      update({ alerts });
    }}
  />
);

const DescriptionSection = ({
  form,
  storedHtml,
  update,
}: SectionProps & { readonly storedHtml: string }) => (
  <Field label={m.cal_description()}>
    {form.keepHtmlDescription ? (
      <div {...stylex.props(styles.form)}>
        <p {...stylex.props(styles.hint)}>{m.cal_description_html()}</p>
        <div>
          <Button
            onClick={() => {
              update({
                description: htmlToText(storedHtml),
                keepHtmlDescription: false,
              });
            }}
            size="sm"
            variant="secondary"
          >
            {m.cal_description_to_text()}
          </Button>
        </div>
      </div>
    ) : (
      <textarea
        dir="auto"
        maxLength={20_000}
        onChange={(event) => {
          update({ description: event.target.value });
        }}
        value={form.description}
        {...stylex.props(styles.textarea)}
      />
    )}
  </Field>
);

const guestsIn = (
  text: string,
  current: readonly GuestInput[],
  myEmail: string | undefined
): { readonly added: GuestInput[]; readonly invalid: boolean } => {
  const found = text
    .split(/[\s,;]+/u)
    .map((part) => part.trim().replaceAll(/^<|>$/gu, ""))
    .filter((part) => part !== "");
  const valid = found.flatMap((part) => {
    const email = addressOf(part);
    return email === null ? [] : [email];
  });
  const known = new Set(current.map((guest) => guest.email));
  return {
    added: [...new Set(valid)]
      .filter((email) => !known.has(email) && email !== myEmail)
      .map((email) => ({ email, name: null, optional: false })),
    invalid: valid.length !== found.length,
  };
};

const Editor = ({ existing, initial, onCancel, onSaved }: EditorProps) => {
  const keys = useKeys();
  const calendars = useCalendars();
  const me = useMe();
  const actions = useEventActions();
  const [form, setForm] = useState<EventForm>(initial);
  const [guestText, setGuestText] = useState("");
  const [problem, setProblem] = useState<FormProblem | null>(null);
  const [askScope, setAskScope] = useState(false);
  const [notify, setNotify] = useState(true);
  // A new event's reminders follow its calendar's defaults until the user
  // changes them (calendar/defaults.ts).
  const [alertsTouched, setAlertsTouched] = useState(false);
  // An event stays in its account: when editing, only that account's
  // calendars; a new one may go to any calendar the user can write to.
  const writable = (calendars.data ?? []).filter(
    (calendar) =>
      calendar.mayWrite &&
      holdsEvents(calendar) &&
      (existing === null || calendar.accountId === existing.detail.accountId)
  );
  const chosenKey = calendarOfForm(writable, form)?.key ?? "";
  const isOccurrence = existing?.detail.isOccurrence === true;

  const update = (change: Partial<EventForm>): void => {
    setForm((current) =>
      changedForm(
        current,
        change,
        writable,
        existing === null && !alertsTouched
      )
    );
    setProblem(null);
  };

  const addGuests = (text: string): void => {
    const { added, invalid } = guestsIn(text, form.guests, me.data?.email);
    if (added.length > 0) {
      update({ guests: [...form.guests, ...added] });
    }
    setGuestText(invalid ? text : "");
    if (invalid) {
      setProblem("invalid-guest");
    }
  };

  const save = (scope: Scope): void => {
    if (guestText.trim() !== "") {
      addGuests(guestText);
      return;
    }
    const found = validateForm(form);
    if (found !== null) {
      setProblem(found);
      return;
    }
    if (me.data === undefined) {
      return;
    }
    const stored =
      existing === null
        ? null
        : {
            alerts: existing.alerts,
            detail: existing.detail,
            locations: existing.locations,
            participants: existing.participants,
            scope,
          };
    actions.save.mutate(
      {
        existing: stored,
        form,
        me: me.data,
        notify: notify && form.guests.length > 0,
      },
      {
        onSuccess: (savedId) => {
          const accountId =
            stored === null
              ? form.accountId
              : accountRef(stored.detail.accountId, calendars.data ?? []);
          onSaved(
            stored !== null && scope === "one" && isOccurrence
              ? {
                  accountId,
                  baseId: stored.detail.baseId,
                  id: stored.detail.id,
                }
              : { accountId, baseId: savedId, id: savedId }
          );
        },
      }
    );
  };

  const submit = (): void => {
    if (isOccurrence) {
      setAskScope(true);
    } else {
      save("all");
    }
  };

  useShortcut("mod+enter", () => {
    submit();
  });
  useShortcut("escape", (event) => {
    if (
      !(event.target instanceof Element) ||
      event.target.closest('[role="dialog"], [role="alertdialog"]') === null
    ) {
      onCancel();
    }
  });

  return (
    <>
      <div {...stylex.props(panelStyles.header)}>
        <IconButton
          icon={ArrowLeft}
          label={m.cancel()}
          onClick={onCancel}
          shortcut={["Esc"]}
        />
        <span {...stylex.props(panelStyles.spacer)} />
        <Button
          loading={actions.save.isPending}
          onClick={submit}
          shortcut={keys("mod+enter")}
          variant="primary"
        >
          {m.cal_save()}
        </Button>
      </div>
      <form
        aria-label={existing === null ? m.cal_new_event() : m.cal_edit_event()}
        onSubmit={(event) => {
          event.preventDefault();
          submit();
        }}
        {...stylex.props(panelStyles.body, styles.form)}
      >
        <input
          aria-label={m.cal_title()}
          // The editor opens to type the title, like every calendar.
          // oxlint-disable-next-line jsx-a11y/no-autofocus
          autoFocus
          dir="auto"
          maxLength={500}
          onChange={(event) => {
            update({ title: event.target.value });
          }}
          placeholder={m.cal_title_placeholder()}
          value={form.title}
          {...stylex.props(styles.titleInput)}
        />
        <TimesSection
          form={form}
          invalidEnd={problem === "end-before-start"}
          isOccurrence={isOccurrence}
          setForm={setForm}
          update={update}
        />
        {isOccurrence ? null : <RepeatSection form={form} update={update} />}
        {isOccurrence || writable.length <= 1 ? null : (
          <Field label={m.cal_calendar()}>
            <Select
              label={m.cal_calendar()}
              onValueChange={(key) => {
                const chosen = writable.find(
                  (calendar) => calendar.key === key
                );
                if (chosen !== undefined) {
                  update({
                    accountId: chosen.owner === null ? null : chosen.accountId,
                    calendarId: chosen.id,
                  });
                }
              }}
              options={writable.map((calendar) => ({
                label:
                  calendar.owner === null
                    ? titleText(calendar.name)
                    : m.cal_calendar_of({
                        name: titleText(calendar.name),
                        owner: titleText(calendar.owner),
                      }),
                value: calendar.key,
              }))}
              value={chosenKey}
            />
          </Field>
        )}
        <Field label={m.cal_location()}>
          <Input
            dir="auto"
            maxLength={500}
            onChange={(event) => {
              update({ location: event.target.value });
            }}
            value={form.location}
          />
        </Field>
        <GuestsSection
          form={form}
          guestText={guestText}
          isNew={existing === null}
          notify={notify}
          onAdd={addGuests}
          onGuestText={(text) => {
            setGuestText(text);
            setProblem(null);
          }}
          onNotify={setNotify}
          update={update}
        />
        <RemindersSection
          form={form}
          onTouched={() => {
            setAlertsTouched(true);
          }}
          update={update}
        />
        <DescriptionSection
          form={form}
          storedHtml={existing?.detail.description ?? ""}
          update={update}
        />
        {problem === null ? null : (
          <p role="alert" {...stylex.props(styles.error)}>
            {problemText(problem)}
          </p>
        )}
      </form>
      <ScopeDialog
        onCancel={() => {
          setAskScope(false);
        }}
        onChoose={(scope) => {
          setAskScope(false);
          save(scope);
        }}
        oneAllowed={
          existing !== null &&
          canChangeOneOccurrence(existing.detail, form.allDay)
        }
        open={askScope}
        title={m.cal_edit_repeating()}
      />
    </>
  );
};

/** Editor for a new event starting at `start` ("2026-10-26T10:00" or a date). */
export const NewEventEditor = ({
  end = null,
  onCancel,
  onSaved,
  start,
}: {
  readonly start: string;
  /** The end of a dragged range (same form as `start`). */
  readonly end?: string | null;
  readonly onCancel: () => void;
  readonly onSaved: (target: EventTarget) => void;
}) => {
  const zone = useTimeZone();
  const calendars = useCalendars();
  if (calendars.data === undefined) {
    return (
      <div {...stylex.props(panelStyles.center)}>
        <Spinner label={m.cal_loading()} />
      </div>
    );
  }
  // The user's own calendars first: shared ones only when chosen.
  const own = calendars.data.filter(
    (item) => item.owner === null && holdsEvents(item)
  );
  const calendar =
    own.find((item) => item.isDefault && item.mayWrite) ??
    own.find((item) => item.mayWrite) ??
    null;
  const allDay = start.length === 10;
  let picked = parseDateTime("2026-01-01T09:00");
  try {
    picked = parseDateTime(allDay ? `${start}T00:00` : start);
  } catch {
    // Keep the default; the URL was edited by hand.
  }
  const minutes = rangeMinutes(picked, end, allDay);
  // In the calendar's zone, if it has one (same instant as picked).
  const { at, zone: eventZone } = newEventStart(
    picked,
    zone,
    calendar?.timeZone,
    allDay
  );
  return (
    <Editor
      existing={null}
      initial={{
        ...newForm(calendar?.id ?? null, at, minutes, eventZone, allDay),
        alerts: prefillAlerts(calendar, allDay),
        recurrence: NO_RECURRENCE,
      }}
      onCancel={onCancel}
      onSaved={onSaved}
    />
  );
};

/** Editor for an existing event or occurrence. */
export const EditEventEditor = ({
  onCancel,
  onSaved,
  target,
}: {
  readonly target: EventTarget;
  readonly onCancel: () => void;
  readonly onSaved: (target: EventTarget) => void;
}) => {
  const query = useEventDetail(target);
  if (query.data === undefined || query.data === null) {
    return (
      <div {...stylex.props(panelStyles.center)}>
        {query.isPending ? (
          <Spinner label={m.cal_loading()} />
        ) : (
          <p>{m.cal_event_gone()}</p>
        )}
      </div>
    );
  }
  return (
    <Editor
      existing={query.data}
      initial={formOf(query.data.detail, query.data.recurrence)}
      onCancel={onCancel}
      onSaved={onSaved}
    />
  );
};
