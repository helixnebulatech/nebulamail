// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { Field as BaseField } from "@base-ui/react/field";
import { Input as BaseInput } from "@base-ui/react/input";
import * as stylex from "@stylexjs/stylex";
import { Search } from "lucide-react";
import { createContext, use } from "react";
import type { ComponentProps, ReactNode } from "react";

import type { Icon } from "./button";
import { cls } from "./internal";
import { colors, density, font, motion, radius, shadow } from "./tokens.stylex";

const styles = stylex.create({
  // The visible box is the wrapper, so icons and hints can sit inside it.
  box: {
    alignItems: "center",
    backgroundColor: colors.bg,
    borderColor: {
      ":focus-within": colors.acc,
      ":hover": colors.fg3,
      default: colors.line2,
    },
    borderRadius: radius.control,
    borderStyle: "solid",
    borderWidth: "1px",
    boxShadow: { ":focus-within": shadow.focusHalo, default: "none" },
    boxSizing: "border-box",
    color: colors.fg3,
    columnGap: "8px",
    display: "flex",
    height: density.input,
    paddingInline: "12px",
    transitionDuration: motion.fast,
    transitionProperty: "border-color, box-shadow",
    transitionTimingFunction: motion.easing,
  },
  disabled: { opacity: 0.5, pointerEvents: "none" },
  // Next to compact controls (a segmented filter, sm buttons): 32 px.
  small: { borderRadius: radius.md, height: "32px", paddingInline: "10px" },
  // An inline filter next to a SegmentedControl: same pill family.
  pill: { borderRadius: radius.pill, paddingInlineStart: "12px" },
  input: {
    "::placeholder": { color: colors.fg3, opacity: 1 },
    appearance: "none",
    backgroundColor: "transparent",
    borderStyle: "none",
    color: colors.fg,
    flexGrow: 1,
    fontFamily: font.sans,
    fontSize: font.sizeMd,
    height: "100%",
    minWidth: 0,
    outline: "none",
    padding: 0,
  },
  invalid: {
    borderColor: { ":focus-within": colors.danger, default: colors.danger },
    boxShadow: {
      ":focus-within": `0 0 0 4px color-mix(in srgb, ${colors.danger} 16%, transparent)`,
      default: "none",
    },
  },
  search: {
    backgroundColor: colors.bg,
    borderColor: { ":focus-within": colors.acc, default: colors.line },
    borderRadius: radius.lg,
    boxShadow: { ":focus-within": shadow.focusHalo, default: shadow.sh1 },
    columnGap: "10px",
    height: "40px",
    paddingInlineEnd: "10px",
    paddingInlineStart: "14px",
  },
  // Inside a palette (Linear-style) the field is the top of the dialog: no
  // box and no ring (it always has the focus), only a hairline under it.
  searchEmbedded: {
    backgroundColor: "transparent",
    borderBottomColor: colors.line,
    borderColor: {
      ":focus-within": "transparent",
      ":hover": "transparent",
      default: "transparent",
    },
    borderRadius: 0,
    boxShadow: { ":focus-within": "none", default: "none" },
    fontSize: font.sizeLg,
    height: "52px",
    paddingInlineEnd: "16px",
    paddingInlineStart: "20px",
  },
});

type BaseInputProps = Omit<
  ComponentProps<typeof BaseInput>,
  "className" | "style" | "size"
>;

export interface InputProps extends BaseInputProps {
  /** Leading icon. */
  readonly icon?: Icon;
  /** Trailing content (a unit, a shortcut hint, a button). */
  readonly end?: ReactNode;
  /** Error look; inside a Field use its `error` instead. */
  readonly invalid?: boolean;
  /** "sm": 32 px tall, to sit next to compact controls. */
  readonly size?: "sm" | "md";
  /**
   * "pill": fully rounded, for a filter field in a toolbar next to a
   * SegmentedControl (same height with size "sm", same shape).
   */
  readonly shape?: "rounded" | "pill";
}

// Let an Input draw the error/disabled look of its Field.
const FieldInvalidContext = createContext(false);
const FieldDisabledContext = createContext(false);

/** Text input. Inside a Field it is labelled and described automatically. */
export const Input = ({
  icon: IconGlyph,
  end,
  invalid: invalidProp = false,
  disabled: disabledProp = false,
  size = "md",
  shape = "rounded",
  ...props
}: InputProps) => {
  const invalid = invalidProp || use(FieldInvalidContext);
  const disabled = disabledProp || use(FieldDisabledContext);
  return (
    <span
      {...stylex.props(
        styles.box,
        size === "sm" ? styles.small : null,
        shape === "pill" ? styles.pill : null,
        invalid ? styles.invalid : null,
        disabled ? styles.disabled : null
      )}
    >
      {IconGlyph === undefined ? null : <IconGlyph aria-hidden size={16} />}
      <BaseInput
        {...props}
        {...(invalid ? { "aria-invalid": true } : {})}
        className={cls(styles.input)}
        disabled={disabled}
      />
      {end}
    </span>
  );
};

const textareaStyles = stylex.create({
  box: {
    alignItems: "stretch",
    height: "auto",
    paddingBlock: "8px",
  },
  textarea: {
    fontFamily: font.sans,
    lineHeight: font.lineHeight,
    minHeight: "72px",
    resize: "vertical",
  },
});

type BaseTextareaProps = Omit<
  ComponentProps<"textarea">,
  "className" | "style"
>;

export interface TextareaProps extends BaseTextareaProps {
  readonly invalid?: boolean;
}

/** Multi-line text. Inside a Field it is labelled automatically. */
export const Textarea = ({
  invalid: invalidProp = false,
  disabled: disabledProp = false,
  rows = 3,
  ...props
}: TextareaProps) => {
  const invalid = invalidProp || use(FieldInvalidContext);
  const disabled = disabledProp || use(FieldDisabledContext);
  return (
    <span
      {...stylex.props(
        styles.box,
        textareaStyles.box,
        invalid ? styles.invalid : null,
        disabled ? styles.disabled : null
      )}
    >
      <BaseField.Control
        disabled={disabled}
        render={
          <textarea
            {...props}
            {...(invalid ? { "aria-invalid": true } : {})}
            className={cls(styles.input, textareaStyles.textarea)}
            rows={rows}
          />
        }
      />
    </span>
  );
};

export interface SearchFieldProps extends Omit<BaseInputProps, "type"> {
  /** Accessible name (the placeholder is not a label). */
  readonly label: string;
  /** Shortcut hint slot, e.g. `<Shortcut keys={["⌘", "K"]} />` or `["Ctrl", "K"]`. */
  readonly hint?: ReactNode;
  /** In a command palette: no box, a hairline under it (the dialog is the box). */
  readonly embedded?: boolean;
}

/** The top-bar search box with a shortcut hint. */
export const SearchField = ({
  label,
  hint,
  embedded = false,
  ...props
}: SearchFieldProps) => (
  <span
    {...stylex.props(
      styles.box,
      styles.search,
      embedded ? styles.searchEmbedded : null
    )}
  >
    <Search aria-hidden size={16} />
    <BaseInput
      {...props}
      aria-label={label}
      className={cls(styles.input)}
      type="search"
    />
    {hint}
  </span>
);

const fieldStyles = stylex.create({
  description: { color: colors.fg3, fontSize: font.sizeSm, margin: 0 },
  error: { color: colors.danger, fontSize: font.sizeSm, margin: 0 },
  label: {
    color: colors.fg2,
    fontSize: font.sizeSm,
    fontWeight: font.weightButton,
  },
  root: {
    display: "flex",
    flexDirection: "column",
    fontFamily: font.sans,
    minWidth: 0,
    rowGap: "6px",
  },
});

export interface FieldProps {
  readonly label: ReactNode;
  readonly description?: ReactNode;
  /** Error message; marks the field invalid while set. */
  readonly error?: ReactNode;
  readonly disabled?: boolean;
  /** The control: an Input. */
  readonly children: ReactNode;
}

/** Label + control + description/error, wired for accessibility. */
export const Field = ({
  label,
  description,
  error,
  disabled = false,
  children,
}: FieldProps) => {
  const invalid = error !== undefined && error !== null && error !== false;
  return (
    <BaseField.Root
      className={cls(fieldStyles.root)}
      disabled={disabled}
      invalid={invalid}
    >
      <BaseField.Label className={cls(fieldStyles.label)}>
        {label}
      </BaseField.Label>
      <FieldInvalidContext value={invalid}>
        <FieldDisabledContext value={disabled}>{children}</FieldDisabledContext>
      </FieldInvalidContext>
      {invalid ? (
        <BaseField.Error className={cls(fieldStyles.error)} match>
          {error}
        </BaseField.Error>
      ) : null}
      {!invalid && description !== undefined ? (
        <BaseField.Description className={cls(fieldStyles.description)}>
          {description}
        </BaseField.Description>
      ) : null}
    </BaseField.Root>
  );
};
