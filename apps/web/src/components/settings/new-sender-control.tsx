// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// «Avisar de remitentes nuevos»: the reader's «Primera vez que te escribe»
// hint (mail/sender-new.ts), on by default. Only in Settings › General,
// not in the quick settings popover.

import { Switch } from "@nebulamail/ui";

import { useNewSenderHint } from "../../mail/use-new-sender-hint";
import { m } from "../../paraglide/messages.js";
import { SettingGroup } from "./section";

export const NewSenderControl = () => {
  const [enabled, setEnabled] = useNewSenderHint();
  return (
    <SettingGroup>
      <Switch
        checked={enabled}
        description={m.settings_new_sender_hint()}
        label={m.settings_new_sender()}
        onCheckedChange={setEnabled}
      />
    </SettingGroup>
  );
};
