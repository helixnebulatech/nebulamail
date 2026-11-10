// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Share a calendar or a task list (both are calendars):
// who has access and how, with the folders' share body and the calendar
// levels (calendar/sharing.ts). Offered only when the server lets the
// user share it (`myRights.mayShare`; never through Radicale).

import { cleanDisplayText } from "@nebulamail/mail-render";
import { Button, Dialog, Input, Switch, useToast } from "@nebulamail/ui";
import { colors, font } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { Copy } from "lucide-react";

import { calendarKeys, useCalendars } from "../../calendar/hooks";
import type { CalendarSummary } from "../../calendar/model";
import { useSetPublished } from "../../calendar/nextcloud-hooks";
import type { CalendarShareLevel } from "../../calendar/sharing";
import {
  CALENDAR_SHARE_LEVELS,
  getCalendarShares,
  setCalendarShare,
  shareFailure,
} from "../../calendar/sharing";
import { m } from "../../paraglide/messages.js";
import { ShareBody } from "../mail/share-dialog";

const styles = stylex.create({
  link: { alignItems: "center", columnGap: "8px", display: "flex" },
  linkInput: { flexGrow: 1, minWidth: 0 },
  section: {
    borderStyle: "none",
    display: "flex",
    flexDirection: "column",
    margin: 0,
    marginTop: "20px",
    minWidth: 0,
    padding: 0,
    rowGap: "10px",
  },
  legend: {
    color: colors.fg2,
    fontSize: font.sizeSm,
    fontWeight: font.weightMedium,
    marginBottom: "8px",
    padding: 0,
  },
  warning: { color: colors.fg3, fontSize: font.sizeSm, margin: 0 },
});

/**
 * Publishing at a public link (Nextcloud through the bridge): a switch,
 * the link to copy (never opened here) and what it means. Only where the
 * server lets this calendar be published.
 */
const PublishSection = ({
  calendar,
}: {
  readonly calendar: Pick<CalendarSummary, "accountId" | "id" | "key">;
}) => {
  const toast = useToast();
  const calendars = useCalendars();
  const publish = useSetPublished();
  // The calendar as the server says now (the link comes after publishing).
  const fresh = calendars.data?.find((item) => item.key === calendar.key);
  const publishing = fresh?.publishing ?? null;
  if (publishing === null) {
    return null;
  }
  const { url } = publishing;
  const copyLink = async (link: string): Promise<void> => {
    try {
      await navigator.clipboard.writeText(link);
      toast.show({ title: m.cal_publish_copied() });
    } catch {
      // No clipboard (permissions): the link stays selectable.
    }
  };
  return (
    <fieldset {...stylex.props(styles.section)}>
      <legend {...stylex.props(styles.legend)}>{m.cal_publish_title()}</legend>
      <Switch
        checked={publishing.published}
        disabled={publish.isPending}
        label={m.cal_publish_switch()}
        onCheckedChange={(published) => {
          publish.mutate(
            { calendar, published },
            {
              onError: () => {
                toast.show({ title: m.cal_publish_error(), tone: "danger" });
              },
            }
          );
        }}
      />
      <p {...stylex.props(styles.warning)}>{m.cal_publish_warning()}</p>
      {publishing.published && url !== null ? (
        <div {...stylex.props(styles.link)}>
          <span {...stylex.props(styles.linkInput)}>
            <Input
              aria-label={m.cal_publish_link()}
              dir="ltr"
              onFocus={(event) => {
                event.target.select();
              }}
              readOnly
              value={url}
            />
          </span>
          <Button
            icon={Copy}
            onClick={() => {
              void copyLink(url);
            }}
            variant="secondary"
          >
            {m.cal_publish_copy()}
          </Button>
        </div>
      ) : null}
    </fieldset>
  );
};

/** Free/busy only means nothing for tasks: lists don't offer it. */
const LIST_LEVELS = CALENDAR_SHARE_LEVELS.filter(
  (level) => level !== "freeBusy"
);

export const levelLabel =
  (kind: "calendar" | "list") =>
  (level: CalendarShareLevel | "custom"): string =>
    ({
      custom: m.share_level_custom(),
      freeBusy: m.cal_share_level_free_busy(),
      manage: m.cal_share_level_manage(),
      read:
        kind === "list" ? m.tasks_share_level_read() : m.cal_share_level_read(),
      write: m.cal_share_level_write(),
    })[level];

const errorText = (error: unknown): string => {
  switch (shareFailure(error)) {
    case "forbidden": {
      return m.cal_share_error_forbidden();
    }
    case "invalidProperties": {
      return m.cal_share_error_invalid();
    }
    case "other": {
      return m.share_failed();
    }
    default: {
      return m.share_failed();
    }
  }
};

export const ShareCalendarDialog = ({
  calendar,
  kind = "calendar",
  onOpenChange,
}: {
  readonly calendar: Pick<
    CalendarSummary,
    "accountId" | "id" | "key" | "name"
  > | null;
  /** A task list: its own words and no free/busy level. */
  readonly kind?: "calendar" | "list";
  readonly onOpenChange: (open: boolean) => void;
}) => (
  <Dialog.Root open={calendar !== null} onOpenChange={onOpenChange}>
    <Dialog.Content closeLabel={m.close()}>
      <Dialog.Title>
        {m.cal_share_title({
          name: cleanDisplayText(calendar?.name ?? ""),
        })}
      </Dialog.Title>
      <Dialog.Body>
        {calendar === null ? null : (
          <>
            <ShareBody
              errorText={errorText}
              initialLevel="read"
              invalidateKey={calendarKeys.calendars()}
              levelLabel={levelLabel(kind)}
              levels={kind === "list" ? LIST_LEVELS : CALENDAR_SHARE_LEVELS}
              loadShares={async (people) =>
                await getCalendarShares(calendar, people)
              }
              nobody={
                kind === "list" ? m.tasks_share_nobody() : m.cal_share_nobody()
              }
              note={kind === "list" ? m.tasks_share_note() : m.cal_share_note()}
              saveShare={async (person, level) => {
                await setCalendarShare(calendar, person, level);
              }}
              sharesKey={[
                ...calendarKeys.calendars(),
                "shares",
                calendar.accountId,
                calendar.id,
              ]}
            />
            {kind === "calendar" ? (
              <PublishSection calendar={calendar} />
            ) : null}
          </>
        )}
      </Dialog.Body>
    </Dialog.Content>
  </Dialog.Root>
);
