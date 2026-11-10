// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { Toast as BaseToast } from "@base-ui/react/toast";
import * as stylex from "@stylexjs/stylex";
import { AlertTriangle, Check, Info, X } from "lucide-react";
import type { ReactNode } from "react";

import { cls, shared, usePortalContainer } from "./internal";
import { colors, font, motion, radius, shadow, zIndex } from "./tokens.stylex";

export type ToastTone = "success" | "info" | "danger" | "neutral";

interface ToastData {
  readonly countdown: boolean;
  readonly closeLabel?: string;
}

export interface ToastOptions {
  readonly title: ReactNode;
  readonly description?: ReactNode;
  readonly tone?: ToastTone;
  /** e.g. `{ label: "Deshacer", onClick: undo }`. */
  readonly action?: { readonly label: string; readonly onClick: () => void };
  /** Milliseconds; 0 keeps it until dismissed. Default 5000. */
  readonly timeout?: number;
  /**
   * Accessible name of a close button ("Cerrar"). Give it to toasts that
   * don't time out (`timeout: 0`), or the user can't dismiss them.
   */
  readonly closeLabel?: string;
  /** Draws the shrinking line (undo send). Default: true when there is an action. */
  readonly countdown?: boolean;
  /** "high" announces assertively (errors). */
  readonly priority?: "low" | "high";
  /** Called when it closes (its close button, its action or the timeout). */
  readonly onClose?: () => void;
}

const DEFAULT_TIMEOUT = 5000;

const shrink = stylex.keyframes({
  from: { width: "100%" },
  to: { width: "0%" },
});

const styles = stylex.create({
  close: {
    alignItems: "center",
    backgroundColor: {
      ":hover": `color-mix(in srgb, ${colors.inkFg} 14%, transparent)`,
      default: "transparent",
    },
    borderRadius: radius.md,
    color: colors.inkFg,
    cursor: "pointer",
    display: "inline-flex",
    flexShrink: 0,
    height: "28px",
    justifyContent: "center",
    width: "28px",
  },
  action: {
    backgroundColor: {
      ":hover": `color-mix(in srgb, ${colors.inkFg} 22%, transparent)`,
      default: `color-mix(in srgb, ${colors.inkFg} 14%, transparent)`,
    },
    borderRadius: radius.md,
    color: colors.inkFg,
    cursor: "pointer",
    flexShrink: 0,
    fontWeight: 650,
    paddingBlock: "5px",
    paddingInline: "10px",
    whiteSpace: "nowrap",
  },
  bar: {
    animationFillMode: "forwards",
    animationName: shrink,
    animationPlayState: {
      default: "running",
      [stylex.when.ancestor(":hover")]: "paused",
      [stylex.when.ancestor(":focus-within")]: "paused",
    },
    animationTimingFunction: "linear",
    backgroundColor: colors.acc,
    bottom: 0,
    height: "3px",
    insetInlineStart: 0,
    position: "absolute",
  },
  barDuration: (ms: number) => ({ animationDuration: `${ms}ms` }),
  danger: { color: "#FF7A59" },
  description: {
    color: `color-mix(in srgb, ${colors.inkFg} 72%, transparent)`,
    fontSize: font.sizeSm,
    margin: 0,
  },
  hidden: { opacity: 0, transform: "translateY(12px) scale(0.98)" },
  icon: { flexShrink: 0 },
  info: { color: "#8CB0FF" },
  ok: { color: "#44D39A" },
  root: {
    alignItems: "center",
    backgroundColor: colors.ink,
    borderRadius: radius.lg,
    boxShadow: shadow.sh3,
    boxSizing: "border-box",
    color: colors.inkFg,
    columnGap: "12px",
    display: "flex",
    fontFamily: font.sans,
    fontSize: font.sizeMd,
    maxWidth: "calc(100vw - 32px)",
    minWidth: "320px",
    overflow: "hidden",
    paddingBlock: "10px",
    paddingInlineEnd: "10px",
    paddingInlineStart: "14px",
    position: "relative",
    transitionDuration: motion.normal,
    transitionProperty: "opacity, transform",
    transitionTimingFunction: motion.easing,
    width: "max-content",
  },
  text: { display: "flex", flexDirection: "column", flexGrow: 1, minWidth: 0 },
  title: {
    fontSize: font.sizeMd,
    fontWeight: font.weightMedium,
    lineHeight: font.lineHeight,
    margin: 0,
  },
  viewport: {
    bottom: "24px",
    display: "flex",
    flexDirection: "column-reverse",
    insetInlineStart: "24px",
    outline: "none",
    position: "fixed",
    rowGap: "8px",
    zIndex: zIndex.toast,
  },
});

// The toast is always dark ink, in both themes; status colours are the
// dark-theme ones so they read on it.
const TONE_ICON = {
  danger: { glyph: AlertTriangle, style: styles.danger },
  info: { glyph: Info, style: styles.info },
  neutral: null,
  success: { glyph: Check, style: styles.ok },
} as const;

const isTone = (value: string | undefined): value is ToastTone =>
  value === "success" ||
  value === "info" ||
  value === "danger" ||
  value === "neutral";

const ToastList = () => {
  const { toasts } = BaseToast.useToastManager<ToastData>();
  return toasts.map((toast) => {
    const tone = isTone(toast.type) ? toast.type : "neutral";
    const icon = TONE_ICON[tone];
    const timeout = toast.timeout ?? DEFAULT_TIMEOUT;
    const countdown = toast.data?.countdown === true && timeout > 0;
    return (
      <BaseToast.Root
        className={(state) =>
          cls(
            // Marker: the countdown pauses while the toast is hovered/focused.
            stylex.defaultMarker(),
            styles.root,
            state.transitionStatus === "starting" ||
              state.transitionStatus === "ending"
              ? styles.hidden
              : null
          )
        }
        key={toast.id}
        toast={toast}
      >
        {icon === null ? null : (
          <icon.glyph
            aria-hidden
            size={16}
            {...stylex.props(styles.icon, icon.style)}
          />
        )}
        <div {...stylex.props(styles.text)}>
          <BaseToast.Title className={cls(styles.title)} />
          <BaseToast.Description className={cls(styles.description)} />
        </div>
        {toast.actionProps === undefined ? null : (
          <BaseToast.Action
            className={cls(shared.resetButton, styles.action, shared.focusRing)}
          />
        )}
        {toast.data?.closeLabel === undefined ? null : (
          <BaseToast.Close
            aria-label={toast.data.closeLabel}
            className={cls(shared.resetButton, styles.close, shared.focusRing)}
          >
            <X aria-hidden size={16} />
          </BaseToast.Close>
        )}
        {countdown ? (
          <span
            aria-hidden
            {...stylex.props(styles.bar, styles.barDuration(timeout))}
          />
        ) : null}
      </BaseToast.Root>
    );
  });
};

export interface ToastProviderProps {
  readonly children: ReactNode;
  /** Max visible toasts. */
  readonly limit?: number;
}

/** Put once near the root; then call `useToast()` anywhere below. */
export const ToastProvider = ({ children, limit = 3 }: ToastProviderProps) => {
  const container = usePortalContainer();
  return (
    <BaseToast.Provider limit={limit} timeout={DEFAULT_TIMEOUT}>
      {children}
      <BaseToast.Portal container={container}>
        <BaseToast.Viewport className={cls(styles.viewport)}>
          <ToastList />
        </BaseToast.Viewport>
      </BaseToast.Portal>
    </BaseToast.Provider>
  );
};

export interface ToastApi {
  /** Shows a toast; returns its id. */
  readonly show: (options: ToastOptions) => string;
  readonly dismiss: (id: string) => void;
}

/** Ink toasts with an optional action ("Deshacer") and countdown line. */
export const useToast = (): ToastApi => {
  const manager = BaseToast.useToastManager<ToastData>();
  return {
    dismiss: (id) => {
      manager.close(id);
    },
    show: ({
      title,
      description,
      tone = "neutral",
      action,
      timeout = DEFAULT_TIMEOUT,
      countdown,
      closeLabel,
      priority = "low",
      onClose,
    }) => {
      // Set right after `add`; the action closes its own toast.
      let id = "";
      id = manager.add({
        data: {
          countdown: countdown ?? action !== undefined,
          ...(closeLabel === undefined ? {} : { closeLabel }),
        },
        priority,
        timeout,
        title,
        type: tone,
        ...(description === undefined ? {} : { description }),
        ...(onClose === undefined ? {} : { onClose }),
        ...(action === undefined
          ? {}
          : {
              actionProps: {
                children: action.label,
                onClick: () => {
                  action.onClick();
                  manager.close(id);
                },
              },
            }),
      });
      return id;
    },
  };
};
