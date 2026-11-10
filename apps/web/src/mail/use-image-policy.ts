// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The remote images settings of the build with our server, in React
// (image-policy.ts): the admin's part comes with the grant, the user's
// choices are synced (prefs-sync), and trusted senders are the user's list
// plus their contacts.

import { useQuery } from "@tanstack/react-query";

import { getGrant } from "../lib/auth";
import { useSyncedValue } from "../prefs-sync/use-synced-value";
import type { EmailDetail } from "./api";
import { contactsKey, listContacts } from "./contacts";
import { findRole, useAccountMailboxes } from "./hooks";
import type {
  AdminImagePolicy,
  ImageDecision,
  ImageLoadMode,
  ImagePolicy,
  ImageRoute,
} from "./image-policy";
import {
  decideRemoteImages,
  isTrustedSender,
  resolveImagePolicy,
  withTrustedSender,
} from "./image-policy";

export interface ImageSettings {
  /** Null until the admin's part is known (nothing loads meanwhile). */
  readonly policy: ImagePolicy | null;
  readonly setLoad: (mode: ImageLoadMode) => void;
  readonly setRoute: (route: ImageRoute) => void;
  readonly trustedSenders: readonly string[];
  readonly setTrusted: (address: string, on: boolean) => void;
  readonly clearTrusted: () => void;
}

const useAdminPolicy = (): AdminImagePolicy | null => {
  const { data } = useQuery({
    queryFn: async () => {
      const grant = await getGrant();
      // Our server always sends it; an older one doesn't: its defaults.
      return grant.imagePolicy ?? null;
    },
    queryKey: ["config", "image-policy"],
    staleTime: Number.POSITIVE_INFINITY,
  });
  return data ?? null;
};

export const useImageSettings = (): ImageSettings => {
  const admin = useAdminPolicy();
  const [load, setLoad] = useSyncedValue("remoteImagesLoad");
  const [route, setRoute] = useSyncedValue("remoteImagesVia");
  const [trusted, setTrustedList] = useSyncedValue("imageTrustedSenders");
  const list = trusted ?? [];
  return {
    clearTrusted: () => {
      setTrustedList([]);
    },
    policy: admin === null ? null : resolveImagePolicy(admin, { load, route }),
    setLoad,
    setRoute,
    setTrusted: (address, on) => {
      setTrustedList(withTrustedSender(list, address, on));
    },
    trustedSenders: list,
  };
};

export interface EmailImages {
  readonly settings: ImageSettings;
  readonly decision: ImageDecision;
  /** Loaded at once because the sender is trusted. */
  readonly senderTrusted: boolean;
}

/** What to do with one email's remote images. */
export const useEmailImages = (email: EmailDetail): EmailImages => {
  const settings = useImageSettings();
  const { policy } = settings;
  const wantsTrust = policy?.load === "trusted-senders";
  const mailboxes = useAccountMailboxes();
  // The same query as the recipient suggestions; only when it matters.
  const contacts = useQuery({
    enabled: wantsTrust,
    queryFn: listContacts,
    queryKey: contactsKey,
    staleTime: 5 * 60 * 1000,
  });
  const junk = findRole(mailboxes.data, "junk");
  const senderTrusted =
    wantsTrust &&
    // Until the folders are known, not trusted (it may be in Junk). The
    // contacts may come later: asking first, then loading, is fine.
    mailboxes.data !== undefined &&
    isTrustedSender(
      {
        from: email.from.map((person) => person.email),
        outsideJunk: junk === undefined || !email.mailboxIds.includes(junk.id),
        verdict: email.senderVerdict,
      },
      {
        contacts: (contacts.data ?? []).map((contact) => contact.email),
        list: settings.trustedSenders,
      }
    );
  return {
    decision:
      policy === null ? "never" : decideRemoteImages(policy, { senderTrusted }),
    senderTrusted,
    settings,
  };
};
