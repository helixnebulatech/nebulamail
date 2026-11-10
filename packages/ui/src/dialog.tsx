// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { AlertDialog as BaseAlertDialog } from "@base-ui/react/alert-dialog";
import { Dialog as BaseDialog } from "@base-ui/react/dialog";
import * as stylex from "@stylexjs/stylex";
import { X } from "lucide-react";
import { useRef } from "react";
import type { ReactElement, ReactNode } from "react";

import { Button, IconButton } from "./button";
import { cls, shared, usePortalContainer } from "./internal";
import {
  colors,
  font,
  motion,
  radius,
  shadow,
  space,
  zIndex,
} from "./tokens.stylex";

const styles = stylex.create({
  backdrop: {
    backdropFilter: "blur(3px)",
    backgroundColor: colors.scrim,
    inset: 0,
    position: "fixed",
    transitionDuration: motion.normal,
    transitionProperty: "opacity",
    transitionTimingFunction: motion.easing,
    zIndex: zIndex.overlay,
  },
  backdropHidden: { opacity: 0 },
  body: {
    color: colors.fg2,
    fontSize: font.sizeMd,
    lineHeight: font.lineHeight,
  },
  close: { insetInlineEnd: "10px", position: "absolute", top: "10px" },
  description: {
    color: colors.fg2,
    fontSize: font.sizeMd,
    lineHeight: font.lineHeight,
    marginBlock: "6px 0",
    marginInline: 0,
  },
  footer: {
    columnGap: space.sm,
    display: "flex",
    justifyContent: "flex-end",
    marginTop: space.s20,
  },
  hidden: { opacity: 0, transform: "scale(0.96)" },
  popup: {
    backgroundColor: colors.raised,
    borderColor: colors.line2,
    borderRadius: radius.xl,
    borderStyle: "solid",
    borderWidth: "1px",
    boxShadow: shadow.sh3,
    boxSizing: "border-box",
    color: colors.fg,
    fontFamily: font.sans,
    // Centred without transforms: symmetric, so RTL-safe.
    height: "fit-content",
    inset: 0,
    margin: "auto",
    maxHeight: "calc(100dvh - 48px)",
    maxWidth: "calc(100vw - 32px)",
    outline: "none",
    overflowY: "auto",
    paddingBlock: "20px",
    paddingInline: "22px",
    position: "fixed",
    transitionDuration: motion.normal,
    transitionProperty: "opacity, transform",
    transitionTimingFunction: motion.easing,
    zIndex: zIndex.dialog,
  },
  // Command palettes: anchored near the top, so the box doesn't jump
  // around while its list grows and shrinks with the query.
  top: { marginBlock: "min(12dvh, 120px) auto" },
  sm: { width: "400px" },
  md: { width: "520px" },
  lg: { width: "680px" },
  title: {
    fontSize: "17px",
    fontWeight: font.weightBold,
    letterSpacing: "-0.02em",
    lineHeight: 1.3,
    margin: 0,
    paddingInlineEnd: "28px",
  },
});

const hiddenFor = (status: string | undefined) =>
  status === "starting" || status === "ending";

export interface DialogRootProps {
  /** Dialog.Trigger (optional when controlled) and Dialog.Content. */
  readonly children: ReactNode;
  readonly open?: boolean;
  readonly onOpenChange?: (open: boolean) => void;
}

/** Holds the open state; the parts go inside. */
const DialogRoot = ({ children, open, onOpenChange }: DialogRootProps) => (
  <BaseDialog.Root
    {...(open === undefined ? {} : { open })}
    {...(onOpenChange === undefined
      ? {}
      : {
          onOpenChange: (next: boolean) => {
            onOpenChange(next);
          },
        })}
  >
    {children}
  </BaseDialog.Root>
);

/** Opens the dialog: renders as `render` (a Button…). */
const DialogTrigger = ({ render }: { readonly render: ReactElement }) => (
  <BaseDialog.Trigger render={render} />
);

export interface DialogContentProps {
  /** Dialog.Title first, then Description, Body and Footer as needed. */
  readonly children: ReactNode;
  /** Accessible name of the close button, e.g. "Cerrar". */
  readonly closeLabel: string;
  readonly size?: "sm" | "md" | "lg";
  /** "top" for command palettes (see styles.top). */
  readonly placement?: "center" | "top";
  /**
   * The ✕ button. Off for palettes, where Esc is the way out (it is
   * still announced by the dialog role).
   */
  readonly closeButton?: boolean;
}

const TABBABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"]), [contenteditable="true"]';

/**
 * Where focus goes when the dialog opens: its first field or control, as
 * Base UI does, unless that would be the ✕ (a dialog with only text and
 * buttons at the end, like the shortcuts sheet). Then the card itself takes
 * focus, so no focus ring flashes on ✕ and Tab still reaches it next.
 */
const pickInitialFocus = (popup: HTMLElement | null): HTMLElement | true => {
  if (popup === null) {
    return true;
  }
  const first = popup.querySelector<HTMLElement>(TABBABLE);
  return first === null || first.closest("[data-dialog-close]") !== null
    ? popup
    : true;
};

/** The modal card: scrim with blur, raised surface, radius 16, close. */
const DialogContent = ({
  children,
  closeButton = true,
  closeLabel,
  placement = "center",
  size = "md",
}: DialogContentProps) => {
  const container = usePortalContainer();
  const popupRef = useRef<HTMLDivElement>(null);
  return (
    <BaseDialog.Portal container={container}>
      <BaseDialog.Backdrop
        className={(state) =>
          cls(
            styles.backdrop,
            hiddenFor(state.transitionStatus) ? styles.backdropHidden : null
          )
        }
      />
      <BaseDialog.Popup
        initialFocus={() => pickInitialFocus(popupRef.current)}
        ref={popupRef}
        className={(state) =>
          cls(
            styles.popup,
            styles[size],
            placement === "top" ? styles.top : null,
            hiddenFor(state.transitionStatus) ? styles.hidden : null
          )
        }
      >
        {children}
        {closeButton ? (
          <span data-dialog-close {...stylex.props(styles.close)}>
            <BaseDialog.Close
              render={
                <IconButton
                  icon={X}
                  label={closeLabel}
                  size="sm"
                  tooltip={false}
                />
              }
            />
          </span>
        ) : null}
      </BaseDialog.Popup>
    </BaseDialog.Portal>
  );
};

/**
 * The heading (also the dialog's accessible name). `visuallyHidden` keeps
 * it for screen readers only (command palettes, whose input says it all).
 */
const DialogTitle = ({
  children,
  visuallyHidden = false,
}: {
  readonly children: ReactNode;
  readonly visuallyHidden?: boolean;
}) => (
  <BaseDialog.Title
    className={cls(visuallyHidden ? shared.visuallyHidden : styles.title)}
  >
    {children}
  </BaseDialog.Title>
);

/** The line under the title (the dialog's accessible description). */
const DialogDescription = ({ children }: { readonly children: ReactNode }) => (
  <BaseDialog.Description className={cls(styles.description)}>
    {children}
  </BaseDialog.Description>
);

/** The content between the header and the buttons. */
const DialogBody = ({ children }: { readonly children: ReactNode }) => (
  <div {...stylex.props(styles.body)}>{children}</div>
);

/** Buttons at the bottom (end-aligned). */
const DialogFooter = ({ children }: { readonly children: ReactNode }) => (
  <div {...stylex.props(styles.footer)}>{children}</div>
);

/**
 * Modal dialog as compound parts (composition rule
 * "architecture-compound-components"): the caller composes only what it
 * shows, with no title/description/footer props.
 *
 * ```tsx
 * <Dialog.Root open={open} onOpenChange={setOpen}>
 *   <Dialog.Content closeLabel={m.close()} size="sm">
 *     <Dialog.Title>{m.title()}</Dialog.Title>
 *     <Dialog.Body>…</Dialog.Body>
 *     <Dialog.Footer><Button>…</Button></Dialog.Footer>
 *   </Dialog.Content>
 * </Dialog.Root>
 * ```
 */
export const Dialog = {
  Body: DialogBody,
  Content: DialogContent,
  Description: DialogDescription,
  Footer: DialogFooter,
  Root: DialogRoot,
  Title: DialogTitle,
  Trigger: DialogTrigger,
};

export interface AlertDialogProps {
  readonly trigger?: ReactElement;
  readonly title: ReactNode;
  readonly description: ReactNode;
  readonly confirmLabel: string;
  readonly cancelLabel: string;
  readonly onConfirm: () => void;
  /** "danger" for destructive confirmations (Eliminar para siempre). */
  readonly tone?: "primary" | "danger";
  readonly open?: boolean;
  readonly onOpenChange?: (open: boolean) => void;
}

/**
 * Confirmation that must be answered (no outside-click dismiss): an
 * explicit variant with a fixed shape (title, text, Cancel and Confirm).
 */
export const AlertDialog = ({
  trigger,
  title,
  description,
  confirmLabel,
  cancelLabel,
  onConfirm,
  tone = "primary",
  open,
  onOpenChange,
}: AlertDialogProps) => {
  const container = usePortalContainer();
  return (
    <BaseAlertDialog.Root
      {...(open === undefined ? {} : { open })}
      {...(onOpenChange === undefined
        ? {}
        : {
            onOpenChange: (next: boolean) => {
              onOpenChange(next);
            },
          })}
    >
      {trigger === undefined ? null : (
        <BaseAlertDialog.Trigger render={trigger} />
      )}
      <BaseAlertDialog.Portal container={container}>
        <BaseAlertDialog.Backdrop
          className={(state) =>
            cls(
              styles.backdrop,
              hiddenFor(state.transitionStatus) ? styles.backdropHidden : null
            )
          }
        />
        <BaseAlertDialog.Popup
          className={(state) =>
            cls(
              styles.popup,
              styles.sm,
              hiddenFor(state.transitionStatus) ? styles.hidden : null
            )
          }
        >
          <BaseAlertDialog.Title className={cls(styles.title)}>
            {title}
          </BaseAlertDialog.Title>
          <BaseAlertDialog.Description className={cls(styles.description)}>
            {description}
          </BaseAlertDialog.Description>
          <div {...stylex.props(styles.footer)}>
            <BaseAlertDialog.Close render={<Button>{cancelLabel}</Button>} />
            <BaseAlertDialog.Close
              render={
                <Button onClick={onConfirm} variant={tone}>
                  {confirmLabel}
                </Button>
              }
            />
          </div>
        </BaseAlertDialog.Popup>
      </BaseAlertDialog.Portal>
    </BaseAlertDialog.Root>
  );
};
