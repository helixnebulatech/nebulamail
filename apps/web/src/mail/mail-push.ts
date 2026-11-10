// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// New-mail notifications with every tab closed: Web Push
// (lib/web-push.ts) with the JMAP type "EmailDelivery" (RFC 8621 §1.5),
// whose state only changes when mail is delivered, not on every flag or
// move, so the worker wakes up for new mail only. The worker (pwa/sw.ts)
// then asks `Email/changes` from the last state stored on this device
// (push-store.ts) and announces what notify.ts would: unread inbox mail,
// not junk, from the last 15 minutes; sender and subject, never the body.
//
// The tabs keep the worker in step: turning the setting on or off, and on
// each start (the stored state, the "on" flag and the subscription).

import type { JmapClient, State } from "@nebulamail/jmap-client";

import {
  setPushTypes,
  wantedPushTypes,
  webPushAvailable,
} from "../lib/web-push";
import { getClient } from "./client";
import { prepareEmailPush } from "./email-push";
import { emailState } from "./notify";
import { pushAccountKey, seedMailState, setMailPushOn } from "./push-store";

export const MAIL_PUSH_TYPE = "EmailDelivery";

/** This device's key for the primary mail account of the session. */
export const mailPushAccount = (client: JmapClient): string =>
  pushAccountKey(
    client.session.apiUrl.origin,
    client.session.username,
    client.primaryAccountId()
  );

const startFrom = async (client: JmapClient, state: State): Promise<void> => {
  await setMailPushOn(true);
  await seedMailState(mailPushAccount(client), state);
  // Where mail inside the push goes (Stalwart 1.0), kept fresh each start.
  await prepareEmailPush(client).catch(() => null);
};

/**
 * The setting was turned on (permission granted). Resolves to whether new
 * mail now reaches this device with every tab closed.
 */
export const turnOnMailPush = async (): Promise<boolean> => {
  const client = await getClient();
  await startFrom(client, await emailState(client));
  return (await webPushAvailable())
    ? await setPushTypes({ add: [MAIL_PUSH_TYPE] })
    : false;
};

/** The setting was turned off: the worker stops announcing at once. */
export const turnOffMailPush = async (): Promise<void> => {
  await setMailPushOn(false).catch(() => null);
  if (wantedPushTypes().includes(MAIL_PUSH_TYPE)) {
    await setPushTypes({ remove: [MAIL_PUSH_TYPE] });
  }
};

/**
 * On start, with the account's current state: the worker follows the
 * setting (also if the browser's permission was taken back).
 */
export const keepMailPushInStep = async (
  client: JmapClient,
  state: State,
  wanted: boolean
): Promise<void> => {
  if (!wanted) {
    await turnOffMailPush();
    return;
  }
  await startFrom(client, state);
  if (
    !wantedPushTypes().includes(MAIL_PUSH_TYPE) &&
    (await webPushAvailable())
  ) {
    await setPushTypes({ add: [MAIL_PUSH_TYPE] });
  }
};
