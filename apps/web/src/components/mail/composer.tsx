// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The floating compose windows, bottom right (Gmail/Proton): minimize to a
// chip, full screen, close (the draft stays saved). The body is the TipTap
// editor from @nebulamail/ui; pasted HTML is sanitized.

import {
  AlertDialog,
  Button,
  IconButton,
  Menu,
  MenuCheckboxItem,
  MenuItem,
  PlainTextEditor,
  RichTextEditor,
  Spinner,
  Tooltip,
} from "@nebulamail/ui";
import type { RichTextShortcuts } from "@nebulamail/ui";
import { colors, font, radius, shadow } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import {
  Check,
  ChevronDown,
  ChevronUp,
  CloudOff,
  Ellipsis,
  FileText,
  MailCheck,
  Maximize2,
  Minimize2,
  Minus,
  Paperclip,
  Save,
  Trash2,
  TriangleAlert,
  X,
} from "lucide-react";
import { Suspense, lazy, use, useEffect, useState } from "react";
import type { ReactNode } from "react";

import { formatClock } from "../../lib/format";
import { loadWhenOnline } from "../../lib/load-when-online";
import {
  initialBody,
  initialPlainText,
  plainSignature,
  sanitizePastedHtml,
} from "../../mail/body";
import type { Draft, Identity } from "../../mail/compose";
import { readComposePrefs } from "../../mail/compose-prefs";
import { useIdentities, useSavedIdentities } from "../../mail/hooks";
import { BODY_IMAGE_TYPES } from "../../mail/images";
import type { ServerRefusal } from "../../mail/refusal";
import type { ComposeInit } from "../../mail/reply";
import { resolveIdentity } from "../../mail/use-compose";
import { m } from "../../paraglide/messages.js";
import { getLocale } from "../../paraglide/runtime.js";
import { bp } from "./breakpoints.stylex";
import { ComposeAttachments } from "./compose-attachments";
import type { ComposeWindowState } from "./compose-context";
import { DockDrag } from "./composer-dock";
import type { ComposerFormProviderProps, Problem } from "./composer-form";
import {
  ComposerFormProvider,
  refusalText,
  useComposerForm,
} from "./composer-form";
import { editorLabels } from "./editor-labels";
import { RecipientField } from "./recipient-field";
import { ScheduleMenuItems } from "./schedule-menu";
import { shared } from "./styles";
import type { useKeys } from "./use-apple";

const styles = stylex.create({
  dropZone: {
    alignItems: "center",
    backgroundColor: `color-mix(in srgb, ${colors.raised} 88%, transparent)`,
    borderColor: colors.acc,
    borderRadius: radius.xl,
    borderStyle: "dashed",
    borderWidth: "2px",
    color: colors.accText,
    columnGap: "8px",
    display: "flex",
    fontSize: font.sizeLg,
    fontWeight: font.weightSemibold,
    inset: "6px",
    justifyContent: "center",
    pointerEvents: "none",
    position: "absolute",
    zIndex: 2,
  },
  chip: {
    alignItems: "center",
    backgroundColor: colors.raised,
    borderColor: colors.line2,
    borderRadius: { default: radius.lg, [bp.mobile]: 0 },
    borderStyle: "solid",
    borderWidth: "1px",
    boxShadow: shadow.sh2,
    columnGap: "6px",
    display: "flex",
    fontSize: font.sizeMd,
    fontWeight: font.weightSemibold,
    height: "40px",
    paddingInlineEnd: "6px",
    paddingInlineStart: "12px",
    pointerEvents: "auto",
    width: "260px",
  },
  chipTitle: {
    color: colors.fg,
    flexGrow: 1,
    textAlign: "start",
  },
  error: {
    alignItems: "center",
    color: colors.danger,
    columnGap: "6px",
    display: "flex",
    fontSize: font.sizeSm,
    paddingBottom: "6px",
    paddingInline: "16px",
  },
  icon: { flexShrink: 0 },
  offline: {
    alignItems: "center",
    backgroundColor: colors.warnSoft,
    color: colors.warn,
    columnGap: "8px",
    display: "flex",
    fontSize: font.sizeSm,
    margin: 0,
    paddingBlock: "8px",
    paddingInline: "16px",
  },
  fieldRow: {
    alignItems: "center",
    borderBottomColor: colors.line,
    borderBottomStyle: "solid",
    borderBottomWidth: "1px",
    columnGap: "8px",
    display: "flex",
    fontSize: font.sizeMd,
    minHeight: "44px",
    paddingInline: "16px",
  },
  fieldLabel: { color: colors.fg3, flexShrink: 0 },
  footer: {
    alignItems: "center",
    // Apart from the text above it, like the header below its title.
    borderTopColor: colors.line,
    borderTopStyle: "solid",
    borderTopWidth: "1px",
    columnGap: "8px",
    display: "flex",
    flexShrink: 0,
    paddingBottom: "14px",
    paddingInline: "14px",
    paddingTop: "12px",
  },
  form: {
    display: "flex",
    flexDirection: "column",
    flexGrow: 1,
    minHeight: 0,
    overflowY: "auto",
  },
  // A neutral hairline under the title (an accent streak there read as a
  // stray focus underline).
  header: {
    alignItems: "center",
    borderBottomColor: colors.line,
    borderBottomStyle: "solid",
    borderBottomWidth: "1px",
    columnGap: "2px",
    display: "flex",
    flexShrink: 0,
    height: "46px",
    // Dragged sideways by its title bar (not on phones: full screen).
    cursor: { default: "grab", [bp.mobile]: "auto" },
    touchAction: { default: "none", [bp.mobile]: "auto" },
    paddingInlineEnd: "8px",
    paddingInlineStart: "16px",
    position: "relative",
  },
  identity: { color: colors.fg, fontWeight: font.weightMedium },
  saved: {
    alignItems: "center",
    color: colors.fg3,
    columnGap: "5px",
    display: "inline-flex",
    fontSize: font.sizeSm,
  },
  savedIcon: { color: colors.ok },
  // Centred with the buttons (an inline span sits on the text baseline).
  savedSlot: { alignItems: "center", display: "inline-flex" },
  saveError: { color: colors.warn },
  spacer: { flexGrow: 1 },
  split: { display: "inline-flex" },
  // Typed text is semibold (it titles the message); the empty field reads
  // like the "To" label next to it, not like a bold title.
  subject: {
    "::placeholder": { color: colors.fg3, fontWeight: font.weightNormal },
    backgroundColor: "transparent",
    borderStyle: "none",
    color: colors.fg,
    flexGrow: 1,
    fontFamily: font.sans,
    fontSize: font.sizeMd,
    fontWeight: font.weightSemibold,
    minWidth: 0,
    outline: "none",
    paddingBlock: "10px",
    // No browser inset: the text starts on the 16px line with "To".
    paddingInline: 0,
  },
  title: {
    color: colors.fg,
    flexGrow: 1,
    fontSize: font.sizeMd,
    fontWeight: font.weightSemibold,
    margin: 0,
  },
  toggle: {
    borderRadius: radius.sm,
    color: { ":hover": colors.fg, default: colors.fg2 },
    fontSize: font.sizeMd,
    fontWeight: font.weightMedium,
    paddingBlock: "2px",
    paddingInline: "4px",
  },
  window: {
    backgroundColor: colors.raised,
    // The drop zone covers the window.
    position: "relative",
    borderColor: colors.line2,
    // A floating window (composer-dock.tsx keeps it off the edges).
    borderRadius: { default: radius.xl, [bp.mobile]: 0 },
    borderStyle: "solid",
    borderWidth: { default: "1px", [bp.mobile]: 0 },
    // sh3 carries the faint top highlight that lifts it off a dark reader.
    boxShadow: { default: shadow.sh3, [bp.mobile]: "none" },
    overflow: "hidden",
    boxSizing: "border-box",
    color: colors.fg,
    display: "flex",
    flexDirection: "column",
    // About 440px when empty, then it grows with the message up to the
    // screen; past that the form scrolls inside.
    height: { default: "auto", [bp.mobile]: "100dvh" },
    maxHeight: {
      default: "min(760px, calc(100dvh - 98px))",
      [bp.mobile]: "none",
    },
    minHeight: {
      default: "min(440px, calc(100dvh - 98px))",
      [bp.mobile]: 0,
    },
    // Full screen on phones, clear of the notch and the home bar.
    paddingBottom: { default: 0, [bp.mobile]: "env(safe-area-inset-bottom)" },
    paddingTop: { default: 0, [bp.mobile]: "env(safe-area-inset-top)" },
    pointerEvents: "auto",
    width: { default: "600px", [bp.mobile]: "100vw" },
  },
  // Phones are always full screen: "maximize" changes nothing there.
  maximized: {
    borderRadius: { default: radius.xl, [bp.mobile]: 0 },
    borderWidth: { default: "1px", [bp.mobile]: 0 },
    bottom: { default: "24px", [bp.mobile]: 0 },
    height: "auto",
    insetInline: {
      default: "max(24px, calc((100vw - 980px) / 2))",
      [bp.mobile]: 0,
    },
    position: "fixed",
    top: { default: "24px", [bp.mobile]: 0 },
    width: "auto",
  },
  wideOnly: { display: { default: "inline-flex", [bp.mobile]: "none" } },
});

const headerStatic = stylex.create({ value: { cursor: "auto" } });

/** A window's title bar: dragging it moves the windows sideways. */
const TitleBar = ({
  maximized,
  children,
}: {
  readonly maximized: boolean;
  readonly children: ReactNode;
}) => {
  const startDrag = use(DockDrag);
  return (
    <header
      onPointerDown={maximized || startDrag === null ? undefined : startDrag}
      {...stylex.props(styles.header, maximized ? headerStatic.value : null)}
    >
      {children}
    </header>
  );
};

const PROBLEM_TEXT: Record<Problem, () => string> = {
  "invalid-recipient": m.problem_invalid_recipient,
  "no-identity": m.compose_problem_offline,
  "no-recipients": m.problem_no_recipients,
  uploading: m.compose_problem_uploading,
};

const problemText = (problem: Problem, sendWaiting: boolean): string =>
  problem === "uploading" && sendWaiting
    ? m.compose_send_after_uploads()
    : PROBLEM_TEXT[problem]();

const saveErrorText = (
  refusal: ServerRefusal | null,
  backedUp: boolean
): string => {
  if (refusal !== null) {
    return refusalText(refusal);
  }
  return backedUp ? m.compose_saved_locally() : m.composer_save_error();
};

/** No identities known (opened offline): the sender is filled in later. */
const initialDraft = (
  init: ComposeInit,
  identities: readonly Identity[]
): Draft => {
  const identity = resolveIdentity(null, identities, init.identityEmail);
  const prefs = readComposePrefs();
  // Plain text when asked (a text-only draft or reply), or by default
  // (Settings) for anything that doesn't already come as HTML.
  const plain =
    init.plainText === true ||
    (prefs.format === "text" && init.html === undefined);
  return {
    bcc: init.bcc ?? [],
    cc: init.cc ?? [],
    attachments: init.attachments ?? [],
    bodyStyle: prefs.style,
    ...(plain
      ? {
          html: "",
          plainText: initialPlainText(
            init,
            identity === null ? null : plainSignature(identity)
          ),
        }
      : { html: initialBody(init, identity?.signature ?? null) }),
    identity,
    subject: init.subject ?? "",
    to: init.to ?? [],
    ...(init.inReplyTo === undefined ? {} : { inReplyTo: init.inReplyTo }),
    ...(init.references === undefined ? {} : { references: init.references }),
    ...(init.readReceipt === true ? { readReceipt: true } : {}),
  };
};

const identityLabel = (identity: Identity): string =>
  identity.name === ""
    ? identity.email
    : `${identity.name} <${identity.email}>`;

/** The editor's shortcuts (TipTap's), as shown in its tooltips. */
const editorShortcuts = (
  keys: ReturnType<typeof useKeys>
): RichTextShortcuts => ({
  alignCenter: keys("mod+shift+e"),
  alignJustify: keys("mod+shift+j"),
  alignLeft: keys("mod+shift+l"),
  alignRight: keys("mod+shift+r"),
  bold: keys("mod+b"),
  bulletList: keys("mod+shift+8"),
  clearFormatting: keys("mod+\\"),
  codeBlock: keys("mod+alt+c"),
  heading1: keys("mod+alt+1"),
  heading2: keys("mod+alt+2"),
  heading3: keys("mod+alt+3"),
  italic: keys("mod+i"),
  link: keys("mod+k"),
  orderedList: keys("mod+shift+7"),
  paragraph: keys("mod+alt+0"),
  quote: keys("mod+shift+b"),
  redo: keys("mod+shift+z"),
  strike: keys("mod+shift+s"),
  underline: keys("mod+u"),
  undo: keys("mod+z"),
});

/**
 * "From", when there is a choice; without identities (opened offline) a
 * notice that the message stays on this device for now.
 */
const FromField = ({
  identities,
  onChange,
  sender,
}: {
  readonly identities: readonly Identity[];
  readonly onChange: (identity: Identity) => void;
  readonly sender: Identity | null;
}) => {
  if (sender === null) {
    return (
      <output {...stylex.props(styles.offline)}>
        <CloudOff aria-hidden size={15} {...stylex.props(styles.icon)} />
        {m.compose_offline_no_identity()}
      </output>
    );
  }
  if (identities.length < 2) {
    return null;
  }
  return (
    <div {...stylex.props(styles.fieldRow)}>
      <span {...stylex.props(styles.fieldLabel)}>{m.composer_from()}</span>
      <Menu
        trigger={
          <Button
            aria-label={`${m.composer_from()}: ${identityLabel(sender)}`}
            iconEnd={ChevronDown}
            size="sm"
            variant="ghost"
          >
            {identityLabel(sender)}
          </Button>
        }
      >
        {identities.map((identity) => (
          <MenuItem
            key={identity.id}
            {...(identity.id === sender.id ? { icon: Check } : {})}
            onClick={() => {
              onChange(identity);
            }}
          >
            {identityLabel(identity)}
          </MenuItem>
        ))}
      </Menu>
    </div>
  );
};

/** The body: the rich editor, or a text area in plain-text mode. */
const ComposerBody = () => {
  const { actions, meta, state } = useComposerForm();
  const { draft } = state;
  const autoFocus = state.isReply ? "start" : false;
  const onSubmit = (): void => {
    actions.send();
  };
  return state.plain ? (
    <PlainTextEditor
      autoFocus={autoFocus}
      label={m.composer_body()}
      placeholder={m.composer_placeholder()}
      onChange={(plainText) => {
        actions.update({ plainText });
      }}
      onSubmit={onSubmit}
      value={draft.plainText ?? ""}
    />
  ) : (
    <RichTextEditor
      autoFocus={autoFocus}
      // Replies hide what they answer; a forward shows what it forwards.
      collapseQuote={state.isReply}
      defaultValue={draft.html}
      {...meta.richProps}
      label={m.composer_body()}
      labels={editorLabels()}
      placeholder={m.composer_placeholder()}
      // Serialized when needed, not on every keystroke (use-compose.ts).
      onEdit={(read) => {
        actions.editBody(read);
      }}
      onFiles={(files) => {
        actions.addFiles(files, true);
      }}
      onSubmit={onSubmit}
      sanitize={sanitizePastedHtml}
      shortcuts={editorShortcuts(meta.keys)}
      signature={draft.identity?.signature ?? null}
    />
  );
};

/** A minimized window: a chip with its title. */
const ComposerChip = () => {
  const { actions, state } = useComposerForm();
  return (
    <section
      aria-label={state.title}
      data-composer
      {...stylex.props(styles.chip)}
    >
      <FileText aria-hidden size={15} />
      <button
        onClick={() => {
          actions.restore();
        }}
        type="button"
        {...stylex.props(
          shared.resetButton,
          shared.ellipsis,
          shared.focusRing,
          styles.chipTitle
        )}
      >
        {state.title}
      </button>
      <IconButton
        icon={ChevronUp}
        label={m.composer_restore()}
        onClick={() => {
          actions.restore();
        }}
        size="xs"
      />
      <IconButton
        icon={X}
        label={m.composer_close()}
        onClick={() => {
          actions.close();
        }}
        size="xs"
      />
    </section>
  );
};

/** The open window: keys, dropped files and the drop zone. */
const ComposerFrame = ({ children }: { readonly children: ReactNode }) => {
  const { meta, state } = useComposerForm();
  return (
    // oxlint-disable-next-line jsx-a11y/no-noninteractive-element-interactions -- dropping files is a pointer-only shortcut; the Attach button is the keyboard way.
    <section
      aria-label={state.title}
      data-composer
      onDragLeave={(event) => {
        meta.onDragLeave(event);
      }}
      onKeyDown={(event) => {
        meta.onWindowKeyDown(event);
      }}
      onDragOver={(event) => {
        meta.onDragOver(event);
      }}
      onDrop={(event) => {
        meta.onDrop(event);
      }}
      {...stylex.props(
        styles.window,
        state.window.maximized ? styles.maximized : null
      )}
    >
      {state.dragging ? (
        <div aria-hidden {...stylex.props(styles.dropZone)}>
          <Paperclip aria-hidden size={20} />
          {m.compose_drop_files()}
        </div>
      ) : null}
      {children}
    </section>
  );
};

/** Title, minimize, full screen and close. */
const ComposerHeader = () => {
  const { actions, state } = useComposerForm();
  const { maximized } = state.window;
  return (
    <TitleBar maximized={maximized}>
      <h2 {...stylex.props(styles.title, shared.ellipsis)}>{state.title}</h2>
      <IconButton
        icon={Minus}
        label={m.composer_minimize()}
        onClick={() => {
          actions.minimize();
        }}
        size="sm"
      />
      <span {...stylex.props(styles.wideOnly)}>
        <IconButton
          icon={maximized ? Minimize2 : Maximize2}
          label={maximized ? m.composer_unmaximize() : m.composer_maximize()}
          onClick={() => {
            actions.toggleMaximized();
          }}
          size="sm"
        />
      </span>
      <IconButton
        disabled={state.closing}
        icon={X}
        label={m.composer_close()}
        onClick={() => {
          actions.close();
        }}
        size="sm"
      />
    </TitleBar>
  );
};

/** "Cc" and "Bcc" at the end of "To", until they are shown. */
const CcToggles = () => {
  const { actions, state } = useComposerForm();
  return (
    <>
      {state.showCc ? null : (
        <button
          aria-label={m.composer_add_cc()}
          onClick={() => {
            actions.revealCc();
          }}
          type="button"
          {...stylex.props(shared.resetButton, styles.toggle, shared.focusRing)}
        >
          {m.composer_cc()}
        </button>
      )}
      {state.showBcc ? null : (
        <button
          aria-label={m.composer_add_bcc()}
          onClick={() => {
            actions.revealBcc();
          }}
          type="button"
          {...stylex.props(shared.resetButton, styles.toggle, shared.focusRing)}
        >
          {m.composer_bcc()}
        </button>
      )}
    </>
  );
};

/** From, To, Cc, Bcc and Subject. */
const ComposerFields = () => {
  const { actions, meta, state } = useComposerForm();
  const { draft } = state;
  return (
    <>
      <FromField
        identities={state.identities}
        onChange={(identity) => {
          actions.setIdentity(identity);
        }}
        sender={draft.identity}
      />
      <RecipientField
        autoFocus={!state.isReply}
        end={<CcToggles />}
        label={m.composer_to()}
        onKeyDown={(event) => {
          meta.onFieldKeyDown(event);
        }}
        onChange={(to) => {
          actions.update({ to });
        }}
        value={draft.to}
      />
      {state.showCc ? (
        <RecipientField
          label={m.composer_cc()}
          onKeyDown={(event) => {
            meta.onFieldKeyDown(event);
          }}
          onChange={(cc) => {
            actions.update({ cc });
          }}
          value={draft.cc}
        />
      ) : null}
      {state.showBcc ? (
        <RecipientField
          label={m.composer_bcc()}
          onKeyDown={(event) => {
            meta.onFieldKeyDown(event);
          }}
          onChange={(bcc) => {
            actions.update({ bcc });
          }}
          value={draft.bcc}
        />
      ) : null}
      <div {...stylex.props(styles.fieldRow)}>
        <input
          aria-label={m.composer_subject()}
          onChange={(event) => {
            actions.update({ subject: event.target.value });
          }}
          onKeyDown={(event) => {
            meta.onFieldKeyDown(event);
          }}
          placeholder={m.composer_subject()}
          type="text"
          value={draft.subject}
          {...stylex.props(styles.subject)}
        />
      </div>
    </>
  );
};

/** Attached files and running uploads (inline images show in the body). */
const ComposerFiles = () => {
  const { actions, state } = useComposerForm();
  return (
    <ComposeAttachments
      attachments={state.draft.attachments.filter(
        (attachment) => attachment.cid === undefined
      )}
      onCancel={(upload) => {
        actions.cancelUpload(upload.key);
      }}
      onRemove={(attachment) => {
        actions.removeAttachment(attachment.blobId);
      }}
      uploads={state.uploads}
    />
  );
};

/** What blocks sending, once the user tried. */
const ComposerProblem = () => {
  const { state } = useComposerForm();
  return state.shownProblem === null ? null : (
    <p role="alert" {...stylex.props(styles.error)}>
      <TriangleAlert aria-hidden size={14} />
      {problemText(state.shownProblem, state.sendWaiting)}
    </p>
  );
};

/** "Saving…", "Saved at 10:42" or why it couldn't be saved. */
const SaveStatus = () => {
  const { save } = useComposerForm().state;
  switch (save.state) {
    case "saving": {
      return (
        <span {...stylex.props(styles.saved)}>
          <Spinner size="sm" />
          {m.composer_saving()}
        </span>
      );
    }
    case "error": {
      return (
        <span {...stylex.props(styles.saved, styles.saveError)}>
          <TriangleAlert aria-hidden size={14} />
          {saveErrorText(save.refusal, save.backedUp)}
        </span>
      );
    }
    case "idle":
    case "dirty":
    case "saved": {
      return save.savedAt === null ? null : (
        <span {...stylex.props(styles.saved)}>
          <Check aria-hidden size={14} {...stylex.props(styles.savedIcon)} />
          {m.composer_saved({
            time: formatClock(save.savedAt, getLocale()),
          })}
        </span>
      );
    }
    default: {
      return save.state satisfies never;
    }
  }
};

/** Send (and its menu: receipt, scheduled times). */
const SendButton = () => {
  const { actions, meta, state } = useComposerForm();
  const { draft, menuOpenedAt, scheduleLimit } = state;
  return (
    <span {...stylex.props(styles.split)}>
      {/* The key hint lives in the tooltip: inside the button it made Send
          twice as wide as its label. */}
      <Tooltip content={m.composer_send()} shortcut={meta.keys("mod+enter")}>
        <Button
          join="start"
          loading={state.sendBusy}
          onClick={() => {
            actions.send();
          }}
          variant="primary"
        >
          {m.composer_send()}
        </Button>
      </Tooltip>
      <Menu
        align="start"
        onOpenChange={(open) => {
          // The quick times count from when the menu opens.
          actions.setMenuOpenedAt(open ? new Date() : null);
        }}
        side="top"
        trigger={
          <Button
            aria-label={m.composer_send_options()}
            icon={ChevronDown}
            join="end"
            variant="primary"
          />
        }
      >
        {state.receipts ? (
          <MenuCheckboxItem
            checked={draft.readReceipt === true}
            icon={MailCheck}
            onCheckedChange={(readReceipt) => {
              actions.update({ readReceipt });
            }}
          >
            {m.composer_read_receipt()}
          </MenuCheckboxItem>
        ) : null}
        {scheduleLimit === null || menuOpenedAt === null ? null : (
          <ScheduleMenuItems
            limit={scheduleLimit}
            now={menuOpenedAt}
            onCustom={() => {
              actions.openSchedule();
            }}
            onPick={(at) => {
              actions.send(at);
            }}
            shortcut={meta.keys("mod+shift+enter")}
          />
        )}
      </Menu>
    </span>
  );
};

/** The hidden inputs behind "Attach" and "Insert image". */
const FileInputs = () => {
  const { actions, meta } = useComposerForm();
  const { fileInputRef, imageInputRef } = meta;
  return (
    <>
      <input
        hidden
        multiple
        onChange={(event) => {
          const files = [...(event.target.files ?? [])];
          // The same file can be picked again later.
          event.target.value = "";
          actions.addFiles(files);
        }}
        ref={fileInputRef}
        tabIndex={-1}
        type="file"
      />
      <input
        accept={BODY_IMAGE_TYPES.join(",")}
        data-testid="compose-image-input"
        hidden
        onChange={(event) => {
          const [file] = event.target.files ?? [];
          event.target.value = "";
          if (file !== undefined) {
            actions.addImage(file);
          }
        }}
        ref={imageInputRef}
        tabIndex={-1}
        type="file"
      />
    </>
  );
};

/** Send, attach, save, more options, the save state and discard. */
const ComposerFooter = () => {
  const { actions, meta, state } = useComposerForm();
  return (
    <footer {...stylex.props(styles.footer)}>
      <SendButton />
      <IconButton
        icon={Paperclip}
        label={m.compose_attach()}
        onClick={() => {
          meta.fileInputRef.current?.click();
        }}
        size="sm"
      />
      <IconButton
        icon={Save}
        label={m.composer_save_draft()}
        onClick={() => {
          actions.saveDraft();
        }}
        shortcut={meta.keys("mod+s")}
        size="sm"
      />
      <FileInputs />
      <Menu
        align="start"
        side="top"
        trigger={
          <IconButton
            icon={Ellipsis}
            label={m.composer_more_options()}
            size="sm"
          />
        }
      >
        <MenuCheckboxItem
          checked={state.plain}
          icon={FileText}
          onCheckedChange={(checked) => {
            actions.setPlain(checked);
          }}
        >
          {m.composer_plain_text()}
        </MenuCheckboxItem>
      </Menu>
      <span aria-live="polite" {...stylex.props(styles.savedSlot)}>
        <SaveStatus />
      </span>
      <span {...stylex.props(styles.spacer)} />
      <IconButton
        icon={Trash2}
        label={m.composer_discard()}
        onClick={() => {
          actions.discard();
        }}
        size="sm"
      />
    </footer>
  );
};

/** HTML → plain text loses the formatting: asked first. */
const PlainTextConfirm = () => {
  const { actions, state } = useComposerForm();
  return (
    <AlertDialog
      cancelLabel={m.composer_plain_cancel()}
      confirmLabel={m.composer_plain_confirm()}
      description={m.composer_plain_confirm_body()}
      onConfirm={() => {
        actions.toPlainText();
      }}
      onOpenChange={(open) => {
        actions.setConfirmPlain(open);
      }}
      open={state.confirmPlain}
      title={m.composer_plain_confirm_title()}
    />
  );
};

// The date picker (react-aria) loads when the dialog first opens.
const ScheduleDialog = lazy(async () => {
  const { ScheduleDialog: Dialog } = await loadWhenOnline(
    async () => await import("./schedule-send")
  );
  return { default: Dialog };
});

/** "Schedule send": pick the date and time. */
const ScheduleSend = () => {
  const { actions, state } = useComposerForm();
  // Mounted from the first opening on, so it can animate closing.
  const [used, setUsed] = useState(false);
  if (state.scheduleOpenedAt !== null && !used) {
    setUsed(true);
  }
  if (state.scheduleLimit === null || !used) {
    return null;
  }
  return (
    <Suspense fallback={null}>
      <ScheduleDialog
        busy={state.sending}
        limit={state.scheduleLimit}
        onClose={() => {
          actions.closeSchedule();
        }}
        onSubmit={(at) => {
          actions.closeSchedule();
          actions.send(at);
        }}
        openedAt={state.scheduleOpenedAt}
        submitLabel={m.schedule_submit()}
        title={m.schedule_title()}
      />
    </Suspense>
  );
};

/** The window's layout, composed from its parts. */
const ComposerLayout = () => {
  const { state } = useComposerForm();
  if (state.window.minimized) {
    return <ComposerChip />;
  }
  return (
    <ComposerFrame>
      <ComposerHeader />
      <div {...stylex.props(styles.form)}>
        <ComposerFields />
        <ComposerBody />
      </div>
      <ComposerFiles />
      <ComposerProblem />
      <ComposerFooter />
      <PlainTextConfirm />
      <ScheduleSend />
    </ComposerFrame>
  );
};

const ComposerForm = ({
  identities,
  initial,
  window,
}: Omit<ComposerFormProviderProps, "children">) => (
  <ComposerFormProvider
    identities={identities}
    initial={initial}
    window={window}
  >
    <ComposerLayout />
  </ComposerFormProvider>
);

/** One compose window, loaded lazily by the dock (composer-dock.tsx). */
export const ComposerWindow = ({
  window,
}: {
  readonly window: ComposeWindowState;
}) => {
  const live = useIdentities();
  const saved = useSavedIdentities();
  // The server's identities; until then the copy on this device. With
  // neither, wait only while the server may still answer: offline (paused)
  // or failed, the window opens anyway and the draft stays on the device.
  const list = live.data ?? saved.data ?? [];
  const waiting =
    live.data === undefined &&
    (saved.isPending || (list.length === 0 && live.fetchStatus === "fetching"));
  // Back online without the server's identities: ask again now (a query
  // that already failed isn't always retried on its own).
  const known = live.data !== undefined;
  const { refetch } = live;
  useEffect(() => {
    const retry = (): void => {
      if (!known) {
        void refetch();
      }
    };
    globalThis.addEventListener("online", retry);
    return () => {
      globalThis.removeEventListener("online", retry);
    };
  }, [known, refetch]);
  // Fixed once the form shows: a later refetch (back online) must not
  // bring the spinner back and remount it, losing what was written.
  const [initial, setInitial] = useState<Draft | null>(null);
  if (initial === null && !waiting) {
    setInitial(initialDraft(window.init, list));
  }
  if (initial === null) {
    return (
      <div data-composer {...stylex.props(styles.chip)}>
        <Spinner size="sm" />
      </div>
    );
  }
  return <ComposerForm identities={list} initial={initial} window={window} />;
};
