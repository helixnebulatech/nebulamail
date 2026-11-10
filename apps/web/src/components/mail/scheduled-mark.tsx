// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// «Programado · <fecha>»: a message in Sent whose
// submission is still pending with `HOLDUNTIL` hasn't gone out yet. In a
// list row, a short mark instead of the delivery icon; in the open email,
// a line under the header with a link to «Programados» (where it can be
// moved or cancelled). The times come from the «Programados» query
// (`useScheduledSendAt`): no request per row.

import type { EmailId } from "@nebulamail/jmap-client";
import { Badge } from "@nebulamail/ui";
import { colors, font } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { Link } from "@tanstack/react-router";
import { CalendarClock } from "lucide-react";

import { formatSendAt } from "../../mail/schedule";
import { useScheduledSendAt } from "../../mail/use-schedule";
import { m } from "../../paraglide/messages.js";
import { getLocale } from "../../paraglide/runtime.js";
import { DeliveryList } from "./delivery-status";
import { shared } from "./styles";

const styles = stylex.create({
  row: {
    alignItems: "center",
    color: colors.accText,
    columnGap: "3px",
    display: "inline-flex",
    fontWeight: font.weightSemibold,
    whiteSpace: "nowrap",
  },
  icon: { flexShrink: 0 },
  line: {
    alignItems: "center",
    columnGap: "8px",
    display: "flex",
    flexWrap: "wrap",
    fontSize: font.sizeSm,
    marginTop: "8px",
    rowGap: "4px",
  },
  link: {
    color: colors.accText,
    textDecoration: { ":hover": "underline", default: "none" },
  },
});

/** Short date for a list row: "4 nov, 8:00". */
const formatShort = (sendAt: string, locale: string): string => {
  const date = new Date(sendAt);
  return Number.isNaN(date.getTime())
    ? ""
    : new Intl.DateTimeFormat(locale, {
        day: "numeric",
        hour: "numeric",
        minute: "2-digit",
        month: "short",
      }).format(date);
};

/** In a Sent row: the clock and when it goes out. */
export const ScheduledRowMark = ({ sendAt }: { readonly sendAt: string }) => {
  const locale = getLocale();
  const label = m.scheduled_mark({ date: formatShort(sendAt, locale) });
  return (
    <span
      data-scheduled={sendAt}
      title={m.scheduled_mark_title({ date: formatSendAt(sendAt, locale) })}
      {...stylex.props(styles.row)}
    >
      <CalendarClock aria-hidden size={13} {...stylex.props(styles.icon)} />
      {label}
    </span>
  );
};

/** Under the header of the open email, with a link to «Programados». */
export const ScheduledNotice = ({ sendAt }: { readonly sendAt: string }) => {
  const date = formatSendAt(sendAt, getLocale());
  return (
    <div data-scheduled={sendAt} {...stylex.props(styles.line)}>
      <span title={m.scheduled_mark_title({ date })}>
        <Badge icon={CalendarClock} tone="brand">
          {m.scheduled_mark({ date })}
        </Badge>
      </span>
      <Link
        to="/mail/scheduled"
        {...stylex.props(styles.link, shared.focusRing)}
      >
        {m.scheduled_mark_open()}
      </Link>
    </div>
  );
};

/**
 * Under an email of ours: «Programado» while it waits on the server,
 * otherwise what happened to it (delivery-status.tsx). The scheduled
 * query is the one «Programados» uses, shared and cached.
 */
export const SentStatus = ({ emailId }: { readonly emailId: EmailId }) => {
  const sendAt = useScheduledSendAt(true).get(emailId);
  return sendAt === undefined ? (
    <DeliveryList emailId={emailId} />
  ) : (
    <ScheduledNotice sendAt={sendAt} />
  );
};
