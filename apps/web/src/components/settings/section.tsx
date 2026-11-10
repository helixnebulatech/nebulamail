// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Building blocks of the settings sections: a title, groups of settings
// and notices (info, warning, danger).

import { EmptyState } from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { CircleAlert, Info, TriangleAlert } from "lucide-react";
import { useId } from "react";
import type { ReactNode } from "react";

import { bp } from "../mail/breakpoints.stylex";
import { settingsMeasure } from "./measure.stylex";

const styles = stylex.create({
  // 28px under the section title, plus the first group heading's 14px:
  // the same 42px every later heading gets (group padding 14 + 28).
  header: { marginBottom: "28px", maxWidth: settingsMeasure.width },
  title: {
    color: colors.fg,
    fontSize: font.sizeXxl,
    fontWeight: font.weightSemibold,
    letterSpacing: font.trackingTitle,
    lineHeight: 1.25,
    margin: 0,
  },
  description: {
    color: colors.fg3,
    fontSize: font.sizeMd,
    lineHeight: 1.5,
    margin: 0,
    marginTop: "6px",
  },
  subHead: {
    alignItems: "center",
    columnGap: "12px",
    display: "flex",
    justifyContent: "space-between",
    maxWidth: settingsMeasure.width,
    minHeight: "32px",
  },
  subTitle: {
    color: colors.fg,
    fontSize: font.sizeLg,
    fontWeight: font.weightSemibold,
    margin: 0,
  },
  subWrap: { marginBottom: "12px", maxWidth: settingsMeasure.width },
  group: {
    borderBottomColor: colors.line,
    borderBottomStyle: "solid",
    borderBottomWidth: "1px",
    display: "flex",
    flexDirection: "column",
    gap: "8px",
    maxWidth: settingsMeasure.width,
    paddingBlock: "14px",
  },
  // A setting: label and description on the start side, its control on
  // the end side (switches do the same through `Switch description`).
  row: {
    alignItems: "flex-start",
    columnGap: "24px",
    display: "flex",
    flexWrap: "wrap",
    justifyContent: "space-between",
    rowGap: "10px",
  },
  // A wide control (a segmented control with four options): beside the
  // label like every other row, with the description under both so it
  // keeps the full width instead of a squeezed column. Phones stack.
  rowWide: {
    display: { default: "grid", [bp.mobile]: "flex" },
    flexDirection: "column",
    gridTemplateColumns: "minmax(0, 1fr) auto",
  },
  rowWideLabel: { gridColumn: 1, gridRow: 1 },
  rowWideControl: {
    gridColumn: 2,
    gridRow: 1,
    justifyContent: { default: "flex-end", [bp.mobile]: "flex-start" },
  },
  rowWideDescription: { gridColumn: "1 / -1", gridRow: 2 },
  rowText: {
    display: "flex",
    flexBasis: "240px",
    flexDirection: "column",
    flexGrow: 1,
    gap: "4px",
    minWidth: 0,
  },
  // As tall as a segmented control or select (32px), so the label sits on
  // the control's centre line.
  rowLabel: {
    alignItems: "center",
    color: colors.fg,
    display: "flex",
    fontSize: font.sizeMd,
    fontWeight: font.weightMedium,
    lineHeight: 1.4,
    margin: 0,
    minHeight: "32px",
  },
  rowDescription: {
    color: colors.fg3,
    fontSize: font.sizeSm,
    lineHeight: 1.45,
    margin: 0,
  },
  // A read-only status row ("How they load: through the proxy"): a note,
  // not a setting without its control.
  note: {
    alignItems: "flex-start",
    color: colors.fg2,
    columnGap: "10px",
    display: "flex",
    fontSize: font.sizeSm,
    lineHeight: 1.45,
  },
  noteIcon: { color: colors.fg3, flexShrink: 0, marginTop: "1px" },
  noteLabel: { color: colors.fg, fontWeight: font.weightMedium },
  rowControl: {
    alignItems: "center",
    columnGap: "8px",
    display: "flex",
    flexShrink: 0,
    flexWrap: "wrap",
    rowGap: "8px",
  },
  // Groups of rows inside a section (Appearance, Reading…).
  // Clearly above the rows' 13.5px labels, clearly below the title.
  heading: {
    color: colors.fg,
    fontSize: font.sizeXl,
    fontWeight: font.weightSemibold,
    letterSpacing: font.trackingTight,
    margin: 0,
    marginTop: "28px",
    maxWidth: settingsMeasure.width,
  },
  firstHeading: { marginTop: "14px" },
  // Empty lists inside a section (no filters, no scripts): a quiet dashed box,
  // start-aligned like the rows around it.
  inlineEmpty: {
    borderColor: colors.line2,
    borderRadius: radius.lg,
    borderStyle: "dashed",
    borderWidth: "1px",
    maxWidth: settingsMeasure.width,
  },
  groupLabel: {
    color: colors.fg2,
    fontSize: font.sizeSm,
    fontWeight: font.weightSemibold,
    margin: 0,
  },
  notice: {
    alignItems: "flex-start",
    borderRadius: radius.lg,
    columnGap: "10px",
    display: "flex",
    fontSize: font.sizeMd,
    lineHeight: 1.5,
    marginBlock: "10px",
    maxWidth: settingsMeasure.width,
    padding: "10px 12px",
  },
  info: { backgroundColor: colors.infoSoft, color: colors.fg },
  warn: { backgroundColor: colors.warnSoft, color: colors.fg },
  danger: { backgroundColor: colors.dangerSoft, color: colors.fg },
  noticeIcon: { flexShrink: 0, marginTop: "2px" },
  infoIcon: { color: colors.info },
  warnIcon: { color: colors.warn },
  dangerIcon: { color: colors.danger },
  noticeBody: {
    display: "flex",
    flexDirection: "column",
    gap: "6px",
    minWidth: 0,
  },
});

export const SectionHeader = ({
  description,
  title,
}: {
  readonly title: string;
  readonly description?: ReactNode;
}) => (
  <header {...stylex.props(styles.header)}>
    {/* The panel's own title, so it doesn't open on a grey sentence; the
        top bar names the section too (settings-shell.tsx), smaller. */}
    <h2 {...stylex.props(styles.title)}>{title}</h2>
    {description === undefined ? null : (
      <p {...stylex.props(styles.description)}>{description}</p>
    )}
  </header>
);

/**
 * The head of a block inside a section (e.g. Filters › Rules, Sieve
 * scripts): its title, an optional action on the same row (a small
 * secondary button) and a description in the section's text style.
 */
export const SubsectionHeader = ({
  action,
  description,
  id,
  title,
}: {
  readonly title: string;
  readonly id?: string;
  readonly description?: ReactNode;
  readonly action?: ReactNode;
}) => (
  <div {...stylex.props(styles.subWrap)}>
    <div {...stylex.props(styles.subHead)}>
      <h3
        {...(id === undefined ? {} : { id })}
        {...stylex.props(styles.subTitle)}
      >
        {title}
      </h3>
      {action}
    </div>
    {description === undefined ? null : (
      <p {...stylex.props(styles.description)}>{description}</p>
    )}
  </div>
);

/**
 * One setting as a row. The visible label is `aria-hidden` when the control
 * names itself (segmented controls and selects take `label`), so it isn't
 * read twice.
 */
export const SettingRow = ({
  children,
  description,
  label,
  wide = false,
}: {
  readonly label: ReactNode;
  readonly description?: ReactNode;
  /** The control; omit it for a read-only status row. */
  readonly children?: ReactNode;
  /**
   * A wide control: it stays beside the label, and the description runs
   * under both at the full width.
   */
  readonly wide?: boolean;
}) => {
  if (children === undefined && !wide) {
    return (
      <div {...stylex.props(styles.note)}>
        <Info aria-hidden size={15} {...stylex.props(styles.noteIcon)} />
        <p {...stylex.props(styles.rowDescription)}>
          <span {...stylex.props(styles.noteLabel)}>{label}</span>
          {description === undefined ? null : <> · {description}</>}
        </p>
      </div>
    );
  }
  const labelText = (
    <p
      {...(children === undefined ? {} : { "aria-hidden": true })}
      {...stylex.props(styles.rowLabel, wide ? styles.rowWideLabel : null)}
    >
      {label}
    </p>
  );
  const descriptionText =
    description === undefined ? null : (
      <p
        {...stylex.props(
          styles.rowDescription,
          wide ? styles.rowWideDescription : null
        )}
      >
        {description}
      </p>
    );
  const control =
    children === undefined ? null : (
      <div
        {...stylex.props(
          styles.rowControl,
          wide ? styles.rowWideControl : null
        )}
      >
        {children}
      </div>
    );
  if (wide) {
    return (
      <div {...stylex.props(styles.row, styles.rowWide)}>
        {labelText}
        {control}
        {descriptionText}
      </div>
    );
  }
  return (
    <div {...stylex.props(styles.row)}>
      <div {...stylex.props(styles.rowText)}>
        {labelText}
        {descriptionText}
      </div>
      {control}
    </div>
  );
};

/**
 * A titled group of settings rows inside a section (Appearance, Privacy…),
 * a region named after its title.
 */
export const SettingsBlock = ({
  children,
  first = false,
  title,
}: {
  readonly title: string;
  readonly children: ReactNode;
  readonly first?: boolean;
}) => {
  const id = useId();
  return (
    <section aria-labelledby={id}>
      <h3
        id={id}
        {...stylex.props(styles.heading, first ? styles.firstHeading : null)}
      >
        {title}
      </h3>
      {children}
    </section>
  );
};

/**
 * "Nothing here yet" inside a section: the shared `EmptyState` (title and
 * one line, centred) in a quiet box, so it reads like the other empty
 * states without a pane-sized illustration.
 */
export const InlineEmpty = ({
  children,
  title,
}: {
  readonly title: string;
  readonly children?: ReactNode;
}) => (
  <div {...stylex.props(styles.inlineEmpty)}>
    <EmptyState headingLevel={4} title={title}>
      {children}
    </EmptyState>
  </div>
);

export const SettingGroup = ({
  children,
  label,
}: {
  readonly children: ReactNode;
  readonly label?: string;
}) => (
  <section
    {...(label === undefined ? {} : { "aria-label": label })}
    {...stylex.props(styles.group)}
  >
    {label === undefined ? null : (
      <h3 aria-hidden {...stylex.props(styles.groupLabel)}>
        {label}
      </h3>
    )}
    {children}
  </section>
);

const TONES = {
  danger: {
    icon: CircleAlert,
    iconStyle: styles.dangerIcon,
    style: styles.danger,
  },
  info: { icon: Info, iconStyle: styles.infoIcon, style: styles.info },
  warn: { icon: TriangleAlert, iconStyle: styles.warnIcon, style: styles.warn },
} as const;

export const Notice = ({
  children,
  tone = "info",
}: {
  readonly children: ReactNode;
  readonly tone?: keyof typeof TONES;
}) => {
  const { icon: Glyph, iconStyle, style } = TONES[tone];
  return (
    <div
      role={tone === "danger" ? "alert" : "note"}
      {...stylex.props(styles.notice, style)}
    >
      <Glyph
        aria-hidden
        size={16}
        {...stylex.props(styles.noticeIcon, iconStyle)}
      />
      <div {...stylex.props(styles.noticeBody)}>{children}</div>
    </div>
  );
};
