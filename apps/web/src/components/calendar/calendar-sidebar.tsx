// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Left side of the calendar: the brand, "New event", a month to jump to a
// day, and the user's calendars with their colours (tick to show or hide;
// stored on the server, so every device sees the same).

import { cleanDisplayText } from "@nebulamail/mail-render";
import {
  Checkbox,
  ComposeButton,
  IconButton,
  Menu,
  MenuItem,
  MiniCalendar,
  Spinner,
} from "@nebulamail/ui";
import { colors, radius, touch } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import {
  CalendarPlus,
  MoreHorizontal,
  Palette,
  PenLine,
  Plus,
  Rss,
  Share2,
  Trash2,
} from "lucide-react";
import { useState } from "react";

import { useCalendars, useToggleCalendar } from "../../calendar/hooks";
import { viewRange } from "../../calendar/layout";
import type { CalendarSummary } from "../../calendar/model";
import { holdsEvents } from "../../calendar/model";
import { useCalendarExtras } from "../../calendar/nextcloud-hooks";
import { m } from "../../paraglide/messages.js";
import { sidebarLayout } from "../mail/sidebar";
import { SidebarSection } from "../mail/sidebar-section";
import { SidebarTop } from "../mail/sidebar-top";
import { shared } from "../mail/styles";
import { ShowTasksToggle } from "../tasks/calendar-tasks";
import { CalendarDialog, DeleteCalendarDialog } from "./calendar-dialogs";
import { SafeText } from "./event-text";
import { SubscribeDialog, TrashDialog } from "./nextcloud-dialogs";
import { ShareCalendarDialog } from "./share-calendar-dialog";
import { useCalendarNav } from "./use-calendar-nav";
import { useDateLocale, useWeekStart } from "./use-calendar-prefs";

const styles = stylex.create({
  calendar: {
    alignItems: "center",
    backgroundColor: { ":hover": colors.hover, default: "transparent" },
    borderRadius: radius.md,
    display: "flex",
    minHeight: "32px",
    minWidth: 0,
    // The box on the icon column (x = 22), under the heading.
    paddingInlineStart: "10px",
    position: "relative",
  },
  calendarCheck: { flexGrow: 1, minWidth: 0, paddingInlineEnd: "8px" },
  // The checkbox's 8px gap leaves the name 2px short of the heading's
  // label column (x = 48).
  calendarName: { marginInlineStart: "2px" },
  // The row's ⋯ lies over the end of the row (no width kept for it, so
  // names aren't cut short): on hover or keyboard focus, like the mail
  // rows' actions; always on touch screens, which have no hover.
  calendarMenu: {
    backgroundColor: colors.hover,
    borderRadius: radius.sm,
    insetInlineEnd: "4px",
    opacity: {
      default: 0,
      [stylex.when.ancestor(":hover")]: 1,
      [stylex.when.ancestor(":focus-within")]: 1,
      [touch.noHover]: 1,
    },
    position: "absolute",
    top: "50%",
    transform: "translateY(-50%)",
  },
  root: {
    boxSizing: "border-box",
    display: "flex",
    flexDirection: "column",
    height: "100%",
    overflowY: "auto",
    paddingBottom: "14px",
    paddingInline: "12px",
    paddingTop: "12px",
    rowGap: "16px",
  },
});

const CalendarList = ({
  calendars,
  onDelete,
  onEdit,
  onShare,
  onToggle,
}: {
  readonly calendars: readonly CalendarSummary[];
  readonly onToggle: (calendar: CalendarSummary, visible: boolean) => void;
  readonly onEdit: (calendar: CalendarSummary) => void;
  readonly onShare: (calendar: CalendarSummary) => void;
  readonly onDelete: (calendar: CalendarSummary) => void;
}) => (
  <ul {...stylex.props(shared.resetList)}>
    {calendars.map((calendar) => (
      <li
        key={calendar.key}
        {...stylex.props(styles.calendar, stylex.defaultMarker())}
      >
        <span {...stylex.props(styles.calendarCheck)}>
          {/* The box itself carries the calendar's colour (no extra dot). */}
          <Checkbox
            checked={calendar.visible}
            label={
              <span
                // The whole name when it doesn't fit.
                title={cleanDisplayText(calendar.name)}
                {...stylex.props(shared.ellipsis, styles.calendarName)}
              >
                <SafeText text={calendar.name} />
              </span>
            }
            onCheckedChange={(visible) => {
              onToggle(calendar, visible);
            }}
            tone={calendar.color}
          />
        </span>
        <span {...stylex.props(styles.calendarMenu)}>
          <Menu
            align="end"
            trigger={
              <IconButton
                icon={MoreHorizontal}
                label={m.cal_calendar_options({
                  name: cleanDisplayText(calendar.name),
                })}
                size="xs"
                tooltip={false}
              />
            }
          >
            <MenuItem
              icon={calendar.owner === null ? PenLine : Palette}
              onClick={() => {
                onEdit(calendar);
              }}
            >
              {calendar.owner === null
                ? m.cal_edit_calendar_menu()
                : m.cal_change_color()}
            </MenuItem>
            {calendar.mayShare === true ? (
              <MenuItem
                icon={Share2}
                onClick={() => {
                  onShare(calendar);
                }}
              >
                {m.cal_share_menu()}
              </MenuItem>
            ) : null}
            {calendar.mayDelete ? (
              <MenuItem
                danger
                icon={Trash2}
                onClick={() => {
                  onDelete(calendar);
                }}
              >
                {calendar.subscription === null ||
                calendar.subscription === undefined
                  ? m.cal_delete_calendar()
                  : m.cal_unsubscribe()}
              </MenuItem>
            ) : null}
          </Menu>
        </span>
      </li>
    ))}
  </ul>
);

export const CalendarSidebar = ({
  onNavigate,
}: {
  /** Called after choosing something (closes the phone drawer). */
  readonly onNavigate?: () => void;
}) => {
  const nav = useCalendarNav();
  const query = useCalendars();
  // Task-only lists (the bridge's) live in Tasks, not here.
  const calendars = {
    ...query,
    data: query.data?.filter((calendar) => holdsEvents(calendar)),
  };
  const toggle = useToggleCalendar();
  const [, firstDayOfWeek] = useWeekStart();
  const locale = useDateLocale();
  const [editing, setEditing] = useState<{
    readonly calendar: CalendarSummary | null;
  } | null>(null);
  const [deleting, setDeleting] = useState<CalendarSummary | null>(null);
  const [sharing, setSharing] = useState<CalendarSummary | null>(null);
  const extras = useCalendarExtras();
  const [extra, setExtra] = useState<"subscribe" | "trash" | null>(null);
  // Calendars others share, grouped by who shares them.
  const owners = Object.entries(
    Object.groupBy(
      (calendars.data ?? []).filter((calendar) => calendar.owner !== null),
      (calendar) => calendar.owner ?? ""
    )
  ).flatMap(([owner, list]) =>
    list === undefined ? [] : [[owner, list] as const]
  );
  return (
    <nav aria-label={m.cal_sidebar()} {...stylex.props(styles.root)}>
      <SidebarTop
        current="calendar"
        onNavigate={onNavigate}
        xstyle={sidebarLayout.composeInGap16}
      >
        <ComposeButton
          icon={CalendarPlus}
          onClick={() => {
            nav.newEventHere();
            onNavigate?.();
          }}
          shortcut={["C"]}
        >
          {m.cal_new_event()}
        </ComposeButton>
      </SidebarTop>
      <MiniCalendar
        label={m.cal_pick_date()}
        monthPicker={{
          choose: m.cal_pick_month(),
          nextYear: m.cal_next_year(),
          previousYear: m.cal_previous_year(),
        }}
        nextLabel={m.cal_next_month()}
        onChange={(date) => {
          nav.goTo(date);
          onNavigate?.();
        }}
        previousLabel={m.cal_previous_month()}
        value={nav.anchor}
        {...(nav.view === "week"
          ? { range: viewRange("week", nav.anchor, locale, firstDayOfWeek) }
          : {})}
        {...(firstDayOfWeek === undefined ? {} : { firstDayOfWeek })}
      />
      <section>
        <SidebarSection
          action={
            <>
              {extras.subscriptions || extras.trash ? (
                <Menu
                  align="end"
                  trigger={
                    <IconButton
                      icon={MoreHorizontal}
                      label={m.cal_more_options()}
                      size="xs"
                      tooltip={false}
                    />
                  }
                >
                  {extras.subscriptions ? (
                    <MenuItem
                      icon={Rss}
                      onClick={() => {
                        setExtra("subscribe");
                      }}
                    >
                      {m.cal_subscribe_menu()}
                    </MenuItem>
                  ) : null}
                  {extras.trash ? (
                    <MenuItem
                      icon={Trash2}
                      onClick={() => {
                        setExtra("trash");
                      }}
                    >
                      {m.cal_trash_menu()}
                    </MenuItem>
                  ) : null}
                </Menu>
              ) : null}
              <IconButton
                icon={Plus}
                label={m.cal_new_calendar()}
                onClick={() => {
                  setEditing({ calendar: null });
                }}
                size="xs"
              />
            </>
          }
          flush
          onToggle={null}
          open
        >
          {m.cal_my_calendars()}
        </SidebarSection>
        {calendars.isError ? <p>{m.cal_error_load()}</p> : null}
        {calendars.isPending ? <Spinner label={m.cal_loading()} /> : null}
        {calendars.data === undefined ? null : (
          <CalendarList
            calendars={calendars.data.filter(
              (calendar) => calendar.owner === null
            )}
            onDelete={setDeleting}
            onEdit={(calendar) => {
              setEditing({ calendar });
            }}
            onShare={setSharing}
            onToggle={(calendar, visible) => {
              toggle.mutate({ calendar, visible });
            }}
          />
        )}
      </section>
      {owners.map(([owner, list]) => (
        <section aria-label={m.cal_shared_by({ owner })} key={owner}>
          <SidebarSection flush onToggle={null} open>
            <SafeText text={m.cal_shared_by({ owner })} />
          </SidebarSection>
          <CalendarList
            calendars={list}
            onDelete={setDeleting}
            onEdit={(calendar) => {
              setEditing({ calendar });
            }}
            onShare={setSharing}
            onToggle={(calendar, visible) => {
              toggle.mutate({ calendar, visible });
            }}
          />
        </section>
      ))}
      <ShowTasksToggle />
      <CalendarDialog
        calendar={editing?.calendar ?? null}
        onOpenChange={(open) => {
          if (!open) {
            setEditing(null);
          }
        }}
        open={editing !== null}
      />
      <DeleteCalendarDialog
        calendar={deleting}
        onOpenChange={(open) => {
          if (!open) {
            setDeleting(null);
          }
        }}
      />
      <ShareCalendarDialog
        calendar={sharing}
        onOpenChange={(open) => {
          if (!open) {
            setSharing(null);
          }
        }}
      />
      <SubscribeDialog
        onOpenChange={(open) => {
          setExtra(open ? "subscribe" : null);
        }}
        open={extra === "subscribe"}
      />
      <TrashDialog
        onOpenChange={(open) => {
          setExtra(open ? "trash" : null);
        }}
        open={extra === "trash"}
      />
    </nav>
  );
};
