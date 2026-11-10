// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Event reminders in an open tab. The server pushes a
// CalendarAlert when an alert is due (calendar/alerts.ts); a visible tab
// shows it in a small stack (Open, Snooze, Dismiss); a hidden one also
// shows a system notification when the user turned reminders on (same tag
// as the service worker's, so one replaces the other). Dismiss and snooze
// are stored on the event, so every device stops ringing; other tabs of
// this browser hear about it at once (BroadcastChannel). A task's
// reminder says when it is due, opens it in Tasks and
// offers "Mark done".

import type { CalendarAlert } from "@nebulamail/jmap-client";
import {
  Badge,
  Button,
  IconButton,
  Menu,
  MenuItem,
  Switch,
  useToast,
} from "@nebulamail/ui";
import {
  colors,
  font,
  radius,
  shadow,
  zIndex,
} from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useNavigate } from "@tanstack/react-router";
import { AlarmClock, Bell, Check, Lock, X } from "lucide-react";
import { useEffect, useEffectEvent, useState } from "react";

import type { DueAlert } from "../../calendar/alerts";
import {
  SNOOZE_MINUTES,
  dismissAlert,
  loadAlert,
  snoozeAlert,
  stillRelevant,
  watchAlerts,
} from "../../calendar/alerts";
import { useTimeZone } from "../../calendar/hooks";
import { formatTime } from "../../calendar/time";
import { remindersWanted, useReminders } from "../../calendar/use-reminders";
import { listenForVerification, renewWebPush } from "../../lib/web-push";
import type { NotifyPermission } from "../../mail/use-notifications";
import { m } from "../../paraglide/messages.js";
import { applyTaskChange, completeFromAlert } from "../../tasks/api";
import { refParam } from "../../tasks/links";
import { bp } from "../mail/breakpoints.stylex";
import { taskReplyTexts } from "../tasks/task-format";
import { useOwnTasksAccount } from "../tasks/use-tasks-nav";
import { titleText } from "./event-text";
import { formatWhen, timingOf } from "./format";
import { useDateLocale } from "./use-calendar-prefs";

const styles = stylex.create({
  actions: {
    columnGap: "6px",
    display: "flex",
    flexWrap: "wrap",
    marginTop: "8px",
    rowGap: "6px",
  },
  card: {
    backgroundColor: colors.raised,
    borderColor: colors.line,
    borderInlineStartColor: colors.acc,
    borderInlineStartWidth: "3px",
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: "1px",
    boxShadow: shadow.sh3,
    color: colors.fg,
    paddingBlock: "10px",
    paddingInline: "12px",
  },
  head: { alignItems: "flex-start", columnGap: "8px", display: "flex" },
  labelWithChip: {
    alignItems: "center",
    columnGap: "8px",
    display: "inline-flex",
    flexWrap: "wrap",
    rowGap: "4px",
  },
  icon: { color: colors.accText, flexShrink: 0, marginTop: "2px" },
  stack: {
    display: "flex",
    flexDirection: "column",
    insetInlineStart: { default: "16px", [bp.mobile]: "8px" },
    maxWidth: { default: "360px", [bp.mobile]: "calc(100vw - 16px)" },
    position: "fixed",
    rowGap: "8px",
    top: {
      default: "auto",
      [bp.mobile]: "calc(64px + env(safe-area-inset-top))",
    },
    bottom: { default: "16px", [bp.mobile]: "auto" },
    width: "100%",
    zIndex: zIndex.toast,
  },
  text: { display: "flex", flexDirection: "column", flexGrow: 1, minWidth: 0 },
  title: {
    fontSize: font.sizeMd,
    fontWeight: font.weightSemibold,
    overflowWrap: "anywhere",
  },
  when: { color: colors.fg2, fontSize: font.sizeSm },
});

const CHANNEL = "wm-calendar-alerts";

const hint = (
  permission: NotifyPermission,
  closedTabs: boolean | undefined
) => {
  switch (permission) {
    case "unsupported": {
      return m.cal_reminders_unsupported();
    }
    case "denied": {
      return m.cal_reminders_denied();
    }
    case "default":
    case "granted": {
      return closedTabs === false
        ? m.cal_reminders_hint_open_tab()
        : m.cal_reminders_hint();
    }
    default: {
      return permission satisfies never;
    }
  }
};

/** The "Event reminders" switch (Settings and the calendar's menu). */
export const RemindersControl = () => {
  const reminders = useReminders();
  let blocked: string | null = null;
  if (reminders.permission === "denied") {
    blocked = m.settings_notifications_blocked_chip();
  } else if (reminders.permission === "unsupported") {
    blocked = m.settings_notifications_unsupported_chip();
  }
  return (
    <Switch
      checked={reminders.enabled}
      description={hint(reminders.permission, reminders.closedTabs)}
      disabled={blocked !== null}
      label={
        blocked === null ? (
          m.cal_reminders()
        ) : (
          <span {...stylex.props(styles.labelWithChip)}>
            {m.cal_reminders()}
            <Badge icon={Lock} tone="warn">
              {blocked}
            </Badge>
          </span>
        )
      }
      onCheckedChange={(checked) => {
        void reminders.setEnabled(checked);
      }}
    />
  );
};

const whenText = (alert: DueAlert, zone: string, locale: string): string => {
  if (alert.start === null) {
    return "";
  }
  const when = formatWhen(
    timingOf(
      {
        allDay: alert.allDay,
        end: alert.end ?? alert.start,
        start: alert.start,
      },
      zone
    ),
    zone,
    locale
  );
  if (alert.kind !== "task") {
    return when;
  }
  // A task due on a day: also the time its reminder was set for.
  return alert.allDay && alert.remindAt !== null && alert.remindAt !== undefined
    ? m.tasks_reminder_due_at({
        time: formatTime(alert.remindAt, zone, locale),
        when,
      })
    : m.tasks_reminder_due({ when });
};

/** Where "Open" goes: the event in the calendar, or the task in Tasks. */
const openTarget = (alert: DueAlert, own: string | null) =>
  alert.kind === "task"
    ? ({
        search: { task: refParam(alert.accountId, alert.eventId, own) },
        to: "/tasks",
      } as const)
    : ({
        search: { account: alert.accountId, event: alert.eventId },
        to: "/calendar",
      } as const);

const Reminder = ({
  alert,
  onDone,
}: {
  readonly alert: DueAlert;
  readonly onDone: (key: string) => void;
}) => {
  const zone = useTimeZone();
  const locale = useDateLocale();
  const navigate = useNavigate();
  const toast = useToast();
  const own = useOwnTasksAccount();
  const [busy, setBusy] = useState(false);
  const act = async (run: () => Promise<void>): Promise<void> => {
    setBusy(true);
    try {
      await run();
      onDone(alert.key);
    } catch {
      setBusy(false);
      toast.show({ title: m.cal_error_save(), tone: "danger" });
    }
  };
  const title = titleText(alert.title);
  const isTask = alert.kind === "task";
  const markDone = async (): Promise<void> => {
    const done = await completeFromAlert(alert, zone, taskReplyTexts);
    if (done !== null) {
      toast.show({
        action: {
          label: m.toast_undo(),
          onClick: () => {
            void applyTaskChange(done.undo);
          },
        },
        title: m.tasks_completed_toast({ title }),
      });
    }
    if (done?.replyFailed === true) {
      toast.show({ title: m.tasks_reply_failed(), tone: "danger" });
    }
  };
  return (
    <section
      aria-label={
        isTask ? m.tasks_reminder_of({ title }) : m.cal_reminder_of({ title })
      }
      {...stylex.props(styles.card)}
    >
      <div {...stylex.props(styles.head)}>
        <Bell aria-hidden size={16} {...stylex.props(styles.icon)} />
        <div {...stylex.props(styles.text)}>
          <span dir="auto" {...stylex.props(styles.title)}>
            {title}
          </span>
          <span {...stylex.props(styles.when)}>
            {whenText(alert, zone, locale)}
          </span>
        </div>
        <IconButton
          icon={X}
          label={m.cal_reminder_hide()}
          onClick={() => {
            onDone(alert.key);
          }}
          size="xs"
        />
      </div>
      <div {...stylex.props(styles.actions)}>
        <Button
          onClick={() => {
            onDone(alert.key);
            void navigate(openTarget(alert, own));
          }}
          size="sm"
          variant="secondary"
        >
          {m.cal_reminder_open()}
        </Button>
        {alert.stored ? (
          <>
            <Menu
              trigger={
                <Button
                  disabled={busy}
                  icon={AlarmClock}
                  size="sm"
                  variant="secondary"
                >
                  {m.cal_reminder_snooze()}
                </Button>
              }
            >
              {SNOOZE_MINUTES.map((minutes) => (
                <MenuItem
                  key={minutes}
                  onClick={() => {
                    void act(async () => {
                      await snoozeAlert(alert, minutes);
                    });
                  }}
                >
                  {m.cal_reminder_snooze_for({ minutes })}
                </MenuItem>
              ))}
            </Menu>
            <Button
              {...(isTask ? {} : { icon: Check })}
              loading={busy}
              onClick={() => {
                void act(async () => {
                  await dismissAlert(alert);
                });
              }}
              size="sm"
              variant={isTask ? "secondary" : "primary"}
            >
              {m.cal_reminder_dismiss()}
            </Button>
            {isTask ? (
              <Button
                icon={Check}
                loading={busy}
                onClick={() => {
                  void act(markDone);
                }}
                size="sm"
                variant="primary"
              >
                {m.tasks_reminder_done()}
              </Button>
            ) : null}
          </>
        ) : null}
      </div>
    </section>
  );
};

/** System notification for a hidden tab (when reminders are on). */
const notify = (alert: DueAlert, text: string, open: () => void): void => {
  if (!remindersWanted()) {
    return;
  }
  const title = titleText(alert.title);
  const notification = new Notification(title, { body: text, tag: alert.key });
  notification.addEventListener("click", () => {
    globalThis.focus();
    open();
    notification.close();
  });
};

/**
 * Listens for due reminders while the app is open (mail, calendar,
 * contacts, settings) and shows them. Also passes Web Push verifications
 * on and renews this device's push subscription.
 */
export const CalendarReminders = () => {
  const zone = useTimeZone();
  const locale = useDateLocale();
  const navigate = useNavigate();
  const own = useOwnTasksAccount();
  const [alerts, setAlerts] = useState<readonly DueAlert[]>([]);

  const removeEverywhere = (key: string): void => {
    setAlerts((current) => current.filter((item) => item.key !== key));
    try {
      const channel = new BroadcastChannel(CHANNEL);
      // oxlint-disable-next-line unicorn/require-post-message-target-origin -- a BroadcastChannel has no target origin (same origin only).
      channel.postMessage(key);
      channel.close();
    } catch {
      // Other tabs keep it until closed.
    }
  };

  const show = useEffectEvent(async (raw: CalendarAlert): Promise<void> => {
    const alert = await loadAlert(raw, zone);
    if (alert === null || !stillRelevant(alert, Date.now())) {
      return;
    }
    setAlerts((current) =>
      current.some((item) => item.key === alert.key)
        ? current
        : [...current, alert].slice(-4)
    );
    if (document.visibilityState === "hidden") {
      notify(alert, whenText(alert, zone, locale), () => {
        void navigate(openTarget(alert, own));
      });
    }
  });

  const forget = useEffectEvent((key: string) => {
    setAlerts((current) => current.filter((item) => item.key !== key));
  });

  useEffect(() => {
    const controller = new AbortController();
    // Reminders are a nicety on top of the calendar: without push (or
    // calendars) nothing shows, and nothing breaks.
    watchAlerts((alert) => {
      show(alert).catch(() => null);
    }, controller.signal).catch(() => null);
    const stopVerification = listenForVerification();
    renewWebPush().catch(() => null);
    let channel: BroadcastChannel | null = null;
    try {
      channel = new BroadcastChannel(CHANNEL);
      channel.addEventListener("message", (event: MessageEvent<unknown>) => {
        if (typeof event.data === "string") {
          forget(event.data);
        }
      });
    } catch {
      channel = null;
    }
    return () => {
      controller.abort();
      stopVerification();
      channel?.close();
    };
  }, []);

  if (alerts.length === 0) {
    return null;
  }
  return (
    <section
      aria-label={m.cal_reminders_region()}
      aria-live="assertive"
      {...stylex.props(styles.stack)}
    >
      {alerts.map((alert) => (
        <Reminder alert={alert} key={alert.key} onDone={removeEverywhere} />
      ))}
    </section>
  );
};
