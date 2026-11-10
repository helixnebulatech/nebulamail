// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Settings › Vacation response: on/off, optional dates, subject and a
// message written in the composer's editor (sanitized like any HTML we
// send). The plain-text version is generated from it.

import { parseDate } from "@internationalized/date";
import type { CalendarDate } from "@internationalized/date";
import { sanitizeComposeHtml } from "@nebulamail/mail-render";
import {
  Button,
  DateLocale,
  DatePicker,
  Field,
  Input,
  RichTextEditor,
  Spinner,
  Switch,
  useToast,
} from "@nebulamail/ui";
import { colors, font } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useState } from "react";

import { editableBody, htmlToText } from "../../mail/body";
import { m } from "../../paraglide/messages.js";
import {
  useSaveVacation,
  useSupport,
  useVacation,
} from "../../settings/queries";
import type { Vacation, VacationProblem } from "../../settings/vacation/api";
import { VACATION_LIMITS, vacationProblems } from "../../settings/vacation/api";
import { useDateLocale, useWeekStart } from "../calendar/use-calendar-prefs";
import { bp } from "../mail/breakpoints.stylex";
import { editorLabels } from "../mail/editor-labels";
import { settingsMeasure } from "./measure.stylex";
import { Notice, SectionHeader, SettingGroup } from "./section";

const styles = stylex.create({
  form: {
    display: "flex",
    flexDirection: "column",
    gap: "4px",
    maxWidth: settingsMeasure.width,
  },
  dates: {
    display: "grid",
    gap: "12px",
    gridTemplateColumns: { default: "1fr 1fr", [bp.mobile]: "1fr" },
  },
  editor: {
    borderColor: colors.line2,
    borderRadius: "9px",
    borderStyle: "solid",
    borderWidth: "1px",
    display: "flex",
    flexDirection: "column",
    minHeight: "180px",
    overflow: "hidden",
  },
  label: {
    color: colors.fg2,
    fontSize: font.sizeSm,
    fontWeight: font.weightButton,
  },
  hint: { color: colors.fg3, fontSize: font.sizeSm, margin: 0 },
  dateRow: { alignItems: "flex-end", display: "flex", gap: "6px" },
  dateField: { flexGrow: 1, minWidth: 0 },
  over: { color: colors.danger },
  footer: { display: "flex", justifyContent: "flex-end", paddingBlock: "16px" },
});

const PROBLEMS: Record<VacationProblem, () => string> = {
  bodyTooLong: m.vacation_problem_body,
  datesOrder: m.vacation_problem_dates,
  subjectTooLong: m.vacation_problem_subject,
};

/** From this share of the size limit on, the message says how much is left. */
const NEAR_LIMIT = 0.8;

const bytes = (text: string): number =>
  new TextEncoder().encode(text).byteLength;

/** The editor's empty document is still `<p></p>`. */
const isEmptyHtml = (html: string): boolean =>
  html.replaceAll(/<[^>]*>/gu, "").trim() === "";

const cleanBody = (html: string): string =>
  isEmptyHtml(html) ? "" : sanitizeComposeHtml(html);

/** `YYYY-MM-DD` ↔ a calendar day ("" is none). */
const toCalendarDate = (day: string): CalendarDate | null => {
  try {
    return day === "" ? null : parseDate(day);
  } catch {
    return null;
  }
};

/** An optional day: React Aria's date picker and a "Clear" button. */
const OptionalDate = ({
  label,
  min,
  onChange,
  value,
}: {
  readonly label: string;
  readonly value: string;
  readonly min?: string;
  readonly onChange: (day: string) => void;
}) => {
  const [, weekStart] = useWeekStart();
  const minValue = min === undefined ? null : toCalendarDate(min);
  const date = toCalendarDate(value);
  return (
    <div {...stylex.props(styles.dateRow)}>
      <div {...stylex.props(styles.dateField)}>
        <DatePicker
          calendarLabel={m.cal_pick_date()}
          isInvalid={
            date !== null && minValue !== null && date.compare(minValue) < 0
          }
          label={label}
          nextLabel={m.cal_next_month()}
          onChange={(next) => {
            onChange(next === null ? "" : next.toString());
          }}
          previousLabel={m.cal_previous_month()}
          value={date}
          {...(minValue === null ? {} : { minValue })}
          {...(weekStart === undefined ? {} : { firstDayOfWeek: weekStart })}
        />
      </div>
      {date === null ? null : (
        <Button
          onClick={() => {
            onChange("");
          }}
          size="sm"
          variant="ghost"
        >
          {m.vacation_clear_date({ field: label })}
        </Button>
      )}
    </div>
  );
};

const bodyOf = (vacation: Vacation): string =>
  editableBody(
    vacation.settings.htmlBody || null,
    vacation.settings.textBody || null
  );

const VacationForm = ({
  onSaved,
  vacation,
}: {
  readonly vacation: Vacation;
  readonly onSaved: () => void;
}) => {
  const locale = useDateLocale();
  const toast = useToast();
  const save = useSaveVacation();
  const [enabled, setEnabled] = useState(vacation.enabled);
  const [fromDate, setFromDate] = useState(vacation.settings.fromDate);
  const [toDate, setToDate] = useState(vacation.settings.toDate);
  const [subject, setSubject] = useState(vacation.settings.subject);
  const [html, setHtml] = useState(() => bodyOf(vacation));
  // What the fields started from, and the editor's turn to start again.
  const [base, setBase] = useState(vacation);
  const [editorKey, setEditorKey] = useState(0);

  // New values from the server (another client, a change pushed): shown
  // unless the user has started editing, which must never be lost.
  const edited =
    enabled !== base.enabled ||
    fromDate !== base.settings.fromDate ||
    toDate !== base.settings.toDate ||
    subject !== base.settings.subject ||
    cleanBody(html) !== cleanBody(bodyOf(base));
  if (vacation !== base && !edited) {
    setBase(vacation);
    setEnabled(vacation.enabled);
    setFromDate(vacation.settings.fromDate);
    setToDate(vacation.settings.toDate);
    setSubject(vacation.settings.subject);
    setHtml(bodyOf(vacation));
    setEditorKey((key) => key + 1);
  }

  const cleanHtml = cleanBody(html);
  const settings = {
    fromDate,
    htmlBody: cleanHtml,
    subject,
    textBody: cleanHtml === "" ? "" : htmlToText(cleanHtml),
    toDate,
  };
  const problems = vacationProblems(settings);
  const used = Math.max(bytes(settings.htmlBody), bytes(settings.textBody));

  return (
    <form
      noValidate
      onSubmit={(event) => {
        event.preventDefault();
        if (problems.length > 0) {
          return;
        }
        save.mutate(
          { enabled, settings },
          {
            onError: () => {
              toast.show({ title: m.vacation_save_error(), tone: "danger" });
            },
            onSuccess: (resumed) => {
              onSaved();
              toast.show({
                description:
                  resumed === null
                    ? undefined
                    : m.vacation_resumed_script({ name: resumed }),
                title: enabled ? m.vacation_saved_on() : m.vacation_saved_off(),
                tone: "success",
              });
            },
          }
        );
      }}
      {...stylex.props(styles.form)}
    >
      <SettingGroup>
        <Switch
          checked={enabled}
          description={m.vacation_enabled_hint()}
          label={m.vacation_enabled()}
          onCheckedChange={setEnabled}
        />
        {enabled && vacation.pausesScript !== null ? (
          <Notice tone="warn">
            <span>
              {m.vacation_pauses_script({ name: vacation.pausesScript })}
            </span>
          </Notice>
        ) : null}
        {vacation.resumesScript === null ? null : (
          <Notice>
            <span>
              {m.vacation_resumes_script({ name: vacation.resumesScript })}
            </span>
          </Notice>
        )}
      </SettingGroup>
      <SettingGroup label={m.vacation_dates()}>
        <DateLocale locale={locale}>
          <div {...stylex.props(styles.dates)}>
            <OptionalDate
              label={m.vacation_from()}
              onChange={setFromDate}
              value={fromDate}
            />
            <OptionalDate
              label={m.vacation_to()}
              onChange={setToDate}
              value={toDate}
              {...(fromDate === "" ? {} : { min: fromDate })}
            />
          </div>
        </DateLocale>
        <p {...stylex.props(styles.hint)}>{m.vacation_dates_hint()}</p>
      </SettingGroup>
      <SettingGroup>
        <Field label={m.vacation_subject()}>
          <Input
            maxLength={VACATION_LIMITS.subject}
            onChange={(event) => {
              setSubject(event.target.value);
            }}
            placeholder={m.vacation_subject_placeholder()}
            value={subject}
          />
        </Field>
        <span aria-hidden {...stylex.props(styles.label)}>
          {m.vacation_message()}
        </span>
        <div {...stylex.props(styles.editor)}>
          <RichTextEditor
            defaultValue={html}
            key={editorKey}
            label={m.vacation_message()}
            labels={editorLabels()}
            onChange={setHtml}
            placeholder={m.vacation_message_placeholder()}
            sanitize={sanitizeComposeHtml}
          />
        </div>
        {/* Only near the limit: a byte count means nothing until then. */}
        {used > VACATION_LIMITS.body * NEAR_LIMIT ? (
          <p
            aria-live="polite"
            {...stylex.props(
              styles.hint,
              used > VACATION_LIMITS.body ? styles.over : null
            )}
          >
            {used > VACATION_LIMITS.body
              ? m.vacation_message_over()
              : m.vacation_message_near({
                  left: String(VACATION_LIMITS.body - used),
                })}
          </p>
        ) : null}
      </SettingGroup>
      {problems.length > 0 ? (
        <Notice tone="danger">
          {problems.map((problem) => (
            <span key={problem}>{PROBLEMS[problem]()}</span>
          ))}
        </Notice>
      ) : null}
      <div {...stylex.props(styles.footer)}>
        <Button
          disabled={problems.length > 0}
          loading={save.isPending}
          type="submit"
          variant="primary"
        >
          {m.vacation_save()}
        </Button>
      </div>
    </form>
  );
};

export const VacationSection = () => {
  const support = useSupport();
  const vacation = useVacation();
  const [saves, setSaves] = useState(0);
  const header = (
    <SectionHeader
      description={m.vacation_description()}
      title={m.settings_section_vacation()}
    />
  );
  if (support.data?.vacation === false) {
    return (
      <>
        {header}
        <Notice>{m.vacation_unsupported()}</Notice>
      </>
    );
  }
  if (vacation.isError) {
    return (
      <>
        {header}
        <Notice tone="danger">
          <span>{m.vacation_load_error()}</span>
        </Notice>
      </>
    );
  }
  if (vacation.data === undefined) {
    return (
      <>
        {header}
        <Spinner label={m.vacation_loading()} />
      </>
    );
  }
  return (
    <>
      {header}
      {/* A fresh form after each save (the server's values). */}
      <VacationForm
        key={saves}
        onSaved={() => {
          setSaves((count) => count + 1);
        }}
        vacation={vacation.data}
      />
    </>
  );
};
