// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// «Programados»: the messages waiting on the server to go
// out (pending EmailSubmissions), soonest first, with «Cancelar envío»
// (back to Drafts, like undo) and «Cambiar hora» (cancel, then submit
// again with the new time). Only reachable when the account can schedule.
// On the bridge with `sendWhenSignedOut`, each scheduled message also says
// whether it goes out after the user signs out, and lets them change it.

import { cleanDisplayText } from "@nebulamail/mail-render";
import {
  Button,
  EmptyState,
  IllustrationInboxZero,
  Panel,
  Spinner,
  Switch,
  useToast,
} from "@nebulamail/ui";
import { colors, font } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { CalendarClock, X } from "lucide-react";
import { Suspense, lazy, useState } from "react";

import { loadWhenOnline } from "../../lib/load-when-online";
import { getClient } from "../../mail/client";
import { rescheduleEmail, undoSend } from "../../mail/compose";
import type { ScheduleLimit, ScheduledEmail } from "../../mail/schedule";
import {
  formatSendAt,
  latestAllowed,
  scheduleFailure,
} from "../../mail/schedule";
import { setSendWhenSignedOut } from "../../mail/send-signed-out";
import {
  refreshAfterChange,
  scheduleKeys,
  useScheduleSupport,
  useScheduled,
  useSendSignedOutSupport,
} from "../../mail/use-schedule";
import { m } from "../../paraglide/messages.js";
import { getLocale } from "../../paraglide/runtime.js";
import { bp } from "./breakpoints.stylex";
import { shared } from "./styles";

// The date picker (react-aria) loads when a time is first changed.
const ScheduleDialog = lazy(async () => {
  const { ScheduleDialog: Dialog } = await loadWhenOnline(
    async () => await import("./schedule-send")
  );
  return { default: Dialog };
});

const styles = stylex.create({
  actions: { columnGap: "6px", display: "flex", flexShrink: 0 },
  center: {
    alignItems: "center",
    display: "flex",
    flexDirection: "column",
    flexGrow: 1,
    justifyContent: "center",
    padding: "24px",
    rowGap: "12px",
  },
  heading: {
    fontSize: font.sizeXxl,
    fontWeight: 750,
    letterSpacing: "-0.02em",
    margin: 0,
  },
  // The top bar names the page on wider screens (top-bar.tsx): here only
  // for screen readers, except on phones.
  headingBar: {
    alignItems: "center",
    borderBottomColor: colors.line,
    borderBottomStyle: "solid",
    borderBottomWidth: { default: 0, [bp.mobile]: "1px" },
    clip: { default: "rect(0 0 0 0)", [bp.mobile]: "auto" },
    display: "flex",
    height: { default: "1px", [bp.mobile]: "auto" },
    minHeight: { default: 0, [bp.mobile]: "48px" },
    overflow: "hidden",
    paddingInline: { default: 0, [bp.mobile]: "16px" },
    position: { default: "absolute", [bp.mobile]: "static" },
    width: { default: "1px", [bp.mobile]: "auto" },
  },
  list: { listStyle: "none", margin: 0, overflowY: "auto", padding: 0 },
  meta: { color: colors.fg3, fontSize: font.sizeSm },
  panel: { flexGrow: 1, minWidth: 0 },
  row: {
    alignItems: "center",
    borderBottomColor: colors.line,
    borderBottomStyle: "solid",
    borderBottomWidth: "1px",
    columnGap: "12px",
    display: "flex",
    flexWrap: { default: "nowrap", [bp.mobile]: "wrap" },
    paddingBlock: "10px",
    paddingInline: "16px",
    rowGap: "8px",
  },
  signedOut: { fontSize: font.sizeSm, marginTop: "4px" },
  sendAt: {
    color: colors.accText,
    fontSize: font.sizeSm,
    fontWeight: font.weightSemibold,
  },
  subject: {
    color: colors.fg,
    fontWeight: font.weightSemibold,
    textDecoration: { ":hover": "underline", default: "none" },
  },
  text: {
    display: "flex",
    flexDirection: "column",
    flexGrow: 1,
    minWidth: 0,
    rowGap: "2px",
  },
});

/** Whether this message goes out after sign-out, and the switch. */
const SignedOutSwitch = ({ item }: { readonly item: ScheduledEmail }) => {
  const toast = useToast();
  const queryClient = useQueryClient();
  const [busy, setBusy] = useState(false);
  // Shown at once; the list refresh confirms it.
  const [shown, setShown] = useState(item.sendWhenSignedOut);
  const change = async (on: boolean): Promise<void> => {
    setBusy(true);
    setShown(on);
    try {
      await setSendWhenSignedOut(await getClient(), item.submissionId, on);
    } catch {
      setShown(!on);
      toast.show({
        priority: "high",
        title: m.toast_action_failed(),
        tone: "danger",
      });
    }
    setBusy(false);
    await queryClient.invalidateQueries({ queryKey: scheduleKeys.list() });
  };
  return (
    <span {...stylex.props(styles.signedOut)}>
      <Switch
        checked={shown}
        disabled={busy}
        label={m.scheduled_send_signed_out()}
        onCheckedChange={(on) => {
          void change(on);
        }}
      />
    </span>
  );
};

const Row = ({
  item,
  onChange,
  signedOut,
}: {
  readonly item: ScheduledEmail;
  readonly onChange: (item: ScheduledEmail) => void;
  /** The server sends scheduled mail after sign-out (the bridge). */
  readonly signedOut: boolean;
}) => {
  const toast = useToast();
  const queryClient = useQueryClient();
  const [busy, setBusy] = useState(false);
  const subject = cleanDisplayText(item.subject).trim();

  const cancel = async (): Promise<void> => {
    setBusy(true);
    try {
      await undoSend(item);
      toast.show({ title: m.toast_schedule_cancelled(), tone: "info" });
    } catch (error) {
      const kind = scheduleFailure(error)?.kind;
      if (kind === "alreadyCanceled") {
        toast.show({
          title: m.toast_schedule_already_cancelled(),
          tone: "info",
        });
      } else {
        toast.show({
          priority: "high",
          title:
            kind === "alreadySent"
              ? m.toast_schedule_too_late()
              : m.toast_action_failed(),
          tone: "danger",
        });
      }
    }
    setBusy(false);
    await refreshAfterChange(queryClient);
  };

  return (
    <li {...stylex.props(styles.row)}>
      <div {...stylex.props(styles.text)}>
        <Link
          params={{ emailId: item.emailId, mailbox: "sent" }}
          to="/mail/$mailbox/$emailId"
          {...stylex.props(styles.subject, shared.ellipsis, shared.focusRing)}
        >
          {subject === "" ? m.row_no_subject() : subject}
        </Link>
        <span {...stylex.props(styles.meta, shared.ellipsis)}>
          {m.scheduled_to({
            recipients: item.recipients
              .map((email) => cleanDisplayText(email))
              .join(", "),
          })}
        </span>
        <span {...stylex.props(styles.sendAt)}>
          <time dateTime={item.sendAt}>
            {m.scheduled_send_at({
              date: formatSendAt(item.sendAt, getLocale()),
            })}
          </time>
        </span>
        {signedOut && item.holdUntil ? <SignedOutSwitch item={item} /> : null}
      </div>
      <div {...stylex.props(styles.actions)}>
        <Button
          disabled={busy || item.mailFrom === null}
          icon={CalendarClock}
          onClick={() => {
            onChange(item);
          }}
          size="sm"
        >
          {m.scheduled_change()}
        </Button>
        <Button
          icon={X}
          loading={busy}
          onClick={() => {
            void cancel();
          }}
          size="sm"
          variant="ghost"
        >
          {m.scheduled_cancel()}
        </Button>
      </div>
    </li>
  );
};

/** The message for a failed «Cambiar hora». */
const rescheduleErrorText = (
  error: unknown,
  limit: ScheduleLimit | null
): string => {
  const failure = scheduleFailure(error);
  switch (failure?.kind) {
    case "refused": {
      return m.toast_reschedule_refused();
    }
    case "interrupted": {
      return m.toast_reschedule_interrupted();
    }
    case "alreadySent": {
      return m.toast_schedule_too_late();
    }
    case "alreadyCanceled": {
      return m.toast_schedule_already_cancelled();
    }
    case "time": {
      // Checked here before anything was sent: not the server's doing.
      if (failure.problem === "tooFar" && limit !== null) {
        return m.schedule_too_far({
          date: formatSendAt(latestAllowed(new Date(), limit), getLocale()),
        });
      }
      return failure.problem === "past"
        ? m.schedule_past()
        : m.schedule_invalid();
    }
    case undefined: {
      return m.toast_action_failed();
    }
    default: {
      return failure satisfies never;
    }
  }
};

export const ScheduledPage = () => {
  const { limit, loading } = useScheduleSupport();
  const scheduled = useScheduled(limit !== null);
  const signedOut = useSendSignedOutSupport();
  const toast = useToast();
  const queryClient = useQueryClient();
  const [changing, setChanging] = useState<{
    readonly item: ScheduledEmail;
    readonly openedAt: Date;
  } | null>(null);
  const [saving, setSaving] = useState(false);
  // Mounted from the first «Change time» on, so it can animate closing.
  const [dialogUsed, setDialogUsed] = useState(false);
  if (changing !== null && !dialogUsed) {
    setDialogUsed(true);
  }

  const change = async (item: ScheduledEmail, at: Date): Promise<void> => {
    setSaving(true);
    try {
      await rescheduleEmail(item, at);
      setChanging(null);
      toast.show({
        title: m.toast_rescheduled({ date: formatSendAt(at, getLocale()) }),
        tone: "success",
      });
    } catch (error) {
      const kind = scheduleFailure(error)?.kind;
      if (kind === "refused" || kind === "interrupted") {
        setChanging(null);
        await queryClient.invalidateQueries({
          queryKey: scheduleKeys.limit(),
        });
      }
      toast.show({
        priority: "high",
        title: rescheduleErrorText(error, limit),
        tone: "danger",
      });
    }
    setSaving(false);
    await refreshAfterChange(queryClient);
  };

  const body = (() => {
    if (loading || (limit !== null && scheduled.isPending)) {
      return (
        <div {...stylex.props(styles.center)}>
          <Spinner />
        </div>
      );
    }
    if (limit === null) {
      // Reached by URL on a server without FUTURERELEASE (the bridge).
      return (
        <div {...stylex.props(styles.center)}>
          <EmptyState
            illustration={<IllustrationInboxZero />}
            title={m.scheduled_unsupported()}
          >
            {m.scheduled_unsupported_hint()}
          </EmptyState>
        </div>
      );
    }
    if (scheduled.isError) {
      return (
        <div {...stylex.props(styles.center)}>
          <p>{m.scheduled_load_error()}</p>
          <Button
            onClick={() => {
              void scheduled.refetch();
            }}
            size="sm"
          >
            {m.scheduled_retry()}
          </Button>
        </div>
      );
    }
    const items = scheduled.data ?? [];
    if (items.length === 0) {
      return (
        <div {...stylex.props(styles.center)}>
          <EmptyState
            illustration={<IllustrationInboxZero />}
            title={m.scheduled_empty()}
          >
            {m.scheduled_empty_hint()}
          </EmptyState>
        </div>
      );
    }
    return (
      <ul aria-label={m.nav_scheduled()} {...stylex.props(styles.list)}>
        {items.map((item) => (
          <Row
            item={item}
            key={item.submissionId}
            onChange={(picked) => {
              setChanging({ item: picked, openedAt: new Date() });
            }}
            signedOut={signedOut}
          />
        ))}
      </ul>
    );
  })();

  return (
    <Panel label={m.nav_scheduled()} xstyle={styles.panel}>
      <div {...stylex.props(styles.headingBar)}>
        <h1 {...stylex.props(styles.heading)}>{m.nav_scheduled()}</h1>
      </div>
      {body}
      {limit === null || !dialogUsed ? null : (
        <Suspense fallback={null}>
          <ScheduleDialog
            busy={saving}
            limit={limit}
            onClose={() => {
              setChanging(null);
            }}
            onSubmit={(at) => {
              if (changing !== null) {
                void change(changing.item, at);
              }
            }}
            openedAt={changing?.openedAt ?? null}
            submitLabel={m.schedule_change_submit()}
            title={m.schedule_change_title()}
            {...(changing === null ? {} : { initial: changing.item.sendAt })}
          />
        </Suspense>
      )}
    </Panel>
  );
};
