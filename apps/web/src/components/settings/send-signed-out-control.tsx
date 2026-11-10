// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// «Enviar los programados aunque cierre sesión»: the default for new
// scheduled messages, on this device (mail/send-signed-out.ts). Only shown
// when the server (the bridge) can do it.

import { Switch } from "@nebulamail/ui";

import { useLocalFlag } from "../../lib/use-local-flag";
import { SEND_SIGNED_OUT_FLAG } from "../../mail/send-signed-out";
import { useSendSignedOutSupport } from "../../mail/use-schedule";
import { m } from "../../paraglide/messages.js";
import { SettingGroup } from "./section";

export const SendSignedOutControl = () => {
  const supported = useSendSignedOutSupport();
  const [enabled, setEnabled] = useLocalFlag(SEND_SIGNED_OUT_FLAG, false);
  if (!supported) {
    return null;
  }
  return (
    <SettingGroup>
      <Switch
        checked={enabled}
        description={m.settings_send_signed_out_hint()}
        label={m.settings_send_signed_out()}
        onCheckedChange={setEnabled}
      />
    </SettingGroup>
  );
};
