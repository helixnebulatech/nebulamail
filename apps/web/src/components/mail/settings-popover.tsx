// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Quick settings: theme, density, language, the "Verified sender" badge and
// new-mail notifications (off by default; enabling asks for permission).
// Theme and density go to the small preferences cookie so the server renders
// them without a flash; the language is Paraglide's cookie;
// the badge is a per-device flag the admin can lock (use-verified-badge.ts).
// The controls are shared with the full settings page (/settings/general).

import {
  Badge,
  Dialog,
  IconButton,
  LinkButton,
  Popover,
  SegmentedControl,
  Switch,
  TextLink,
  themeProps,
} from "@nebulamail/ui";
import { colors, font } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { Link, useRouteContext, useRouter } from "@tanstack/react-router";
import { Lock, Settings } from "lucide-react";
import type { ComponentType, ReactNode } from "react";

import type { Preferences } from "../../lib/preferences";
import { savePreferences } from "../../lib/preferences";
import { useNewMailNotifications } from "../../mail/use-notifications";
import type { NotifyPermission } from "../../mail/use-notifications";
import { useVerifiedBadge } from "../../mail/use-verified-badge";
import { m } from "../../paraglide/messages.js";
import { getLocale, setLocale } from "../../paraglide/runtime.js";
import type { Locale } from "../../paraglide/runtime.js";
import { recordChange } from "../../prefs-sync/sync";
import { bp } from "./breakpoints.stylex";

const styles = stylex.create({
  // Phones always show one panel at a time: where emails open has no
  // effect there.
  notOnPhones: { display: { default: "contents", [bp.mobile]: "none" } },
  labelWithChip: {
    alignItems: "center",
    columnGap: "8px",
    display: "inline-flex",
    flexWrap: "wrap",
    rowGap: "4px",
  },
  risks: {
    color: colors.fg2,
    fontSize: font.sizeMd,
    lineHeight: 1.5,
    margin: 0,
    paddingInlineStart: "20px",
  },
  risksItem: { marginBottom: "8px" },
  label: {
    color: colors.fg2,
    display: "block",
    fontSize: font.sizeSm,
    fontWeight: font.weightSemibold,
    marginBottom: "6px",
  },
});

export const THEMES = [
  { label: m.theme_light, value: "light" },
  { label: m.theme_dark, value: "dark" },
  { label: m.theme_system, value: "system" },
] as const;

export const DENSITIES = [
  { label: m.density_compact, value: "compact" },
  { label: m.density_comfortable, value: "comfortable" },
] as const;

/** Where emails open (Preferences.pane). */
export const PANES = [
  { label: m.pane_side, value: "side" },
  { label: m.pane_full, value: "full" },
] as const;

export const LANGUAGES = [
  { label: m.language_es, value: "es" },
  { label: m.language_en, value: "en" },
] as const satisfies readonly {
  readonly label: () => string;
  readonly value: Locale;
}[];

/**
 * Light ↔ dark as a crossfade (View Transitions API), where the browser has
 * it and the user doesn't ask for less motion. The new theme goes on
 * <html> inside the transition, so the "after" picture is right at once;
 * the root re-renders with the same classes (routes/__root.tsx).
 */
const crossfadeTheme = (next: Preferences): void => {
  if (
    typeof document.startViewTransition !== "function" ||
    globalThis.matchMedia("(prefers-reduced-motion: reduce)").matches
  ) {
    return;
  }
  document.startViewTransition(() => {
    const { className } = themeProps(next.theme, next.density);
    if (className !== undefined) {
      document.documentElement.className = className;
    }
  });
};

/** Current preferences and a setter that re-renders with the new theme. */
export const usePreferences = (): {
  readonly preferences: Preferences;
  readonly setPreferences: (patch: Partial<Preferences>) => void;
  readonly setLanguage: (locale: Locale) => void;
} => {
  const router = useRouter();
  const { preferences } = useRouteContext({ from: "__root__" });
  return {
    preferences,
    setLanguage: (locale) => {
      if (locale !== getLocale()) {
        // Synced on the next start (the page reloads now).
        recordChange("language", locale === "es" ? "es" : "en");
        // Saves the cookie and reloads: every string is server-rendered.
        void setLocale(locale);
      }
    },
    setPreferences: (patch) => {
      const next = { ...preferences, ...patch };
      savePreferences(next);
      if (patch.theme !== undefined && patch.theme !== preferences.theme) {
        crossfadeTheme(next);
      }
      if (patch.theme !== undefined) {
        recordChange("theme", patch.theme);
      }
      if (patch.density !== undefined) {
        recordChange("density", patch.density);
      }
      if (patch.pane !== undefined) {
        recordChange("pane", patch.pane);
      }
      void router.invalidate();
    },
  };
};

// The segmented control carries the accessible name; this is the visual one.
const Group = ({
  children,
  label,
}: {
  readonly children: ReactNode;
  readonly label: string;
}) => (
  <Popover.Section>
    <span aria-hidden {...stylex.props(styles.label)}>
      {label}
    </span>
    {children}
  </Popover.Section>
);

// The dangers of the green badge, in plain words (docs/verified-sender.md).
// Outside the popover so it stays open when the popover closes.
const RISKS = [
  m.verified_risks_server,
  m.verified_risks_old,
  m.verified_risks_meaning,
  m.verified_risks_name,
  m.verified_risks_warnings,
] as const;

export const VerifiedRisks = ({
  onOpenChange,
  open,
}: {
  readonly onOpenChange: (open: boolean) => void;
  readonly open: boolean;
}) => (
  <Dialog.Root open={open} onOpenChange={onOpenChange}>
    <Dialog.Content closeLabel={m.close()} size="md">
      <Dialog.Title>{m.verified_risks_title()}</Dialog.Title>
      <Dialog.Description>{m.verified_risks_intro()}</Dialog.Description>
      <Dialog.Body>
        <ul {...stylex.props(styles.risks)}>
          {RISKS.map((risk) => (
            <li key={risk()} {...stylex.props(styles.risksItem)}>
              {risk()}
            </li>
          ))}
        </ul>
      </Dialog.Body>
    </Dialog.Content>
  </Dialog.Root>
);

const notificationsHint = (
  permission: NotifyPermission,
  closedTabs: boolean | undefined
): string => {
  switch (permission) {
    case "unsupported": {
      return m.settings_notifications_unsupported();
    }
    case "denied": {
      return m.settings_notifications_denied();
    }
    case "default":
    case "granted": {
      return closedTabs === true
        ? m.settings_notifications_hint_closed()
        : m.settings_notifications_hint();
    }
    default: {
      return permission satisfies never;
    }
  }
};

/** Theme, density and language: each a labelled segmented control. */
export const AppearanceControls = ({
  group: GroupComponent,
}: {
  /** Wraps each control with its visible label. */
  readonly group: ComponentType<{
    readonly label: string;
    readonly children: ReactNode;
  }>;
}) => {
  const { preferences, setLanguage, setPreferences } = usePreferences();
  return (
    <>
      <GroupComponent label={m.settings_theme()}>
        <SegmentedControl
          label={m.settings_theme()}
          onValueChange={(theme) => {
            setPreferences({ theme });
          }}
          options={THEMES.map((option) => ({
            label: option.label(),
            value: option.value,
          }))}
          value={preferences.theme}
        />
      </GroupComponent>
      <GroupComponent label={m.settings_density()}>
        <SegmentedControl
          label={m.settings_density()}
          onValueChange={(density) => {
            setPreferences({ density });
          }}
          options={DENSITIES.map((option) => ({
            label: option.label(),
            value: option.value,
          }))}
          value={preferences.density}
        />
      </GroupComponent>
      <div {...stylex.props(styles.notOnPhones)}>
        <GroupComponent label={m.settings_pane()}>
          <SegmentedControl
            label={m.settings_pane()}
            onValueChange={(pane) => {
              setPreferences({ pane });
            }}
            options={PANES.map((option) => ({
              label: option.label(),
              value: option.value,
            }))}
            value={preferences.pane}
          />
        </GroupComponent>
      </div>
      <GroupComponent label={m.settings_language()}>
        <SegmentedControl
          label={m.settings_language()}
          onValueChange={setLanguage}
          options={LANGUAGES.map((option) => ({
            label: option.label(),
            value: option.value,
          }))}
          value={getLocale()}
        />
      </GroupComponent>
    </>
  );
};

/** The "Verified sender" switch, its hint and the risks dialog. */
export const VerifiedBadgeControl = ({
  onShowRisks,
}: {
  readonly onShowRisks: () => void;
}) => {
  const badge = useVerifiedBadge();
  return (
    <Switch
      checked={badge.enabled}
      description={
        <>
          {badge.forced
            ? m.settings_verified_badge_forced()
            : m.settings_verified_badge_hint()}{" "}
          <LinkButton onClick={onShowRisks}>
            {m.settings_verified_badge_risks()}
          </LinkButton>
        </>
      }
      disabled={badge.forced}
      label={m.settings_verified_badge()}
      onCheckedChange={(checked) => {
        badge.setEnabled(checked);
      }}
    />
  );
};

/** New-mail notifications switch and its hint. */
export const NotificationsControl = () => {
  const notifications = useNewMailNotifications();
  let blocked: string | null = null;
  if (notifications.permission === "denied") {
    blocked = m.settings_notifications_blocked_chip();
  } else if (notifications.permission === "unsupported") {
    blocked = m.settings_notifications_unsupported_chip();
  }
  return (
    <Switch
      checked={notifications.enabled}
      description={notificationsHint(
        notifications.permission,
        notifications.closedTabs
      )}
      disabled={blocked !== null}
      label={
        blocked === null ? (
          m.settings_notifications()
        ) : (
          <span {...stylex.props(styles.labelWithChip)}>
            {m.settings_notifications()}
            <Badge icon={Lock} tone="warn">
              {blocked}
            </Badge>
          </span>
        )
      }
      onCheckedChange={(checked) => {
        void notifications.setEnabled(checked);
      }}
    />
  );
};

// The "Verified sender" badge is only in Settings › General: it needs its
// explanation and risks, too much for a quick setting.
export const SettingsPopover = () => (
  <Popover.Root>
    <Popover.Trigger
      render={<IconButton icon={Settings} label={m.settings_label()} />}
    />
    <Popover.Content size="sm">
      <Popover.Header>
        <Popover.Title>{m.settings_title()}</Popover.Title>
        <Popover.Description>{m.settings_description()}</Popover.Description>
      </Popover.Header>
      <AppearanceControls group={Group} />
      <Popover.Section>
        <NotificationsControl />
      </Popover.Section>
      <Popover.Section>
        <TextLink render={<Link to="/settings/general" />}>
          {m.settings_all()}
        </TextLink>
      </Popover.Section>
    </Popover.Content>
  </Popover.Root>
);
