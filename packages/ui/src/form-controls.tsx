// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Form controls with the Helix look: a select (Base UI's, with the look of
// our menus), a date field and a code editor (textarea with
// line numbers). The date field is native (the phone's own picker).
//
// DateInput is a plain `<input type="date">` for now; React Aria's
// DatePicker can replace it without changing callers.

import { Select as BaseSelect } from "@base-ui/react/select";
import * as stylex from "@stylexjs/stylex";
import { Check, ChevronDown } from "lucide-react";
import { useState } from "react";
import type { UIEvent } from "react";

import { cls, usePortalContainer } from "./internal";
import { menuStyles, popupClass } from "./menu";
import {
  colors,
  density,
  font,
  motion,
  radius,
  shadow,
  touch,
  zIndex,
} from "./tokens.stylex";

const FORCED_COLORS = "@media (forced-colors: active)";

const styles = stylex.create({
  control: {
    appearance: "none",
    backgroundColor: colors.bg,
    borderColor: {
      ":focus-visible": colors.acc,
      ":hover": colors.fg3,
      default: colors.line2,
    },
    borderRadius: radius.control,
    borderStyle: "solid",
    borderWidth: "1px",
    boxShadow: { ":focus-visible": shadow.focusHalo, default: "none" },
    boxSizing: "border-box",
    color: colors.fg,
    fontFamily: font.sans,
    fontSize: { default: font.sizeMd, [touch.coarse]: "16px" },
    minHeight: { default: density.input, [touch.coarse]: touch.target },
    minWidth: 0,
    outline: "none",
    paddingInline: "12px",
    transitionDuration: motion.fast,
    transitionProperty: "border-color, box-shadow",
    transitionTimingFunction: motion.easing,
    width: "100%",
  },
  disabled: { cursor: "not-allowed", opacity: 0.5 },
  invalid: { borderColor: colors.danger },
  // Select: the trigger is a button that looks like a field.
  trigger: {
    alignItems: "center",
    columnGap: "8px",
    cursor: "pointer",
    display: "flex",
    paddingInlineEnd: "10px",
    textAlign: "start",
  },
  triggerFit: { maxWidth: "100%", width: "auto" },
  valueBox: { display: "grid", flexGrow: 1, minWidth: 0 },
  value: {
    gridArea: "1 / 1",
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  sizer: {
    gridArea: "1 / 1",
    height: 0,
    overflow: "hidden",
    visibility: "hidden",
    whiteSpace: "nowrap",
  },
  chevron: { color: colors.fg3, display: "flex", flexShrink: 0 },
  positioner: { zIndex: zIndex.picker },
  popup: {
    display: "flex",
    flexDirection: "column",
    maxWidth: "var(--available-width)",
    minWidth: "max(var(--anchor-width), 160px)",
    overflowY: "hidden",
    padding: 0,
  },
  list: {
    minHeight: 0,
    outline: "none",
    overflowY: "auto",
    overscrollBehavior: "contain",
    padding: "5px",
  },
  checkSlot: { display: "flex", flexShrink: 0, width: "15px" },
  itemText: { overflow: "hidden", textOverflow: "ellipsis" },
  date: { colorScheme: "light dark" },
  // Code editor: gutter + textarea in one bordered box.
  code: {
    backgroundColor: colors.bg,
    borderColor: { ":focus-within": colors.acc, default: colors.line2 },
    borderRadius: radius.control,
    borderStyle: "solid",
    borderWidth: "1px",
    boxShadow: { ":focus-within": shadow.focusHalo, default: "none" },
    display: "flex",
    fontFamily: font.mono,
    fontSize: "12.5px",
    lineHeight: "20px",
    minWidth: 0,
    overflow: "hidden",
  },
  // The numbers scroll with the text (a transform), inside a clipped box
  // as tall as the textarea.
  gutterBox: {
    backgroundColor: colors.subtle,
    borderInlineEndColor: colors.line,
    borderInlineEndStyle: "solid",
    borderInlineEndWidth: "1px",
    flexShrink: 0,
    overflow: "hidden",
    position: "relative",
  },
  gutter: {
    color: colors.fg3,
    font: "inherit",
    insetInline: 0,
    margin: 0,
    paddingBlock: "10px",
    paddingInline: "8px",
    position: "absolute",
    textAlign: "end",
    top: 0,
    userSelect: "none",
  },
  gutterLine: { display: "block", height: "20px" },
  gutterError: {
    backgroundColor: colors.dangerSoft,
    borderRadius: radius.xs,
    color: colors.danger,
    fontWeight: font.weightBold,
  },
  codeBody: {
    display: "flex",
    flexGrow: 1,
    minWidth: 0,
    position: "relative",
  },
  // The coloured copy of the text, under a transparent textarea: same
  // font, padding and wrapping, moved with the textarea's scroll.
  highlightBox: {
    display: { default: "block", [FORCED_COLORS]: "none" },
    inset: 0,
    overflow: "hidden",
    pointerEvents: "none",
    position: "absolute",
  },
  highlight: {
    color: colors.fg,
    font: "inherit",
    fontSize: { default: "inherit", [touch.coarse]: "16px" },
    margin: 0,
    paddingBlock: "10px",
    paddingInline: "10px",
    tabSize: 2,
    whiteSpace: "pre",
  },
  codeAreaHighlighted: {
    // The text is drawn by the layer below; the caret and the selection
    // stay the textarea's. High-contrast mode shows the plain text.
    caretColor: colors.fg,
    color: { default: "transparent", [FORCED_COLORS]: "CanvasText" },
  },
  codeArea: {
    backgroundColor: "transparent",
    borderStyle: "none",
    boxSizing: "border-box",
    color: colors.fg,
    flexGrow: 1,
    width: "100%",
    fontFamily: "inherit",
    fontSize: { default: "inherit", [touch.coarse]: "16px" },
    lineHeight: "inherit",
    margin: 0,
    minWidth: 0,
    outline: "none",
    paddingBlock: "10px",
    paddingInline: "10px",
    resize: "vertical",
    tabSize: 2,
    whiteSpace: "pre",
  },
});

const tokenStyles = stylex.create({
  action: { color: colors.info },
  comment: { color: colors.fg3, fontStyle: "italic" },
  invalid: {
    color: colors.danger,
    textDecorationLine: "underline",
    textDecorationStyle: "wavy",
  },
  keyword: { color: colors.accText, fontWeight: font.weightSemibold },
  number: { color: colors.warn },
  plain: {},
  punct: { color: colors.fg2 },
  string: { color: colors.ok },
  tag: { color: colors.n2 },
  test: { color: colors.accText },
});

// --- Select -----------------------------------------------------------------

export interface SelectOption<Value extends string> {
  readonly value: Value;
  readonly label: string;
  readonly disabled?: boolean;
}

/** Options under a heading (like `<optgroup>`). */
export interface SelectGroup<Value extends string> {
  readonly label: string;
  readonly options: readonly SelectOption<Value>[];
}

export type SelectEntry<Value extends string> =
  | SelectOption<Value>
  | SelectGroup<Value>;

export interface SelectProps<Value extends string> {
  /** Options and groups of options, in order. Values must be unique. */
  readonly options: readonly SelectEntry<Value>[];
  readonly value: Value;
  readonly onValueChange: (value: Value) => void;
  /** Accessible name (no visible label of its own). */
  readonly label: string;
  readonly disabled?: boolean;
  readonly invalid?: boolean;
  readonly id?: string;
  /** Form field name (a hidden input carries the value). */
  readonly name?: string;
  /**
   * `fill` (default): as wide as its container. `fit`: as wide as its
   * longest option, like a native select (for selects inside a sentence).
   */
  readonly width?: "fill" | "fit";
}

const isGroup = <Value extends string>(
  entry: SelectEntry<Value>
): entry is SelectGroup<Value> => "options" in entry;

const flatOptions = <Value extends string>(
  entries: readonly SelectEntry<Value>[]
): SelectOption<Value>[] =>
  entries.flatMap((entry) => (isGroup(entry) ? entry.options : [entry]));

const SelectItem = <Value extends string>({
  option,
}: {
  readonly option: SelectOption<Value>;
}) => (
  <BaseSelect.Item
    className={(state) =>
      cls(
        menuStyles.item,
        state.highlighted ? menuStyles.highlighted : null,
        state.disabled ? menuStyles.disabled : null
      )
    }
    data-value={option.value}
    disabled={option.disabled ?? false}
    label={option.label}
    value={option.value}
  >
    <BaseSelect.ItemText className={cls(menuStyles.label, styles.itemText)}>
      {option.label}
    </BaseSelect.ItemText>
    {/* The check only in the chosen option (cheap with hundreds of
        options); the slot keeps the labels aligned. */}
    <span aria-hidden {...stylex.props(styles.checkSlot)}>
      <BaseSelect.ItemIndicator>
        <Check aria-hidden size={15} {...stylex.props(menuStyles.check)} />
      </BaseSelect.ItemIndicator>
    </span>
  </BaseSelect.Item>
);

/**
 * A select with the look of our menus (Base UI's Select: a combobox
 * button and a listbox). Keyboard: arrows, Home/End, typing jumps to the
 * option that starts with it, Enter chooses, Esc closes.
 */
export const Select = <Value extends string>({
  disabled = false,
  id,
  invalid = false,
  label,
  name,
  onValueChange,
  options,
  value,
  width = "fill",
}: SelectProps<Value>) => {
  const container = usePortalContainer();
  const flat = flatOptions(options);
  const labelOf = (current: unknown): string =>
    flat.find((option) => option.value === current)?.label ?? "";
  return (
    <BaseSelect.Root
      disabled={disabled}
      onValueChange={(next: unknown) => {
        const chosen = flat.find((option) => option.value === next);
        if (chosen !== undefined) {
          onValueChange(chosen.value);
        }
      }}
      value={value}
      {...(id === undefined ? {} : { id })}
      {...(name === undefined ? {} : { name })}
    >
      <BaseSelect.Trigger
        aria-label={label}
        // The name is always `label`: inside a Field, Base UI would name the
        // select after the Field's label (aria-labelledby wins over
        // aria-label), and a Field can hold several selects. An explicit
        // undefined replaces Base UI's value.
        aria-labelledby={undefined}
        className={cls(
          styles.control,
          styles.trigger,
          width === "fit" ? styles.triggerFit : null,
          invalid ? styles.invalid : null,
          disabled ? styles.disabled : null
        )}
        data-value={value}
        {...(invalid ? { "aria-invalid": true } : {})}
      >
        <span {...stylex.props(styles.valueBox)}>
          <BaseSelect.Value className={cls(styles.value)}>
            {labelOf}
          </BaseSelect.Value>
          {width === "fit"
            ? flat.map((option) => (
                // Invisible copies of every label in the same grid cell:
                // the trigger is as wide as the longest one.
                <span
                  aria-hidden
                  key={option.value}
                  {...stylex.props(styles.sizer)}
                >
                  {option.label}
                </span>
              ))
            : null}
        </span>
        <BaseSelect.Icon className={cls(styles.chevron)}>
          <ChevronDown aria-hidden size={14} />
        </BaseSelect.Icon>
      </BaseSelect.Trigger>
      <BaseSelect.Portal container={container}>
        <BaseSelect.Positioner
          alignItemWithTrigger={false}
          className={cls(menuStyles.positioner, styles.positioner)}
          collisionPadding={8}
          sideOffset={6}
        >
          <BaseSelect.Popup
            className={(state) => popupClass(state, styles.popup)}
          >
            <BaseSelect.List className={cls(styles.list)}>
              {options.map((entry) =>
                isGroup(entry) ? (
                  <BaseSelect.Group key={`group:${entry.label}`}>
                    <BaseSelect.GroupLabel
                      className={cls(menuStyles.groupLabel)}
                    >
                      {entry.label}
                    </BaseSelect.GroupLabel>
                    {entry.options.map((option) => (
                      <SelectItem key={option.value} option={option} />
                    ))}
                  </BaseSelect.Group>
                ) : (
                  <SelectItem key={entry.value} option={entry} />
                )
              )}
            </BaseSelect.List>
          </BaseSelect.Popup>
        </BaseSelect.Positioner>
      </BaseSelect.Portal>
    </BaseSelect.Root>
  );
};

// --- DateInput --------------------------------------------------------------

export interface DateInputProps {
  /** `YYYY-MM-DD`, or "" for none. */
  readonly value: string;
  readonly onValueChange: (value: string) => void;
  readonly label: string;
  readonly min?: string;
  readonly max?: string;
  readonly disabled?: boolean;
  readonly invalid?: boolean;
  readonly id?: string;
}

/** A calendar day (native picker; see the note at the top of the file). */
export const DateInput = ({
  disabled = false,
  id,
  invalid = false,
  label,
  max,
  min,
  onValueChange,
  value,
}: DateInputProps) => (
  <input
    aria-label={label}
    disabled={disabled}
    onChange={(event) => {
      onValueChange(event.target.value);
    }}
    type="date"
    value={value}
    {...(id === undefined ? {} : { id })}
    {...(min === undefined ? {} : { min })}
    {...(max === undefined ? {} : { max })}
    {...(invalid ? { "aria-invalid": true } : {})}
    {...stylex.props(
      styles.control,
      styles.date,
      invalid ? styles.invalid : null,
      disabled ? styles.disabled : null
    )}
  />
);

// --- CodeEditor -------------------------------------------------------------

/** What a piece of code is, for its colour (a small fixed palette). */
export type CodeTokenKind =
  | "keyword"
  | "test"
  | "action"
  | "tag"
  | "string"
  | "number"
  | "comment"
  | "punct"
  | "plain"
  | "invalid";

export interface CodeToken {
  readonly kind: CodeTokenKind;
  readonly text: string;
}

export interface CodeEditorProps {
  readonly value: string;
  readonly onValueChange: (value: string) => void;
  readonly label: string;
  /** 1-based line to mark (an error), or null. */
  readonly errorLine?: number | null;
  readonly rows?: number;
  readonly readOnly?: boolean;
  /** Id of the element that describes the editor (an error message). */
  readonly describedBy?: string;
  /**
   * Colours: the text in pieces that join back into exactly the text
   * (only text nodes are rendered, never HTML). Without it, plain text.
   */
  readonly highlight?: (text: string) => readonly CodeToken[];
}

/** Code with line numbers and, optionally, syntax colours (no scripts). */
export const CodeEditor = ({
  describedBy,
  errorLine = null,
  highlight,
  label,
  onValueChange,
  readOnly = false,
  rows = 18,
  value,
}: CodeEditorProps) => {
  const [scrollTop, setScrollTop] = useState(0);
  const [scrollLeft, setScrollLeft] = useState(0);
  const lines = value.split("\n").length;
  const handleScroll = (event: UIEvent<HTMLTextAreaElement>): void => {
    setScrollTop(event.currentTarget.scrollTop);
    setScrollLeft(event.currentTarget.scrollLeft);
  };
  const tokens = highlight?.(value) ?? null;
  return (
    <div {...stylex.props(styles.code)}>
      <div
        aria-hidden
        style={{ width: `calc(${String(String(lines).length)}ch + 18px)` }}
        {...stylex.props(styles.gutterBox)}
      >
        <pre
          style={{ transform: `translateY(${String(-scrollTop)}px)` }}
          {...stylex.props(styles.gutter)}
        >
          {Array.from({ length: lines }, (_, index) => (
            <span
              key={index}
              {...stylex.props(
                styles.gutterLine,
                errorLine === index + 1 ? styles.gutterError : null
              )}
            >
              {index + 1}
            </span>
          ))}
        </pre>
      </div>
      <div {...stylex.props(styles.codeBody)}>
        {tokens === null ? null : (
          <div aria-hidden {...stylex.props(styles.highlightBox)}>
            <pre
              data-code-highlight=""
              style={{
                transform: `translate(${String(-scrollLeft)}px, ${String(-scrollTop)}px)`,
              }}
              {...stylex.props(styles.highlight)}
            >
              {tokens.map((token, index) => (
                <span
                  // Pieces have no identity of their own; their order is it.
                  // oxlint-disable-next-line react/no-array-index-key -- see above.
                  key={index}
                  {...stylex.props(tokenStyles[token.kind])}
                >
                  {token.text}
                </span>
              ))}
              {/* A last empty line still takes its height. */}
              {"\n"}
            </pre>
          </div>
        )}
        <textarea
          aria-label={label}
          autoCapitalize="off"
          autoComplete="off"
          autoCorrect="off"
          onChange={(event) => {
            onValueChange(event.target.value);
          }}
          onScroll={handleScroll}
          readOnly={readOnly}
          rows={rows}
          spellCheck={false}
          value={value}
          wrap="off"
          {...(describedBy === undefined
            ? {}
            : { "aria-describedby": describedBy })}
          {...(errorLine === null ? {} : { "aria-invalid": true })}
          {...stylex.props(
            styles.codeArea,
            tokens === null ? null : styles.codeAreaHighlighted
          )}
        />
      </div>
    </div>
  );
};
