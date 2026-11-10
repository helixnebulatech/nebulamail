// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { Popover as BasePopover } from "@base-ui/react/popover";
import * as stylex from "@stylexjs/stylex";
import type { ReactElement, ReactNode } from "react";

import { cls, usePortalContainer } from "./internal";
import { colors, font, motion, shadow, zIndex } from "./tokens.stylex";

const styles = stylex.create({
  // The accent strip on top: this popover reports protection (privacy report).
  accent: {
    "::before": {
      backgroundColor: colors.acc,
      content: '""',
      height: "3px",
      insetInline: 0,
      position: "absolute",
      top: 0,
    },
    paddingTop: "4px",
  },
  description: {
    color: colors.fg3,
    display: "block",
    fontSize: font.sizeSm,
    margin: 0,
  },
  header: {
    alignItems: "center",
    columnGap: "10px",
    display: "flex",
    paddingBottom: "10px",
    paddingInline: "16px",
    paddingTop: "12px",
  },
  hidden: { opacity: 0, transform: "scale(0.96) translateY(-4px)" },
  md: { width: "390px" },
  popup: {
    backgroundColor: colors.raised,
    borderColor: colors.line2,
    borderRadius: "14px",
    borderStyle: "solid",
    borderWidth: "1px",
    boxShadow: shadow.sh3,
    boxSizing: "border-box",
    color: colors.fg,
    fontFamily: font.sans,
    fontSize: font.sizeMd,
    maxHeight: "var(--available-height)",
    maxWidth: "calc(100vw - 24px)",
    outline: "none",
    overflow: "hidden",
    overflowY: "auto",
    position: "relative",
    transformOrigin: "var(--transform-origin)",
    transitionDuration: motion.fast,
    transitionProperty: "opacity, transform",
    transitionTimingFunction: motion.easing,
  },
  positioner: { zIndex: zIndex.popover },
  sm: { width: "280px" },
  title: {
    display: "block",
    fontSize: "14.5px",
    fontWeight: font.weightSemibold,
    margin: 0,
  },
});

const sectionStyles = stylex.create({
  section: {
    borderTopColor: colors.line,
    borderTopStyle: "solid",
    borderTopWidth: "1px",
    paddingBlock: "10px",
    paddingInline: "16px",
  },
});

export interface PopoverRootProps {
  /** Popover.Trigger and Popover.Content. */
  readonly children: ReactNode;
  readonly open?: boolean;
  readonly onOpenChange?: (open: boolean) => void;
}

/** Holds the open state; the parts go inside. */
const PopoverRoot = ({ children, open, onOpenChange }: PopoverRootProps) => (
  <BasePopover.Root
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
  </BasePopover.Root>
);

/** Opens the popover: renders as `render` (a Button…). */
const PopoverTrigger = ({ render }: { readonly render: ReactElement }) => (
  <BasePopover.Trigger render={render} />
);

export interface PopoverContentProps {
  /** Popover.Header, then Popover.Section blocks or any content. */
  readonly children: ReactNode;
  /** "accent": the nebula strip on top (privacy report). */
  readonly variant?: "plain" | "accent";
  readonly size?: "sm" | "md";
  readonly side?: "top" | "bottom" | "left" | "right";
  readonly align?: "start" | "center" | "end";
}

/** The floating card: radius 14, elevation 3, next to its trigger. */
const PopoverContent = ({
  children,
  variant = "plain",
  size = "md",
  side = "bottom",
  align = "end",
}: PopoverContentProps) => {
  const container = usePortalContainer();
  return (
    <BasePopover.Portal container={container}>
      <BasePopover.Positioner
        align={align}
        className={cls(styles.positioner)}
        side={side}
        sideOffset={8}
      >
        <BasePopover.Popup
          className={(state) =>
            cls(
              styles.popup,
              styles[size],
              variant === "accent" ? styles.accent : null,
              state.transitionStatus === "starting" ||
                state.transitionStatus === "ending"
                ? styles.hidden
                : null
            )
          }
        >
          {children}
        </BasePopover.Popup>
      </BasePopover.Positioner>
    </BasePopover.Portal>
  );
};

/** The header row: an optional icon, then Popover.Title and Description. */
const PopoverHeader = ({
  children,
  icon,
}: {
  readonly children: ReactNode;
  /** E.g. a ShieldGlyph. */
  readonly icon?: ReactNode;
}) => (
  <div {...stylex.props(styles.header)}>
    {icon}
    <div>{children}</div>
  </div>
);

/** The heading (also the popover's accessible name). */
const PopoverTitle = ({ children }: { readonly children: ReactNode }) => (
  <BasePopover.Title className={cls(styles.title)}>
    {children}
  </BasePopover.Title>
);

/** The line under the title. */
const PopoverDescription = ({ children }: { readonly children: ReactNode }) => (
  <BasePopover.Description className={cls(styles.description)}>
    {children}
  </BasePopover.Description>
);

/** A block inside a popover, separated by a hairline. */
const PopoverSection = ({ children }: { readonly children: ReactNode }) => (
  <div {...stylex.props(sectionStyles.section)}>{children}</div>
);

/**
 * Non-modal floating card (privacy report, details) as compound parts
 * (composition rule "architecture-compound-components").
 *
 * ```tsx
 * <Popover.Root>
 *   <Popover.Trigger render={<Button>…</Button>} />
 *   <Popover.Content variant="accent">
 *     <Popover.Header icon={<ShieldGlyph size={22} />}>
 *       <Popover.Title>{m.title()}</Popover.Title>
 *     </Popover.Header>
 *     <Popover.Section>…</Popover.Section>
 *   </Popover.Content>
 * </Popover.Root>
 * ```
 */
export const Popover = {
  Content: PopoverContent,
  Description: PopoverDescription,
  Header: PopoverHeader,
  Root: PopoverRoot,
  Section: PopoverSection,
  Title: PopoverTitle,
  Trigger: PopoverTrigger,
};
