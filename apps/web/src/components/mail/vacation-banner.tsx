// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// A strip above the message list while the vacation response is on (or
// scheduled), like Gmail's: so nobody forgets it running.

import { TextLink } from "@nebulamail/ui";
import { colors, font } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { Link } from "@tanstack/react-router";
import { Plane } from "lucide-react";

import { useNow } from "../../lib/use-now";
import { m } from "../../paraglide/messages.js";
import { getLocale } from "../../paraglide/runtime.js";
import { useSupport, useVacation } from "../../settings/shared-queries";

const styles = stylex.create({
  banner: {
    alignItems: "center",
    backgroundColor: colors.warnSoft,
    borderBottomColor: colors.line,
    borderBottomStyle: "solid",
    borderBottomWidth: "1px",
    color: colors.fg,
    columnGap: "8px",
    display: "flex",
    flexShrink: 0,
    flexWrap: "wrap",
    fontSize: font.sizeMd,
    paddingBlock: "8px",
    paddingInline: "14px",
  },
  icon: { color: colors.warn, flexShrink: 0 },
  text: { flexGrow: 1, minWidth: 0 },
});

const day = (date: Date): string =>
  new Intl.DateTimeFormat(getLocale(), {
    day: "numeric",
    month: "short",
  }).format(date);

export const VacationBanner = () => {
  const support = useSupport();
  const { data } = useVacation();
  const now = useNow();
  if (support.data?.vacation !== true || data === undefined || !data.enabled) {
    return null;
  }
  if (data.to !== null && data.to.getTime() < now) {
    return null;
  }
  let text = m.vacation_banner_on();
  if (data.from !== null && data.from.getTime() > now) {
    text = m.vacation_banner_scheduled({ date: day(data.from) });
  } else if (data.to !== null) {
    text = m.vacation_banner_until({ date: day(data.to) });
  }
  return (
    <output {...stylex.props(styles.banner)}>
      <Plane aria-hidden size={16} {...stylex.props(styles.icon)} />
      <span {...stylex.props(styles.text)}>{text}</span>
      <TextLink render={<Link to="/settings/vacation" />}>
        {m.vacation_banner_manage()}
      </TextLink>
    </output>
  );
};
