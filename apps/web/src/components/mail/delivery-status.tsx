// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// What happened to a sent email (mail/delivery.ts): a small icon in the
// Sent list and, in the open email, one badge per recipient with the
// server's reply for a bounce. Nothing when the server doesn't know.

import type { EmailId } from "@nebulamail/jmap-client";
import { cleanDisplayText } from "@nebulamail/mail-render";
import { Badge, Tooltip } from "@nebulamail/ui";
import type { BadgeTone } from "@nebulamail/ui";
import { colors, font } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { Check, CircleX, Clock, Eye } from "lucide-react";
import type { LucideIcon } from "lucide-react";

import type { DeliveryState, RecipientStatus } from "../../mail/delivery";
import { overallState } from "../../mail/delivery";
import { useDeliveryStatuses } from "../../mail/hooks";
import { m } from "../../paraglide/messages.js";
import { shared } from "./styles";

const styles = stylex.create({
  icon: { flexShrink: 0 },
  delivered: { color: colors.ok },
  failed: { color: colors.danger },
  queued: { color: colors.warn },
  read: { color: colors.info },
  list: {
    alignItems: "center",
    columnGap: "6px",
    display: "flex",
    flexWrap: "wrap",
    fontSize: font.sizeSm,
    marginTop: "8px",
    rowGap: "4px",
  },
  badges: {
    alignItems: "center",
    columnGap: "6px",
    display: "flex",
    flexWrap: "wrap",
    rowGap: "4px",
  },
  term: { color: colors.fg3 },
  reply: {
    color: colors.danger,
    flexBasis: "100%",
    fontSize: font.sizeSm,
    margin: 0,
    overflowWrap: "anywhere",
  },
});

type Shown = Exclude<DeliveryState, "unknown">;

const LOOK: Readonly<
  Record<
    Shown,
    {
      readonly icon: LucideIcon;
      readonly tone: BadgeTone;
      readonly label: () => string;
    }
  >
> = {
  delivered: { icon: Check, label: m.delivery_delivered, tone: "ok" },
  failed: { icon: CircleX, label: m.delivery_failed, tone: "danger" },
  queued: { icon: Clock, label: m.delivery_queued, tone: "warn" },
  read: { icon: Eye, label: m.delivery_read, tone: "info" },
};

const isShown = (state: DeliveryState): state is Shown => state !== "unknown";

/** The icon of a list row (Sent): the state of the whole message. */
export const DeliveryIcon = ({
  recipients,
}: {
  readonly recipients: readonly RecipientStatus[] | undefined;
}) => {
  const state = overallState(recipients ?? []);
  if (!isShown(state)) {
    return null;
  }
  const look = LOOK[state];
  const Glyph = look.icon;
  return (
    <span data-delivery={state} title={look.label()}>
      <Glyph
        aria-hidden
        size={13}
        {...stylex.props(styles.icon, styles[state])}
      />
      <span {...stylex.props(shared.srOnly)}>{look.label()}</span>
    </span>
  );
};

const hint = (status: RecipientStatus): string | null => {
  if (status.reply !== null) {
    return m.delivery_reply({ reply: status.reply });
  }
  if (status.state === "read") {
    return m.delivery_read_hint();
  }
  return status.state === "queued" ? m.delivery_queued_hint() : null;
};

const RecipientBadge = ({ status }: { readonly status: RecipientStatus }) => {
  if (!isShown(status.state)) {
    return null;
  }
  const look = LOOK[status.state];
  const address = cleanDisplayText(status.email);
  const text = hint(status);
  const badge = (
    <span>
      <Badge icon={look.icon} tone={look.tone}>
        <bdi>{address}</bdi>
        <span {...stylex.props(shared.srOnly)}>: {look.label()}</span>
      </Badge>
    </span>
  );
  return (
    <li data-delivery={status.state}>
      {text === null ? badge : <Tooltip content={text}>{badge}</Tooltip>}
    </li>
  );
};

/** Per recipient, under the header of an email we sent. */
export const DeliveryList = ({ emailId }: { readonly emailId: EmailId }) => {
  const query = useDeliveryStatuses([emailId]);
  const recipients = (query.data?.get(emailId) ?? []).filter((status) =>
    isShown(status.state)
  );
  if (recipients.length === 0) {
    return null;
  }
  // Bounce reasons are also written out: a tooltip is easy to miss.
  const bounced = recipients.filter(
    (status) => status.state === "failed" && status.reply !== null
  );
  return (
    <div {...stylex.props(styles.list)}>
      <span aria-hidden {...stylex.props(styles.term)}>
        {m.delivery_label()}
      </span>
      <ul
        aria-label={m.delivery_label()}
        {...stylex.props(shared.resetList, styles.badges)}
      >
        {recipients.map((status) => (
          <RecipientBadge key={status.email} status={status} />
        ))}
      </ul>
      {bounced.map((status) => (
        <p key={status.email} {...stylex.props(styles.reply)}>
          <bdi>{cleanDisplayText(status.email)}</bdi>
          {": "}
          {status.reply}
        </p>
      ))}
    </div>
  );
};
