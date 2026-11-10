// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// General settings: the same controls as the quick settings popover.

import { Switch } from "@nebulamail/ui";
import { useState } from "react";
import type { ReactNode } from "react";

import { useLocalFlag } from "../../lib/use-local-flag";
import { m } from "../../paraglide/messages.js";
import { MAIL_FALLBACK_FLAG, syncPreferences } from "../../prefs-sync/sync";
import type { SyncStatus } from "../../prefs-sync/sync";
import { useSyncStatus } from "../../prefs-sync/use-prefs-sync";
import { RemindersControl } from "../calendar/reminders";
import {
  AppearanceControls,
  NotificationsControl,
  VerifiedBadgeControl,
  VerifiedRisks,
} from "../mail/settings-popover";
import { NewSenderControl } from "./new-sender-control";
import { PreloadControl } from "./preload-control";
import {
  SectionHeader,
  SettingGroup,
  SettingRow,
  SettingsBlock,
} from "./section";
import { SendSignedOutControl } from "./send-signed-out-control";
import { WritingControls } from "./writing-control";

const statusText = (status: SyncStatus, mailAllowed: boolean): string => {
  switch (status.state) {
    case "synced": {
      return status.backend === "filenode"
        ? m.settings_sync_files()
        : m.settings_sync_mail_on();
    }
    case "error": {
      return m.settings_sync_error();
    }
    case "local": {
      return mailAllowed ? m.settings_sync_error() : m.settings_sync_local();
    }
    case "idle":
    case "syncing": {
      return m.settings_sync_checking();
    }
    default: {
      return status satisfies never;
    }
  }
};

/** Where the preferences are kept, and the opt-in mail-folder fallback. */
const SyncControl = () => {
  const status = useSyncStatus();
  const [mailAllowed, setMailAllowed] = useLocalFlag(MAIL_FALLBACK_FLAG, false);
  const files = status.state === "synced" && status.backend === "filenode";
  return (
    <SettingGroup>
      <SettingRow
        description={
          <span aria-live="polite" data-sync-state={status.state}>
            {statusText(status, mailAllowed)}
          </span>
        }
        label={m.settings_sync_status()}
      />
      {files ? null : (
        <Switch
          checked={mailAllowed}
          description={m.settings_sync_mail_hint()}
          label={m.settings_sync_mail()}
          onCheckedChange={(checked) => {
            setMailAllowed(checked);
            void syncPreferences();
          }}
        />
      )}
    </SettingGroup>
  );
};

/** A segmented control as a settings row (theme, density…). */
const AppearanceRow = ({
  children,
  label,
}: {
  readonly label: string;
  readonly children: ReactNode;
}) => (
  <SettingGroup>
    <SettingRow label={label}>{children}</SettingRow>
  </SettingGroup>
);

export const GeneralSection = () => {
  const [risksOpen, setRisksOpen] = useState(false);
  return (
    <>
      <SectionHeader
        description={m.settings_description()}
        title={m.settings_section_general()}
      />
      <SettingsBlock first title={m.settings_group_appearance()}>
        <AppearanceControls group={AppearanceRow} />
      </SettingsBlock>
      <SettingsBlock title={m.settings_group_reading()}>
        <SettingGroup>
          <VerifiedBadgeControl
            onShowRisks={() => {
              setRisksOpen(true);
            }}
          />
        </SettingGroup>
        <NewSenderControl />
      </SettingsBlock>
      <SettingsBlock title={m.settings_privacy()}>
        <PreloadControl />
      </SettingsBlock>
      <SettingsBlock title={m.settings_writing()}>
        <WritingControls />
        <SendSignedOutControl />
      </SettingsBlock>
      <SettingsBlock title={m.settings_group_notifications()}>
        <SettingGroup>
          <NotificationsControl />
        </SettingGroup>
        <SettingGroup>
          <RemindersControl />
        </SettingGroup>
      </SettingsBlock>
      <SettingsBlock title={m.settings_sync()}>
        <SyncControl />
      </SettingsBlock>
      <VerifiedRisks onOpenChange={setRisksOpen} open={risksOpen} />
    </>
  );
};
