// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Create a calendar, change its name, colour, time zone and default
// reminders, or delete it. Colours: the palette (readable
// on light and dark) or any hex colour from the system picker; nothing but
// `#rrggbb` reaches the server.

import type { PrincipalId } from "@nebulamail/jmap-client";
import {
  AlertDialog,
  Button,
  Dialog,
  Field,
  Input,
  useToast,
} from "@nebulamail/ui";
import { colors, font, radius, touch } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { Check } from "lucide-react";
import type { RefObject } from "react";
import { useId, useRef, useState } from "react";

import { EventSaveError, MAX_CONTENTS } from "../../calendar/api";
import { CALENDAR_PALETTE, isSafeColor } from "../../calendar/colors";
import type { CalendarSettings } from "../../calendar/defaults";
import { settingsOf, settingsPatch } from "../../calendar/defaults";
import {
  calendarKeys,
  useCalendarContents,
  useCalendars,
  useCreateCalendar,
  useDeleteCalendar,
  useUpdateCalendar,
} from "../../calendar/hooks";
import type { CalendarSummary } from "../../calendar/model";
import type { CalendarShareLevel } from "../../calendar/sharing";
import {
  CALENDAR_SHARE_LEVELS,
  calendarRightsFor,
} from "../../calendar/sharing";
import { browserTimeZone } from "../../calendar/time";
import { m } from "../../paraglide/messages.js";
import { ShareBody } from "../mail/share-dialog";
import { shared } from "../mail/styles";
import { titleText } from "./event-text";
import { RemindersField } from "./repeat-field";
import { levelLabel } from "./share-calendar-dialog";
import { TimeZoneField } from "./time-zone-field";

const styles = stylex.create({
  custom: {
    alignItems: "center",
    color: colors.fg2,
    columnGap: "8px",
    display: "inline-flex",
    fontSize: font.sizeSm,
    minHeight: { default: "32px", [touch.coarse]: touch.target },
  },
  customInput: {
    backgroundColor: "transparent",
    borderColor: colors.line,
    borderRadius: radius.md,
    borderStyle: "solid",
    borderWidth: "1px",
    cursor: "pointer",
    height: "28px",
    padding: "2px",
    width: "40px",
  },
  error: { color: colors.danger, fontSize: font.sizeSm, margin: 0 },
  footer: { columnGap: "8px", display: "flex", justifyContent: "flex-end" },
  form: { display: "flex", flexDirection: "column", rowGap: "16px" },
  group: {
    borderStyle: "none",
    display: "flex",
    flexDirection: "column",
    margin: 0,
    minWidth: 0,
    padding: 0,
    rowGap: "8px",
  },
  hint: { color: colors.fg3, fontSize: font.sizeSm, margin: 0 },
  legend: {
    color: colors.fg2,
    fontSize: font.sizeSm,
    fontWeight: font.weightMedium,
    marginBottom: "8px",
    padding: 0,
  },
  palette: {
    borderStyle: "none",
    margin: 0,
    minWidth: 0,
    padding: 0,
  },
  swatch: {
    alignItems: "center",
    backgroundColor: "var(--swatch)",
    borderColor: { default: "transparent", ":hover": colors.fg3 },
    borderRadius: radius.pill,
    borderStyle: "solid",
    borderWidth: "2px",
    boxSizing: "border-box",
    color: "#FFFFFF",
    display: "inline-flex",
    height: { default: "28px", [touch.coarse]: touch.target },
    justifyContent: "center",
    width: { default: "28px", [touch.coarse]: touch.target },
  },
  swatchChosen: {
    borderColor: colors.fg,
    boxShadow: `0 0 0 2px ${colors.bg} inset`,
  },
  swatches: { display: "flex", flexWrap: "wrap", gap: "8px" },
});

const swatchVars = stylex.create({
  color: (color: string) => ({ "--swatch": color }),
});

const [FALLBACK] = CALENDAR_PALETTE;

/** Palette + "Custom" (the system colour picker). */
export const ColorChooser = ({
  onChange,
  value,
}: {
  readonly value: string;
  readonly onChange: (color: string) => void;
}) => {
  const inPalette = CALENDAR_PALETTE.some(
    (color) => color.toLowerCase() === value.toLowerCase()
  );
  return (
    <fieldset {...stylex.props(styles.palette)}>
      <legend {...stylex.props(styles.legend)}>{m.cal_color()}</legend>
      <div {...stylex.props(styles.swatches)}>
        {CALENDAR_PALETTE.map((color, index) => {
          const chosen = color.toLowerCase() === value.toLowerCase();
          return (
            <button
              aria-pressed={chosen}
              aria-label={m.cal_color_n({ n: index + 1 })}
              key={color}
              onClick={() => {
                onChange(color);
              }}
              type="button"
              {...stylex.props(
                shared.resetButton,
                shared.focusRing,
                styles.swatch,
                chosen ? styles.swatchChosen : null,
                swatchVars.color(color)
              )}
            >
              {chosen ? <Check aria-hidden size={14} /> : null}
            </button>
          );
        })}
        <label {...stylex.props(styles.custom)}>
          <input
            onChange={(event) => {
              if (isSafeColor(event.target.value)) {
                onChange(event.target.value);
              }
            }}
            type="color"
            value={isSafeColor(value) && value.length === 7 ? value : FALLBACK}
            {...stylex.props(styles.customInput)}
          />
          {inPalette ? m.cal_color_custom() : m.cal_color_custom_chosen()}
        </label>
      </div>
    </fieldset>
  );
};

/** Who a new calendar is shared with: person → level, until it's made. */
type Drafts = RefObject<Map<PrincipalId, CalendarShareLevel>>;

/**
 * Sharing a calendar that doesn't exist yet: the same body as sharing
 * one, kept here and sent with the create (`shareWith`). Changes stay in
 * the dialog (a ref read when saving), never on the server.
 */
const NewCalendarShares = ({ drafts }: { readonly drafts: Drafts }) => {
  const id = useId();
  const key = [...calendarKeys.all, "new-shares", id];
  return (
    <fieldset {...stylex.props(styles.group)}>
      <legend {...stylex.props(styles.legend)}>
        {m.cal_new_share_title()}
      </legend>
      <ShareBody
        addedText={(name) => m.cal_new_share_added({ name })}
        initialLevel="read"
        invalidateKey={key}
        levelLabel={levelLabel("calendar")}
        levels={CALENDAR_SHARE_LEVELS}
        loadShares={async (people) =>
          await Promise.resolve(
            [...drafts.current].map(([person, level]) => ({
              level,
              person: people.find((known) => known.id === person) ?? {
                email: null,
                id: person,
                name: person,
                type: "individual",
              },
            }))
          )
        }
        nobody={m.cal_new_share_nobody()}
        note={m.cal_share_note()}
        saveShare={async (person, level) => {
          if (level === null) {
            drafts.current.delete(person);
          } else {
            drafts.current.set(person, level);
          }
          await Promise.resolve();
        }}
        sharesKey={key}
      />
    </fieldset>
  );
};

const errorText = (error: unknown): string => {
  if (!(error instanceof EventSaveError)) {
    return m.cal_error_save();
  }
  if (error.properties.includes("shareWith")) {
    return m.cal_error_new_share();
  }
  if (error.type === "forbidden") {
    return m.cal_error_forbidden();
  }
  if (error.type === "invalidProperties") {
    if (error.properties.includes("timeZone")) {
      return m.cal_error_time_zone();
    }
    if (
      error.properties.some((property) =>
        property.startsWith("defaultAlerts")
      ) ||
      error.properties.includes("trigger")
    ) {
      return m.cal_error_default_alerts();
    }
  }
  return m.cal_error_save();
};

/**
 * Time zone and default reminders of the user's own calendars. The
 * reminders only where the server keeps them (Stalwart and the bridge do).
 */
const DefaultsSection = ({
  onChange,
  settings,
  withAlerts,
}: {
  readonly settings: CalendarSettings;
  readonly onChange: (settings: CalendarSettings) => void;
  readonly withAlerts: boolean;
}) => (
  <>
    <TimeZoneField
      description={m.cal_calendar_zone_hint()}
      label={m.cal_time_zone()}
      noneLabel={m.cal_calendar_zone_none()}
      onChange={(timeZone) => {
        onChange({ ...settings, timeZone });
      }}
      value={settings.timeZone}
    />
    {withAlerts ? (
      <fieldset {...stylex.props(styles.group)}>
        <legend {...stylex.props(styles.legend)}>
          {m.cal_default_alerts()}
        </legend>
        <p {...stylex.props(styles.hint)}>{m.cal_default_alerts_hint()}</p>
        <RemindersField
          alerts={settings.withTime}
          label={m.cal_default_alerts_timed()}
          onChange={(withTime) => {
            onChange({ ...settings, withTime });
          }}
        />
        <RemindersField
          alerts={settings.withoutTime}
          label={m.cal_default_alerts_all_day()}
          onChange={(withoutTime) => {
            onChange({ ...settings, withoutTime });
          }}
        />
      </fieldset>
    ) : null}
  </>
);

const hasAlerts = (item: CalendarSummary): boolean =>
  item.defaultAlertsWithTime !== null &&
  item.defaultAlertsWithTime !== undefined;

/**
 * What the calendar form offers. A new calendar can be shared from the
 * start where the user can share the calendars they have (the server
 * decides per calendar). Name, zone and reminders belong to the owner;
 * others' calendars only get a colour of the user's own. A
 * subscription (Nextcloud) has a name and a colour: its events and their
 * zone come from elsewhere. Reminders: for a new calendar, if the user's
 * other calendars have default alerts.
 */
const formShape = (
  calendar: CalendarSummary | null,
  calendars: readonly CalendarSummary[]
) => {
  const canRename = calendar === null || calendar.owner === null;
  const subscription = calendar?.subscription ?? null;
  return {
    canRename,
    canShareNew:
      calendar === null &&
      calendars.some((item) => item.owner === null && item.mayShare === true),
    subscription,
    withAlerts:
      calendar === null
        ? calendars.some((item) => item.owner === null && hasAlerts(item))
        : hasAlerts(calendar),
    withDefaults: canRename && subscription === null,
  };
};

const CalendarForm = ({
  calendar,
  onDone,
}: {
  readonly calendar: CalendarSummary | null;
  readonly onDone: () => void;
}) => {
  const toast = useToast();
  const calendars = useCalendars();
  const create = useCreateCalendar();
  const update = useUpdateCalendar();
  const [name, setName] = useState(calendar?.name ?? "");
  const [color, setColor] = useState(calendar?.color ?? FALLBACK);
  const [settings, setSettings] = useState(() =>
    calendar === null
      ? { ...settingsOf(null), timeZone: browserTimeZone() }
      : settingsOf(calendar)
  );
  const [error, setError] = useState<string | null>(null);
  const drafts = useRef(new Map<PrincipalId, CalendarShareLevel>());
  const { canRename, canShareNew, subscription, withAlerts, withDefaults } =
    formShape(calendar, calendars.data ?? []);
  const busy = create.isPending || update.isPending;

  const submit = (): void => {
    if (canRename && name.trim() === "") {
      setError(m.cal_calendar_name_missing());
      return;
    }
    // Here, not while drawing: the alerts get new random ids.
    const patch = withDefaults
      ? settingsPatch(
          calendar,
          settingsOf(calendar),
          withAlerts ? settings : { ...settings, withTime: [], withoutTime: [] }
        )
      : {};
    if (calendar === null) {
      const shareWith = canShareNew
        ? Object.fromEntries(
            [...drafts.current].map(([person, level]) => [
              person,
              calendarRightsFor(level),
            ])
          )
        : {};
      create.mutate(
        {
          color,
          name,
          // "No zone" too, which isn't a change from `settingsOf(null)`.
          settings: withDefaults
            ? { ...patch, timeZone: settings.timeZone }
            : patch,
          shareWith,
        },
        {
          onError: (failure) => {
            setError(errorText(failure));
          },
          onSuccess: () => {
            toast.show({ title: m.cal_calendar_created() });
            onDone();
          },
        }
      );
      return;
    }
    update.mutate(
      {
        calendar,
        color,
        ...(canRename && name.trim() !== calendar.name ? { name } : {}),
        settings: patch,
      },
      {
        // Stays open until saved: a refused zone or reminder is said here.
        onError: (failure) => {
          setError(errorText(failure));
        },
        onSuccess: onDone,
      }
    );
  };

  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        submit();
      }}
      {...stylex.props(styles.form)}
    >
      {canRename ? (
        <Field label={m.cal_calendar_name()}>
          <Input
            // The dialog opens to type the name.
            // oxlint-disable-next-line jsx-a11y/no-autofocus
            autoFocus
            dir="auto"
            maxLength={255}
            onChange={(event) => {
              setName(event.target.value);
              setError(null);
            }}
            value={name}
          />
        </Field>
      ) : null}
      {subscription?.url === null || subscription?.url === undefined ? null : (
        <p {...stylex.props(styles.hint)}>
          {m.cal_subscription_source({ url: subscription.url })}
        </p>
      )}
      <ColorChooser onChange={setColor} value={color} />
      {withDefaults ? (
        <DefaultsSection
          onChange={setSettings}
          settings={settings}
          withAlerts={withAlerts}
        />
      ) : null}
      {canShareNew ? <NewCalendarShares drafts={drafts} /> : null}
      {error === null ? null : (
        <p role="alert" {...stylex.props(styles.error)}>
          {error}
        </p>
      )}
      <div {...stylex.props(styles.footer)}>
        <Button onClick={onDone} variant="secondary">
          {m.cancel()}
        </Button>
        <Button loading={busy} type="submit" variant="primary">
          {calendar === null ? m.cal_create_calendar() : m.cal_save()}
        </Button>
      </div>
    </form>
  );
};

/**
 * New calendar (`calendar` null) or changes to one: name, colour, time
 * zone and default reminders. Others' calendars: only the colour (the rest
 * is theirs).
 */
export const CalendarDialog = ({
  calendar,
  onOpenChange,
  open,
}: {
  readonly calendar: CalendarSummary | null;
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
}) => (
  <Dialog.Root open={open} onOpenChange={onOpenChange}>
    <Dialog.Content closeLabel={m.close()} size="sm">
      <Dialog.Title>
        {calendar === null ? m.cal_new_calendar() : m.cal_edit_calendar()}
      </Dialog.Title>
      <Dialog.Body>
        {open ? (
          <CalendarForm
            calendar={calendar}
            onDone={() => {
              onOpenChange(false);
            }}
          />
        ) : null}
      </Dialog.Body>
    </Dialog.Content>
  </Dialog.Root>
);

/** What deleting a calendar takes with it, once counted. */
const DeleteCalendarText = ({
  contents,
}: {
  readonly contents: ReturnType<typeof useCalendarContents>;
}) => {
  const { data } = contents;
  return (
    <>
      {m.cal_delete_calendar_body()}
      {contents.isPending ? <> {m.cal_delete_calendar_checking()}</> : null}
      {data !== undefined && data.tasks > 0 ? (
        <> {m.cal_delete_calendar_tasks({ count: data.tasks })}</>
      ) : null}
      {data !== undefined && data.others > 0 ? (
        <> {m.cal_delete_calendar_others({ count: data.others })}</>
      ) : null}
      {data !== undefined && !data.complete ? (
        <> {m.cal_delete_calendar_more({ count: MAX_CONTENTS })}</>
      ) : null}
    </>
  );
};

/** "Delete «Work»?" — the calendar and every event in it. */
export const DeleteCalendarDialog = ({
  calendar,
  onOpenChange,
}: {
  readonly calendar: CalendarSummary | null;
  readonly onOpenChange: (open: boolean) => void;
}) => {
  const toast = useToast();
  const remove = useDeleteCalendar();
  const subscription =
    calendar?.subscription !== null && calendar?.subscription !== undefined;
  // Tasks and other apps' objects go with it: say so (a journal entry,
  // VJOURNAL, is never shown here). Not for a subscription: unsubscribing
  // deletes nothing on the other server.
  const contents = useCalendarContents(subscription ? null : calendar);
  if (calendar === null) {
    return null;
  }
  return (
    <AlertDialog
      cancelLabel={m.cancel()}
      confirmLabel={subscription ? m.cal_unsubscribe() : m.cal_delete()}
      description={
        subscription ? (
          m.cal_unsubscribe_body()
        ) : (
          <DeleteCalendarText contents={contents} />
        )
      }
      onConfirm={() => {
        onOpenChange(false);
        remove.mutate(calendar, {
          onError: (failure) => {
            toast.show({ title: errorText(failure), tone: "danger" });
          },
          onSuccess: () => {
            toast.show({
              title: subscription
                ? m.cal_unsubscribed()
                : m.cal_calendar_deleted(),
            });
          },
        });
      }}
      onOpenChange={onOpenChange}
      open
      title={
        subscription
          ? m.cal_unsubscribe_title({ name: titleText(calendar.name) })
          : m.cal_delete_calendar_title({ name: titleText(calendar.name) })
      }
      tone="danger"
    />
  );
};
