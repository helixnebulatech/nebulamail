// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The send menu's quick times, apart from the "Pick date
// and time" dialog (schedule-send.tsx) and its date picker, which load
// only when it opens. Times are in the user's time zone; anything past the
// server's limit is disabled.

import { MenuGroup, MenuItem } from "@nebulamail/ui";
import { CalendarClock, Clock } from "lucide-react";

import { browserTimeZone } from "../../calendar/time";
import type {
  QuickTime,
  QuickTimeId,
  ScheduleLimit,
} from "../../mail/schedule";
import { checkSendAt, formatSendAt, quickTimes } from "../../mail/schedule";
import { m } from "../../paraglide/messages.js";
import { getLocale } from "../../paraglide/runtime.js";

const QUICK_LABEL: Readonly<Record<QuickTimeId, () => string>> = {
  monday: m.schedule_monday,
  thisAfternoon: m.schedule_this_afternoon,
  tomorrowAfternoon: m.schedule_tomorrow_afternoon,
  tomorrowMorning: m.schedule_tomorrow_morning,
};

export const quickLabel = (id: QuickTimeId): string => QUICK_LABEL[id]();

/** The quick times with whether the server allows each one. */
export const scheduleChoices = (
  now: Date,
  limit: ScheduleLimit
): readonly (QuickTime & { readonly allowed: boolean })[] =>
  quickTimes(now, browserTimeZone()).map((time) => ({
    ...time,
    allowed: checkSendAt(time.at, now, limit) === null,
  }));

export interface ScheduleMenuItemsProps {
  readonly limit: ScheduleLimit;
  /** When the menu was opened (the quick times are counted from it). */
  readonly now: Date;
  readonly onPick: (at: Date) => void;
  readonly onCustom: () => void;
  /** The shortcut that opens the dialog, as shown (⌘⇧↵). */
  readonly shortcut?: readonly string[];
}

/** The send menu's «Programar envío» group. */
export const ScheduleMenuItems = ({
  limit,
  now,
  onCustom,
  onPick,
  shortcut,
}: ScheduleMenuItemsProps) => {
  const locale = getLocale();
  return (
    <MenuGroup label={m.composer_send_later()}>
      {scheduleChoices(now, limit).map((time) => (
        <MenuItem
          disabled={!time.allowed}
          icon={Clock}
          key={time.id}
          onClick={() => {
            onPick(time.at);
          }}
        >
          {quickLabel(time.id)} · {formatSendAt(time.at, locale)}
          {time.allowed ? null : ` (${m.schedule_beyond_limit()})`}
        </MenuItem>
      ))}
      <MenuItem
        icon={CalendarClock}
        onClick={onCustom}
        {...(shortcut === undefined ? {} : { shortcut })}
      >
        {m.schedule_pick()}
      </MenuItem>
    </MenuGroup>
  );
};
