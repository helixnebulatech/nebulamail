// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Rich-text editor for writing mail (TipTap). A compact
// toolbar with the usual shortcuts: undo/redo, text style (paragraph,
// headings, code block), bold, italic, underline, strikethrough, link,
// font and size (system fonts and px sizes only), text colour, highlight,
// alignment, lists and emoji, and a "⋯" menu for the rarer ones (quote,
// horizontal rule, clear formatting, insert an image, text direction).
// Colours come from a short palette that reads well on the white page
// mail is usually read on.
//
// Security: every HTML that enters goes through `sanitize` (pasted
// and dropped HTML) or must already be sanitized (`defaultValue`,
// `signature`); TipTap's schema then drops anything it doesn't know. There
// is no remote image: the only image node is a quoted `cid:` picture, shown
// from a `data:` copy the app passes (`inlineImage`), so nothing in the
// editor can load a URL. Links are http(s)/mailto only.
//
// CSP: TipTap's injected <style> (no nonce) is off (`injectCSS: false`); the
// CSS lives in editor.css. ProseMirror sets inline `style` attributes, which
// the app's `style-src-attr 'unsafe-inline'` allows.

import { Toolbar } from "@base-ui/react/toolbar";
import {
  COMPOSE_FONTS,
  COMPOSE_FONT_SIZES,
  fontById,
  isComposeFontId,
  isComposeFontSize,
  isTextDirection,
} from "@nebulamail/mail-render/compose-style";
import type {
  ComposeFontId,
  ComposeFontSize,
  TextDirection,
} from "@nebulamail/mail-render/compose-style";
import * as stylex from "@stylexjs/stylex";
import type { Editor } from "@tiptap/core";
import { EditorContent, useEditor, useEditorState } from "@tiptap/react";
import {
  ALargeSmall,
  Baseline,
  CaseSensitive,
  Bold,
  ChevronDown,
  Ellipsis,
  Heading1,
  Heading2,
  Heading3,
  Highlighter,
  ImagePlus,
  Italic,
  Link2,
  List,
  ListOrdered,
  Palette,
  Pilcrow,
  PilcrowLeft,
  PilcrowRight,
  Redo2,
  RemoveFormatting,
  SeparatorHorizontal,
  Smile,
  SquareCode,
  Strikethrough,
  TextAlignCenter,
  TextAlignEnd,
  TextAlignJustify,
  TextAlignStart,
  TextQuote,
  Type,
  Underline,
  Undo2,
} from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import type { KeyboardEvent, ReactNode } from "react";

import { Button } from "./button";
import type { Icon } from "./button";
import { normalizeLink } from "./editor-links";
import {
  ALIGNMENTS,
  HEADING_LEVELS,
  Latest,
  INLINE_IMAGE_ATTRIBUTE,
  applySignature,
  clearFormatting,
  composeEditorOptions,
  fillInlineImage,
  insertInlineImage,
  setDirection,
  setFont,
  setFontSize,
  textStyleAt,
} from "./editor-schema";
import type {
  Alignment,
  EditorCallbacks,
  InsertedImage,
} from "./editor-schema";
import { EmojiPicker } from "./emoji-picker";
import { cls, shared } from "./internal";
import {
  Menu,
  MenuItem,
  MenuRadioGroup,
  MenuRadioItem,
  MenuSeparator,
} from "./menu";
import { colors, font, motion, radius } from "./tokens.stylex";
import { Tooltip } from "./tooltip";

const styles = stylex.create({
  root: {
    "--nm-editor-link": colors.accText,
    "--nm-editor-muted": colors.fg2,
    "--nm-editor-placeholder": colors.fg3,
    "--nm-editor-quote": colors.line2,
    "--nm-editor-code": colors.hover,
    display: "flex",
    flexDirection: "column",
    flexGrow: 1,
    minHeight: 0,
  },
  content: {
    color: colors.fg,
    cursor: "text",
    display: "flex",
    flexDirection: "column",
    flexGrow: 1,
    fontFamily: font.sans,
    fontSize: font.sizeLg,
    lineHeight: 1.6,
    minHeight: "120px",
  },
  // The message's default font and size (Settings), as it will be sent.
  contentFont: (family: string, size: string) => ({
    fontFamily: family,
    fontSize: size,
  }),
  // Each font's name in its own font in the menu.
  fontSample: (family: string) => ({ fontFamily: family }),
  // Text starts 16px in, on the same line as the compose window's "To",
  // "Subject" and the toolbar's first icon.
  editable: {
    outline: "none",
    paddingBlock: "12px",
    paddingInline: "16px",
  },
  // One row in a 600 px compose window (wraps only when narrower).
  // "Aa": the less common tools, as a second row with a hairline above.
  extraRow: {
    alignItems: "center",
    borderTopColor: colors.line,
    borderTopStyle: "solid",
    borderTopWidth: "1px",
    display: "flex",
    flexBasis: "100%",
    flexWrap: "wrap",
    marginTop: "4px",
    paddingTop: "4px",
  },
  toolbar: {
    alignItems: "center",
    display: "flex",
    flexWrap: "wrap",
    paddingBlock: "4px",
    // 9px + the buttons' 7px inner space: the first icon at 16px.
    paddingInline: "9px",
  },
  // Between the groups (history, text style, marks, colour, paragraph,
  // insert): visible enough to read as groups at a glance.
  separator: {
    backgroundColor: colors.line2,
    height: "16px",
    marginInline: "4px",
    width: "1px",
  },
  tool: {
    alignItems: "center",
    backgroundColor: {
      ":hover": colors.hover,
      default: "transparent",
    },
    borderRadius: radius.sm,
    color: { ":hover": colors.fg, default: colors.fg2 },
    cursor: "pointer",
    display: "inline-flex",
    height: "28px",
    justifyContent: "center",
    transitionDuration: motion.fast,
    transitionProperty: "background-color, color",
    transitionTimingFunction: motion.easing,
    width: "28px",
  },
  // A tool that opens a menu: its icon and a small caret.
  menuTool: {
    columnGap: "1px",
    paddingInline: "4px",
    width: "auto",
  },
  caret: { color: colors.fg3 },
  // The colour now in use, as a bar under the icon (text colour, highlight).
  indicator: {
    borderRadius: "1px",
    bottom: "4px",
    height: "3px",
    insetInlineStart: "7px",
    position: "absolute",
    width: "14px",
  },
  indicatorColor: (color: string) => ({ backgroundColor: color }),
  relative: { position: "relative" },
  // The hidden colour input sits under its menu button, so the system
  // picker opens there.
  pickerAnchor: { bottom: 0, insetInlineStart: 0 },
  toolActive: {
    backgroundColor: { ":hover": colors.accSoft, default: colors.accSoft },
    color: { ":hover": colors.accText, default: colors.accText },
  },
  toolDisabled: { cursor: "not-allowed", opacity: 0.45 },
  linkBar: {
    alignItems: "center",
    columnGap: "6px",
    display: "flex",
    paddingBottom: "6px",
    paddingInline: "16px",
  },
  linkInput: {
    backgroundColor: colors.bg,
    borderColor: { ":focus": colors.acc, default: colors.line2 },
    borderRadius: radius.control,
    borderStyle: "solid",
    borderWidth: "1px",
    color: colors.fg,
    flexGrow: 1,
    fontFamily: font.sans,
    fontSize: font.sizeMd,
    height: "30px",
    minWidth: 0,
    outline: "none",
    paddingInline: "10px",
  },
  linkError: {
    borderColor: { ":focus": colors.danger, default: colors.danger },
  },
  error: { color: colors.danger, fontSize: font.sizeSm, whiteSpace: "nowrap" },
  group: { alignItems: "center", display: "inline-flex" },
  // Below the text: clicking the empty rest of the area focuses the editor.
  rest: { flexGrow: 1 },
  quoteBar: { flexShrink: 0, paddingBottom: "8px", paddingInline: "14px" },
  // Gmail's "…" under a reply: the quoted message stays in the email, only
  // hidden while writing. The text appears on hover or keyboard focus.
  quoteToggle: {
    alignItems: "center",
    backgroundColor: { ":hover": colors.line, default: colors.hover },
    borderRadius: radius.pill,
    borderStyle: "none",
    color: colors.fg2,
    columnGap: "6px",
    cursor: "pointer",
    display: "inline-flex",
    fontFamily: font.sans,
    fontSize: font.sizeSm,
    height: "22px",
    outlineColor: { ":focus-visible": colors.focus, default: "transparent" },
    outlineOffset: "2px",
    outlineStyle: "solid",
    outlineWidth: "2px",
    paddingInline: "8px",
  },
  quoteLabel: {
    maxWidth: { ":is(:hover > *, :focus-visible > *)": "240px", default: 0 },
    overflow: "hidden",
    transitionDuration: motion.fast,
    transitionProperty: "max-width",
    whiteSpace: "nowrap",
  },
});

/** The colours of the palettes (text and highlight). */
export type RichTextColor =
  | "gray"
  | "red"
  | "orange"
  | "yellow"
  | "green"
  | "blue"
  | "purple"
  | "pink";

/** Text for the toolbar (the app translates it). */
export interface RichTextLabels {
  readonly toolbar: string;
  readonly undo: string;
  readonly redo: string;
  readonly textStyle: string;
  /** The font menu, and its "Default" (the font from Settings). */
  readonly font: string;
  readonly fontDefault: string;
  /** The size menu, and its "Default". */
  readonly fontSize: string;
  readonly fontSizeDefault: string;
  readonly emoji: string;
  /** The "⋯" menu with the rarer tools. */
  readonly more: string;
  /**
   * The "Aa" toggle that shows the rest of the toolbar (text style, font,
   * size, strikethrough, colours, alignment). Hidden at first, so the
   * composer opens with the everyday tools only.
   */
  readonly formattingOptions: string;
  readonly insertImage: string;
  readonly directionLtr: string;
  readonly directionRtl: string;
  readonly paragraph: string;
  readonly heading1: string;
  readonly heading2: string;
  readonly heading3: string;
  readonly codeBlock: string;
  readonly bold: string;
  readonly italic: string;
  readonly underline: string;
  readonly strike: string;
  readonly link: string;
  readonly textColor: string;
  readonly highlight: string;
  /** "No colour" in the colour menus. */
  readonly colorNone: string;
  /** Opens the system colour picker ("Custom…"). */
  readonly colorCustom: string;
  readonly colors: Readonly<Record<RichTextColor, string>>;
  readonly align: string;
  readonly alignLeft: string;
  readonly alignCenter: string;
  readonly alignRight: string;
  readonly alignJustify: string;
  readonly bulletList: string;
  readonly orderedList: string;
  readonly quote: string;
  readonly horizontalRule: string;
  readonly clearFormatting: string;
  readonly linkUrl: string;
  readonly linkApply: string;
  readonly linkRemove: string;
  readonly linkCancel: string;
  readonly linkInvalid: string;
  readonly showQuote: string;
  readonly hideQuote: string;
}

/** Shortcut keys to show in tooltips, already formatted (⌘ or Ctrl). */
export type RichTextShortcuts = Partial<
  Record<
    | "undo"
    | "redo"
    | "paragraph"
    | "heading1"
    | "heading2"
    | "heading3"
    | "codeBlock"
    | "bold"
    | "italic"
    | "underline"
    | "strike"
    | "link"
    | "highlight"
    | "alignLeft"
    | "alignCenter"
    | "alignRight"
    | "alignJustify"
    | "bulletList"
    | "orderedList"
    | "quote"
    | "clearFormatting",
    readonly string[]
  >
>;

export interface RichTextEditorProps {
  /** Initial HTML, already sanitized. Read once, when the editor mounts. */
  readonly defaultValue: string;
  /** Called with the editor's HTML after every change. */
  readonly onChange?: (html: string) => void;
  /**
   * Instead of `onChange`, for long documents: after every change, a
   * function that returns that version's HTML when called (serialized only
   * then, not on every keystroke).
   */
  readonly onEdit?: (read: () => string) => void;
  /** Sanitizer for pasted and dropped HTML (required). */
  readonly sanitize: (html: string) => string;
  /**
   * The signature block's content (sanitized HTML), or null for none. When
   * it changes, the block is replaced (or added before the quote).
   */
  readonly signature?: string | null;
  /** Files pasted into the editor (images from the clipboard…). */
  readonly onFiles?: (files: readonly File[]) => void;
  /** ⌘↵ / Ctrl+Enter. */
  readonly onSubmit?: () => void;
  /** Accessible name of the text area. */
  readonly label: string;
  readonly placeholder?: string;
  readonly labels: RichTextLabels;
  readonly shortcuts?: RichTextShortcuts;
  /**
   * Hide the quoted message behind a "…" button until asked (replies). It
   * is still sent; the caret or a selection inside it shows it again.
   */
  readonly collapseQuote?: boolean;
  /** Focus on mount: at the start (replies) or the end. */
  readonly autoFocus?: "start" | "end" | false;
  /**
   * A `data:` copy of an inline (`cid:`) image of the quote, by Content-ID,
   * once the app has it; until then the image shows as a placeholder. The
   * `cid:` URL itself is never loaded.
   */
  readonly inlineImage?: (cid: string) => string | undefined;
  /** The message's default font and size (shown as they will be sent). */
  readonly defaultFont?: ComposeFontId;
  readonly defaultFontSize?: ComposeFontSize;
  /**
   * "Insert image": the app picks and uploads a local image, then calls
   * `insert` with its Content-ID to show it at the cursor (an inline part
   * of the message). Without it the menu item isn't shown.
   */
  readonly onInsertImage?: (insert: (image: InsertedImage) => void) => void;
}

interface ToolProps {
  readonly icon: Icon;
  readonly label: string;
  readonly shortcut: readonly string[] | undefined;
  readonly active: boolean;
  readonly disabled: boolean;
  readonly onClick: () => void;
}

/** Keep the selection in the editor while clicking a tool. */
const keepSelection = (event: { preventDefault: () => void }): void => {
  event.preventDefault();
};

const Tool = ({
  active,
  disabled,
  icon: Glyph,
  label,
  onClick,
  shortcut,
}: ToolProps) => (
  <Tooltip content={label} {...(shortcut === undefined ? {} : { shortcut })}>
    <Toolbar.Button
      aria-label={label}
      aria-pressed={active}
      className={cls(
        shared.resetButton,
        styles.tool,
        active ? styles.toolActive : null,
        disabled ? styles.toolDisabled : null,
        shared.focusRing
      )}
      disabled={disabled}
      onMouseDown={keepSelection}
      onClick={onClick}
    >
      <Glyph aria-hidden size={16} />
    </Toolbar.Button>
  </Tooltip>
);

interface ToolMenuProps {
  readonly icon: Icon;
  readonly label: string;
  readonly disabled: boolean;
  /** The colour in use, shown under the icon (colour menus). */
  readonly indicator?: string | null;
  readonly children: ReactNode;
}

/** A toolbar button that opens a menu of choices. */
const ToolMenu = ({
  children,
  disabled,
  icon: Glyph,
  indicator,
  label,
}: ToolMenuProps) => (
  <Menu
    tooltip={label}
    trigger={
      <Toolbar.Button
        aria-label={label}
        className={cls(
          shared.resetButton,
          styles.tool,
          styles.menuTool,
          styles.relative,
          disabled ? styles.toolDisabled : null,
          shared.focusRing
        )}
        disabled={disabled}
        onMouseDown={keepSelection}
      >
        <Glyph aria-hidden size={16} />
        <ChevronDown aria-hidden size={12} {...stylex.props(styles.caret)} />
        {indicator === undefined || indicator === null ? null : (
          <span
            aria-hidden
            {...stylex.props(
              styles.indicator,
              styles.indicatorColor(indicator)
            )}
          />
        )}
      </Toolbar.Button>
    }
  >
    {children}
  </Menu>
);

/** The value of "no colour" in the colour menus. */
const NONE = "none";

/** `#rrggbb` for the colour input: hex as it is, `rgb()` converted. */
const toHex6 = (color: string | null, fallback: string): string => {
  if (color === null) {
    return fallback;
  }
  if (/^#[\da-f]{6}$/iu.test(color)) {
    return color.toLowerCase();
  }
  const short = /^#(?<r>[\da-f])(?<g>[\da-f])(?<b>[\da-f])$/iu.exec(color);
  if (short?.groups !== undefined) {
    const { b = "", g = "", r = "" } = short.groups;
    return `#${r}${r}${g}${g}${b}${b}`.toLowerCase();
  }
  const rgb = /^rgb\(\s*(?<r>\d+)\s*,\s*(?<g>\d+)\s*,\s*(?<b>\d+)\s*\)$/iu.exec(
    color
  );
  if (rgb?.groups === undefined) {
    return fallback;
  }
  const { b = "0", g = "0", r = "0" } = rgb.groups;
  return `#${[r, g, b]
    .map((channel) =>
      Math.min(255, Number(channel)).toString(16).padStart(2, "0")
    )
    .join("")}`;
};

interface ColorMenuProps {
  readonly icon: Icon;
  readonly label: string;
  readonly labels: RichTextLabels;
  readonly disabled: boolean;
  readonly palette: readonly (readonly [RichTextColor, string])[];
  /** The colour at the cursor, or null. */
  readonly current: string | null;
  /** The picker's starting colour when there is none. */
  readonly fallback: string;
  /**
   * A colour was chosen (null: none). `focus`: back to the text; false
   * from the system picker, which is still open.
   */
  readonly onPick: (value: string | null, focus: boolean) => void;
}

/**
 * Text colour or highlight: a short palette, and "Custom…" for the
 * system colour picker (any colour, always written as hex).
 */
const ColorMenu = ({
  current,
  disabled,
  fallback,
  icon,
  label,
  labels,
  onPick,
  palette,
}: ColorMenuProps) => {
  const input = useRef<HTMLInputElement>(null);
  const inPalette =
    current === null ||
    palette.some(([, value]) => value.toLowerCase() === current.toLowerCase());
  return (
    <span {...stylex.props(styles.relative)}>
      <ToolMenu
        disabled={disabled}
        icon={icon}
        indicator={current}
        label={label}
      >
        <MenuRadioGroup
          onValueChange={(value) => {
            onPick(value === NONE ? null : value, true);
          }}
          value={current ?? NONE}
        >
          <MenuRadioItem swatch={null} value={NONE}>
            {labels.colorNone}
          </MenuRadioItem>
          {palette.map(([name, value]) => (
            <MenuRadioItem key={name} swatch={value} value={value}>
              {labels.colors[name]}
            </MenuRadioItem>
          ))}
          {inPalette ? null : (
            // The custom colour in use, so it shows as chosen.
            <MenuRadioItem swatch={current} value={current}>
              {toHex6(current, fallback).toUpperCase()}
            </MenuRadioItem>
          )}
        </MenuRadioGroup>
        <MenuItem
          icon={Palette}
          onClick={() => {
            const picker = input.current;
            if (picker === null) {
              return;
            }
            picker.value = toHex6(current, fallback);
            // Still in the click (user activation), as the picker needs.
            try {
              picker.showPicker();
            } catch {
              picker.click();
            }
          }}
        >
          {labels.colorCustom}
        </MenuItem>
      </ToolMenu>
      <input
        aria-hidden
        // Applied as it changes (live preview), without taking the focus
        // from the open picker. Cancelling changes nothing.
        onChange={(event) => {
          onPick(event.currentTarget.value, false);
        }}
        ref={input}
        tabIndex={-1}
        type="color"
        {...stylex.props(shared.visuallyHidden, styles.pickerAnchor)}
      />
    </span>
  );
};

/** Text colours: dark enough to read on white (where mail is read). */
const TEXT_COLORS: readonly (readonly [RichTextColor, string])[] = [
  ["gray", "#5F6368"],
  ["red", "#C5221F"],
  ["orange", "#B45309"],
  ["green", "#188038"],
  ["blue", "#1A73E8"],
  ["purple", "#7B1FA2"],
];

/** Highlights: light enough for black text on top. */
const HIGHLIGHTS: readonly (readonly [RichTextColor, string])[] = [
  ["yellow", "#FFF3A3"],
  ["green", "#CEEAD6"],
  ["blue", "#D2E3FC"],
  ["pink", "#FAD2E1"],
  ["orange", "#FDE2C4"],
  ["purple", "#E9D7FE"],
];

type BlockStyle = "paragraph" | `heading${1 | 2 | 3}` | "codeBlock";

/** The text style menu, in this order. */
const BLOCK_ORDER: readonly BlockStyle[] = [
  "paragraph",
  "heading1",
  "heading2",
  "heading3",
  "codeBlock",
];

const BLOCK_ICON: Readonly<Record<BlockStyle, Icon>> = {
  codeBlock: SquareCode,
  heading1: Heading1,
  heading2: Heading2,
  heading3: Heading3,
  paragraph: Pilcrow,
};

const ALIGN_ICON: Readonly<Record<Alignment, Icon>> = {
  center: TextAlignCenter,
  justify: TextAlignJustify,
  left: TextAlignStart,
  right: TextAlignEnd,
};

const ALIGN_LABEL = {
  center: "alignCenter",
  justify: "alignJustify",
  left: "alignLeft",
  right: "alignRight",
} as const satisfies Record<Alignment, keyof RichTextLabels>;

/** What the toolbar shows: the formatting at the cursor. */
interface ToolbarState {
  readonly marks: readonly string[];
  readonly block: BlockStyle;
  readonly align: Alignment;
  readonly color: string | null;
  readonly highlight: string | null;
  readonly font: ComposeFontId | null;
  readonly size: ComposeFontSize | null;
  readonly dir: TextDirection | null;
  readonly canUndo: boolean;
  readonly canRedo: boolean;
}

const MARKS = [
  "bold",
  "italic",
  "underline",
  "strike",
  "link",
  "bulletList",
  "orderedList",
  "blockquote",
] as const;

const blockStyle = (editor: Editor): BlockStyle => {
  if (editor.isActive("codeBlock")) {
    return "codeBlock";
  }
  const level = HEADING_LEVELS.find((value) =>
    editor.isActive("heading", { level: value })
  );
  return level === undefined ? "paragraph" : `heading${level}`;
};

const stringAttribute = (value: unknown): string | null =>
  typeof value === "string" && value !== "" ? value : null;

const toolbarState = (editor: Editor | null): ToolbarState | null =>
  editor === null
    ? null
    : {
        ...textStyleAt(editor),
        align:
          ALIGNMENTS.find((value) => editor.isActive({ textAlign: value })) ??
          "left",
        block: blockStyle(editor),
        canRedo: editor.can().redo(),
        canUndo: editor.can().undo(),
        color: stringAttribute(editor.getAttributes("textStyle").color),
        highlight: stringAttribute(editor.getAttributes("highlight").color),
        marks: MARKS.filter((name) => editor.isActive(name)),
      };

const sameState = (a: ToolbarState | null, b: ToolbarState | null) =>
  a === b ||
  (a !== null &&
    b !== null &&
    a.align === b.align &&
    a.block === b.block &&
    a.canRedo === b.canRedo &&
    a.canUndo === b.canUndo &&
    a.color === b.color &&
    a.highlight === b.highlight &&
    a.font === b.font &&
    a.size === b.size &&
    a.dir === b.dir &&
    a.marks.join(",") === b.marks.join(","));

const setBlock = (editor: Editor, style: string): void => {
  const chain = editor.chain().focus();
  if (style === "codeBlock") {
    chain.setCodeBlock().run();
    return;
  }
  const level = HEADING_LEVELS.find((value) => style === `heading${value}`);
  if (level === undefined) {
    chain.setParagraph().run();
  } else {
    chain.setHeading({ level }).run();
  }
};

interface ToolGroupProps {
  readonly editor: Editor | null;
  readonly labels: RichTextLabels;
  readonly disabled: boolean;
}

/** A toolbar button styled like the others, for menus and popovers. */
const triggerButton = (label: string, disabled: boolean, glyph: ReactNode) => (
  <Toolbar.Button
    aria-label={label}
    className={cls(
      shared.resetButton,
      styles.tool,
      disabled ? styles.toolDisabled : null,
      shared.focusRing
    )}
    disabled={disabled}
    onMouseDown={keepSelection}
  >
    {glyph}
  </Toolbar.Button>
);

/** Font and size menus ("Default": the message's, from Settings). */
const FontTools = ({
  disabled,
  editor,
  family,
  labels,
  px,
}: ToolGroupProps & {
  readonly family: ComposeFontId | null;
  readonly px: ComposeFontSize | null;
}) => (
  <>
    <ToolMenu disabled={disabled} icon={Type} label={labels.font}>
      <MenuRadioGroup
        onValueChange={(value) => {
          if (editor !== null) {
            setFont(editor, isComposeFontId(value) ? value : null);
          }
        }}
        value={family ?? NONE}
      >
        <MenuRadioItem closeOnClick value={NONE}>
          {labels.fontDefault}
        </MenuRadioItem>
        {COMPOSE_FONTS.map((option) => (
          <MenuRadioItem closeOnClick key={option.id} value={option.id}>
            <span {...stylex.props(styles.fontSample(option.stack))}>
              {option.label}
            </span>
          </MenuRadioItem>
        ))}
      </MenuRadioGroup>
    </ToolMenu>
    <ToolMenu disabled={disabled} icon={ALargeSmall} label={labels.fontSize}>
      <MenuRadioGroup
        onValueChange={(value) => {
          if (editor !== null) {
            setFontSize(editor, isComposeFontSize(value) ? value : null);
          }
        }}
        value={px ?? NONE}
      >
        <MenuRadioItem closeOnClick value={NONE}>
          {labels.fontSizeDefault}
        </MenuRadioItem>
        {COMPOSE_FONT_SIZES.map((size) => (
          <MenuRadioItem closeOnClick key={size} value={size}>
            {size.replace("px", " px")}
          </MenuRadioItem>
        ))}
      </MenuRadioGroup>
    </ToolMenu>
  </>
);

const EmojiTool = ({ disabled, editor, labels }: ToolGroupProps) => (
  <EmojiPicker
    label={labels.emoji}
    onPick={(emoji) => {
      editor?.chain().focus().insertContent(emoji).run();
    }}
    trigger={triggerButton(
      labels.emoji,
      disabled,
      <Smile aria-hidden size={16} />
    )}
  />
);

/**
 * "⋯": the rarer tools (quote, divider line, clear formatting, insert an
 * image, paragraph direction), so the toolbar fits one row of a compose
 * window. Their keyboard shortcuts work all the same.
 */
const MoreTools = ({
  dir,
  disabled,
  editor,
  labels,
  onInsertImage,
  shortcuts,
}: ToolGroupProps & {
  readonly dir: TextDirection | null;
  readonly onInsertImage: (() => void) | undefined;
  readonly shortcuts: RichTextShortcuts;
}) => {
  const run = (action: (current: Editor) => void) => () => {
    if (editor !== null) {
      action(editor);
    }
  };
  return (
    <Menu
      tooltip={labels.more}
      trigger={triggerButton(
        labels.more,
        disabled,
        <Ellipsis aria-hidden size={16} />
      )}
    >
      <MenuItem
        icon={TextQuote}
        onClick={run((current) => {
          current.chain().focus().toggleBlockquote().run();
        })}
        {...(shortcuts.quote === undefined
          ? {}
          : { shortcut: shortcuts.quote })}
      >
        {labels.quote}
      </MenuItem>
      <MenuItem
        icon={SeparatorHorizontal}
        onClick={run((current) => {
          current.chain().focus().setHorizontalRule().run();
        })}
      >
        {labels.horizontalRule}
      </MenuItem>
      <MenuItem
        icon={RemoveFormatting}
        onClick={run(clearFormatting)}
        {...(shortcuts.clearFormatting === undefined
          ? {}
          : { shortcut: shortcuts.clearFormatting })}
      >
        {labels.clearFormatting}
      </MenuItem>
      <MenuSeparator />
      {onInsertImage === undefined ? null : (
        <>
          <MenuItem icon={ImagePlus} onClick={onInsertImage}>
            {labels.insertImage}
          </MenuItem>
          <MenuSeparator />
        </>
      )}
      <MenuRadioGroup
        onValueChange={(value) => {
          if (editor !== null && isTextDirection(value)) {
            setDirection(editor, value);
          }
        }}
        value={dir ?? NONE}
      >
        <MenuRadioItem closeOnClick icon={PilcrowRight} value="ltr">
          {labels.directionLtr}
        </MenuRadioItem>
        <MenuRadioItem closeOnClick icon={PilcrowLeft} value="rtl">
          {labels.directionRtl}
        </MenuRadioItem>
      </MenuRadioGroup>
    </Menu>
  );
};

interface FormatGroupProps {
  readonly editor: Editor | null;
  readonly disabled: boolean;
  readonly labels: RichTextLabels;
  readonly shortcuts: RichTextShortcuts;
  readonly state: ToolbarState | null;
}

const separatorElement = () => (
  <Toolbar.Separator className={cls(styles.separator)} />
);

/** Text style, font and size (behind "Aa"). */
const BlockTools = ({
  disabled,
  editor,
  labels,
  shortcuts,
  state,
}: FormatGroupProps) => {
  const block = state?.block ?? "paragraph";
  return (
    <span {...stylex.props(styles.group)}>
      <ToolMenu
        disabled={disabled}
        icon={BLOCK_ICON[block]}
        label={labels.textStyle}
      >
        <MenuRadioGroup
          onValueChange={(value) => {
            if (editor !== null) {
              setBlock(editor, value);
            }
          }}
          value={block}
        >
          {BLOCK_ORDER.map((style) => (
            <MenuRadioItem
              icon={BLOCK_ICON[style]}
              key={style}
              value={style}
              {...(shortcuts[style] === undefined
                ? {}
                : { shortcut: shortcuts[style] })}
            >
              {labels[style]}
            </MenuRadioItem>
          ))}
        </MenuRadioGroup>
      </ToolMenu>
      <FontTools
        disabled={disabled}
        editor={editor}
        family={state?.font ?? null}
        labels={labels}
        px={state?.size ?? null}
      />
    </span>
  );
};

/** Text colour and highlight (behind "Aa"). */
const ColorTools = ({
  disabled,
  editor,
  labels,
  state,
}: Omit<FormatGroupProps, "shortcuts">) => {
  const separator = separatorElement();
  return (
    <span {...stylex.props(styles.group)}>
      {separator}
      <ColorMenu
        current={state?.color ?? null}
        disabled={disabled}
        fallback="#000000"
        icon={Baseline}
        label={labels.textColor}
        labels={labels}
        onPick={(value, focus) => {
          const chain = focus ? editor?.chain().focus() : editor?.chain();
          if (value === null) {
            chain?.unsetColor().run();
          } else {
            chain?.setColor(value).run();
          }
        }}
        palette={TEXT_COLORS}
      />
      <ColorMenu
        current={state?.highlight ?? null}
        disabled={disabled}
        fallback="#fff3a3"
        icon={Highlighter}
        label={labels.highlight}
        labels={labels}
        onPick={(value, focus) => {
          const chain = focus ? editor?.chain().focus() : editor?.chain();
          if (value === null) {
            chain?.unsetHighlight().run();
          } else {
            chain?.setHighlight({ color: value }).run();
          }
        }}
        palette={HIGHLIGHTS}
      />
    </span>
  );
};

/** Alignment (behind "Aa"). */
const AlignTool = ({
  disabled,
  editor,
  labels,
  shortcuts,
  state,
}: FormatGroupProps) => {
  const align = state?.align ?? "left";
  return (
    <ToolMenu disabled={disabled} icon={ALIGN_ICON[align]} label={labels.align}>
      <MenuRadioGroup
        onValueChange={(value) => {
          editor?.chain().focus().setTextAlign(value).run();
        }}
        value={align}
      >
        {ALIGNMENTS.map((value) => (
          <MenuRadioItem
            icon={ALIGN_ICON[value]}
            key={value}
            value={value}
            {...(shortcuts[ALIGN_LABEL[value]] === undefined
              ? {}
              : { shortcut: shortcuts[ALIGN_LABEL[value]] })}
          >
            {labels[ALIGN_LABEL[value]]}
          </MenuRadioItem>
        ))}
      </MenuRadioGroup>
    </ToolMenu>
  );
};

interface EditorToolbarProps {
  readonly editor: Editor | null;
  readonly labels: RichTextLabels;
  readonly shortcuts: RichTextShortcuts;
  readonly onLink: () => void;
  readonly onInsertImage: (() => void) | undefined;
}

const EditorToolbar = ({
  editor,
  labels,
  onInsertImage,
  onLink,
  shortcuts,
}: EditorToolbarProps) => {
  const state = useEditorState({
    editor,
    equalityFn: sameState,
    selector: ({ editor: current }) => toolbarState(current),
  });
  const disabled = editor === null;
  // The rest of the tools, behind "Aa" (Gmail, Superhuman).
  const [full, setFull] = useState(false);
  const marks = new Set(state?.marks);
  const run = (action: (current: Editor) => void) => () => {
    if (editor !== null) {
      action(editor);
    }
  };
  const tool = (
    name: keyof RichTextShortcuts & keyof RichTextLabels,
    icon: Icon,
    action: (current: Editor) => void,
    active = false
  ) => (
    <Tool
      active={active}
      disabled={disabled}
      icon={icon}
      label={labels[name]}
      onClick={run(action)}
      shortcut={shortcuts[name]}
    />
  );
  const separator = <Toolbar.Separator className={cls(styles.separator)} />;

  return (
    <Toolbar.Root aria-label={labels.toolbar} className={cls(styles.toolbar)}>
      <span {...stylex.props(styles.group)}>
        <Tool
          active={false}
          disabled={disabled || state?.canUndo !== true}
          icon={Undo2}
          label={labels.undo}
          onClick={run((current) => {
            current.chain().focus().undo().run();
          })}
          shortcut={shortcuts.undo}
        />
        <Tool
          active={false}
          disabled={disabled || state?.canRedo !== true}
          icon={Redo2}
          label={labels.redo}
          onClick={run((current) => {
            current.chain().focus().redo().run();
          })}
          shortcut={shortcuts.redo}
        />
      </span>
      <span {...stylex.props(styles.group)}>
        {separator}
        {tool(
          "bold",
          Bold,
          (current) => {
            current.chain().focus().toggleBold().run();
          },
          marks.has("bold")
        )}
        {tool(
          "italic",
          Italic,
          (current) => {
            current.chain().focus().toggleItalic().run();
          },
          marks.has("italic")
        )}
        {tool(
          "underline",
          Underline,
          (current) => {
            current.chain().focus().toggleUnderline().run();
          },
          marks.has("underline")
        )}
        {/* The link tool opens the link bar (RichTextEditor). */}
        <Tool
          active={marks.has("link")}
          disabled={disabled}
          icon={Link2}
          label={labels.link}
          onClick={onLink}
          shortcut={shortcuts.link}
        />
      </span>
      <span {...stylex.props(styles.group)}>
        {separator}
        {tool(
          "bulletList",
          List,
          (current) => {
            current.chain().focus().toggleBulletList().run();
          },
          marks.has("bulletList")
        )}
        {tool(
          "orderedList",
          ListOrdered,
          (current) => {
            current.chain().focus().toggleOrderedList().run();
          },
          marks.has("orderedList")
        )}
      </span>
      <span {...stylex.props(styles.group)}>
        {separator}
        <EmojiTool disabled={disabled} editor={editor} labels={labels} />
        <MoreTools
          dir={state?.dir ?? null}
          disabled={disabled}
          editor={editor}
          labels={labels}
          onInsertImage={onInsertImage}
          shortcuts={shortcuts}
        />
      </span>
      <span {...stylex.props(styles.group)}>
        {separator}
        <Tool
          active={full}
          disabled={disabled}
          icon={CaseSensitive}
          label={labels.formattingOptions}
          onClick={() => {
            setFull((current) => !current);
          }}
          shortcut={undefined}
        />
      </span>
      {/* The rest, on a row of its own under the everyday tools. */}
      {full ? (
        <span {...stylex.props(styles.extraRow)}>
          <BlockTools
            disabled={disabled}
            editor={editor}
            labels={labels}
            shortcuts={shortcuts}
            state={state}
          />
          <span {...stylex.props(styles.group)}>
            {separator}
            {tool(
              "strike",
              Strikethrough,
              (current) => {
                current.chain().focus().toggleStrike().run();
              },
              marks.has("strike")
            )}
          </span>
          <ColorTools
            disabled={disabled}
            editor={editor}
            labels={labels}
            state={state}
          />
          <span {...stylex.props(styles.group)}>
            {separator}
            <AlignTool
              disabled={disabled}
              editor={editor}
              labels={labels}
              shortcuts={shortcuts}
              state={state}
            />
          </span>
        </span>
      ) : null}
    </Toolbar.Root>
  );
};

interface LinkBarProps {
  readonly editor: Editor;
  readonly labels: RichTextLabels;
  readonly initial: string;
  readonly onClose: () => void;
}

/** Edits the link under the cursor (or adds one). Enter applies, Esc closes. */
const LinkBar = ({ editor, initial, labels, onClose }: LinkBarProps) => {
  const [text, setText] = useState(initial);
  const [invalid, setInvalid] = useState(false);
  const id = useId();
  const editing = editor.isActive("link");

  const close = (): void => {
    onClose();
    editor.commands.focus();
  };

  const apply = (): void => {
    const href = normalizeLink(text);
    if (href === null) {
      setInvalid(true);
      return;
    }
    const chain = editor.chain().focus().extendMarkRange("link");
    if (editor.state.selection.empty && !editing) {
      // Nothing selected: the address itself becomes the link text.
      chain
        .insertContent({
          marks: [{ attrs: { href }, type: "link" }],
          text: text.trim(),
          type: "text",
        })
        .run();
    } else {
      chain.setLink({ href }).run();
    }
    onClose();
  };

  const remove = (): void => {
    editor.chain().focus().extendMarkRange("link").unsetLink().run();
    onClose();
  };

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>): void => {
    if (event.key === "Enter") {
      event.preventDefault();
      apply();
    } else if (event.key === "Escape") {
      // Close only the link bar, not the composer.
      event.preventDefault();
      event.stopPropagation();
      close();
    }
  };

  return (
    <div {...stylex.props(styles.linkBar)}>
      <label htmlFor={id} {...stylex.props(shared.visuallyHidden)}>
        {labels.linkUrl}
      </label>
      <input
        aria-describedby={invalid ? `${id}-error` : undefined}
        aria-invalid={invalid}
        // oxlint-disable-next-line jsx-a11y/no-autofocus -- the field opens on request (button or ⌘K) to be typed in.
        autoFocus
        id={id}
        inputMode="url"
        onChange={(event) => {
          setText(event.target.value);
          setInvalid(false);
        }}
        onKeyDown={onKeyDown}
        placeholder="https://"
        spellCheck={false}
        type="text"
        value={text}
        {...stylex.props(styles.linkInput, invalid ? styles.linkError : null)}
      />
      {invalid ? (
        <span id={`${id}-error`} role="alert" {...stylex.props(styles.error)}>
          {labels.linkInvalid}
        </span>
      ) : null}
      <Button onClick={apply} size="sm" variant="primary">
        {labels.linkApply}
      </Button>
      {editing ? (
        <Button onClick={remove} size="sm" variant="ghost">
          {labels.linkRemove}
        </Button>
      ) : null}
      <Button onClick={close} size="sm" variant="ghost">
        {labels.linkCancel}
      </Button>
    </div>
  );
};

const NO_SHORTCUTS: RichTextShortcuts = {};

const currentLink = (editor: Editor): string => {
  const href: unknown = editor.getAttributes("link").href;
  return typeof href === "string" ? href : "";
};

interface QuoteState {
  readonly has: boolean;
  readonly inside: boolean;
}

const NO_QUOTE: QuoteState = { has: false, inside: false };

/**
 * Whether the document has a quoted message, and whether the selection
 * reaches into it (caret inside, or a range such as select-all over it).
 */
const quoteState = (editor: Editor | null): QuoteState => {
  if (editor === null) {
    return NO_QUOTE;
  }
  const { doc, selection } = editor.state;
  let has = false;
  let inside = false;
  doc.descendants((node, position) => {
    if (node.type.name !== "quote") {
      return true;
    }
    has = true;
    inside ||=
      selection.from <= position + node.nodeSize && selection.to >= position;
    return false;
  });
  return { has, inside };
};

/** The editor area in the message's default font and size, if given. */
const contentFont = (
  family: ComposeFontId | undefined,
  size: ComposeFontSize | undefined
) =>
  family === undefined && size === undefined
    ? null
    : styles.contentFont(
        family === undefined ? "inherit" : fontById(family).stack,
        size ?? "inherit"
      );

/** "Insert image": asks the app, which calls back to insert at the cursor. */
const imageInserter = (
  editor: Editor | null,
  onInsertImage: RichTextEditorProps["onInsertImage"]
): (() => void) | undefined =>
  onInsertImage === undefined
    ? undefined
    : () => {
        onInsertImage((image) => {
          if (editor !== null && !editor.isDestroyed) {
            insertInlineImage(editor, image);
          }
        });
      };

export const RichTextEditor = ({
  autoFocus = false,
  collapseQuote = false,
  defaultFont,
  defaultFontSize,
  defaultValue,
  inlineImage,
  label,
  labels,
  onChange,
  onEdit,
  onFiles,
  onInsertImage,
  onSubmit,
  placeholder = "",
  sanitize,
  shortcuts = NO_SHORTCUTS,
  signature = null,
}: RichTextEditorProps) => {
  // The link bar's starting text while open, null while closed.
  const [link, setLink] = useState<string | null>(null);
  // TipTap keeps the functions it was created with; these always call the
  // latest props (updated after each render).
  // oxlint-disable-next-line react/hook-use-state -- a stable box, never replaced (no setter needed); a ref can't be handed to code during render.
  const [callbacks] = useState(
    () =>
      new Latest<EditorCallbacks>({
        inlineImage,
        onChange,
        onEdit,
        onFiles,
        onSubmit,
        sanitize,
      })
  );
  const appliedSignature = useRef(signature);

  const options = composeEditorOptions(callbacks, {
    content: defaultValue,
    placeholder,
  });
  const editor = useEditor({
    ...options,
    autofocus: autoFocus,
    editorProps: {
      ...options.editorProps,
      attributes: {
        "aria-label": label,
        "aria-multiline": "true",
        class: `nm-editor ${cls(styles.editable)}`,
        role: "textbox",
      },
    },
    // The composer only renders in the browser; never render on the server.
    immediatelyRender: false,
  });

  const openLink = (): void => {
    if (editor !== null) {
      setLink(currentLink(editor));
    }
  };

  const quote =
    useEditorState({
      editor,
      equalityFn: (a: QuoteState, b: QuoteState | null) =>
        a.has === b?.has && a.inside === b.inside,
      selector: ({ editor: current }): QuoteState => quoteState(current),
    }) ?? NO_QUOTE;
  const [quoteShown, setQuoteShown] = useState(!collapseQuote);
  // Editing inside the hidden quote (search, select all…) shows it.
  if (quote.inside && !quoteShown) {
    setQuoteShown(true);
  }
  const quoteHidden = quote.has && !quoteShown;

  useEffect(() => {
    callbacks.set({
      inlineImage,
      onChange,
      onEdit,
      onFiles,
      onSubmit,
      openLink,
      sanitize,
    });
  });

  // Inline images whose data: copy arrived after their view was made.
  useEffect(() => {
    if (editor === null || inlineImage === undefined) {
      return;
    }
    for (const view of editor.view.dom.querySelectorAll<HTMLElement>(
      `[${INLINE_IMAGE_ATTRIBUTE}]`
    )) {
      fillInlineImage(
        view,
        inlineImage(view.getAttribute(INLINE_IMAGE_ATTRIBUTE) ?? "")
      );
    }
  }, [editor, inlineImage]);

  // The identity changed: swap the signature (TipTap is an external system).
  useEffect(() => {
    if (editor !== null && signature !== appliedSignature.current) {
      appliedSignature.current = signature;
      applySignature(editor, signature);
    }
  }, [editor, signature]);

  const root = stylex.props(styles.root);
  return (
    <div
      {...root}
      // editor.css hides the quote (it lives in TipTap's DOM, not ours).
      className={`${root.className ?? ""}${quoteHidden ? " nm-quote-hidden" : ""}`}
    >
      <EditorToolbar
        editor={editor}
        labels={labels}
        onInsertImage={imageInserter(editor, onInsertImage)}
        onLink={openLink}
        shortcuts={shortcuts}
      />
      {link === null || editor === null ? null : (
        <LinkBar
          editor={editor}
          initial={link}
          labels={labels}
          onClose={() => {
            setLink(null);
          }}
        />
      )}
      <div
        {...stylex.props(
          styles.content,
          contentFont(defaultFont, defaultFontSize)
        )}
      >
        <EditorContent editor={editor} />
        {quote.has ? (
          <div {...stylex.props(styles.quoteBar)}>
            <button
              aria-expanded={quoteShown}
              onClick={() => {
                setQuoteShown(!quoteShown);
              }}
              title={quoteShown ? labels.hideQuote : labels.showQuote}
              type="button"
              {...stylex.props(styles.quoteToggle)}
            >
              <Ellipsis aria-hidden size={16} />
              <span {...stylex.props(styles.quoteLabel)}>
                {quoteShown ? labels.hideQuote : labels.showQuote}
              </span>
            </button>
          </div>
        ) : null}
        {/* oxlint-disable-next-line jsx-a11y/click-events-have-key-events, jsx-a11y/no-static-element-interactions -- mouse-only convenience; the editor itself is the keyboard target. */}
        <div
          aria-hidden
          onClick={() => {
            editor?.commands.focus();
          }}
          {...stylex.props(styles.rest)}
        />
      </div>
    </div>
  );
};
