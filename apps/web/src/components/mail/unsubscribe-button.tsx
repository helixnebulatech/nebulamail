// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// "Unsubscribe" next to the subject, best option first:
// 1. One-click (RFC 8058, only with an aligned DKIM pass; mail/unsubscribe.ts):
//    after confirming, the BFF POSTs for us; the user never visits the site.
// 2. mailto: opens the composer with the address (to/cc/bcc/subject/body).
// 3. An https page: after a warning, opens in a new tab (noopener, noreferrer).

import { AlertDialog, Button, useToast } from "@nebulamail/ui";
import { useState } from "react";

import type { EmailDetail } from "../../mail/api";
import { unsubscribeOneClick } from "../../mail/api";
import {
  unsubscribePageFor,
  useOneClickUnsubscribe,
} from "../../mail/one-click-unsubscribe";
import { m } from "../../paraglide/messages.js";
import { useComposer } from "./compose-context";
import { composeFromMailto, openExternal } from "./link-dialog";

type Confirm = "one-click" | "web" | null;

const hostOf = (url: string): string => URL.parse(url)?.hostname ?? url;

export const UnsubscribeButton = ({
  email,
}: {
  readonly email: EmailDetail;
}) => {
  const composer = useComposer();
  const toast = useToast();
  const [confirm, setConfirm] = useState<Confirm>(null);
  const [state, setState] = useState<"idle" | "sending" | "done">("idle");
  const [staticOneClick] = useOneClickUnsubscribe();
  const { mailto, web } = email.unsubscribe;
  // Static build: only if the user turned it on, and through the zip's
  // page (it asks again and sends the POST itself).
  let { oneClick } = email.unsubscribe;
  let page: string | null = null;
  if (import.meta.env.NEBULA_STATIC) {
    page =
      staticOneClick && oneClick !== null ? unsubscribePageFor(oneClick) : null;
    oneClick = page === null ? null : oneClick;
  }

  if (oneClick === null && mailto === null && web === null) {
    return null;
  }

  const sendOneClick = async (): Promise<void> => {
    setState("sending");
    const result = await unsubscribeOneClick(email.id, email.accountId).catch(
      () => "failed" as const
    );
    if (result === "done") {
      setState("done");
      toast.show({ title: m.toast_unsubscribed() });
      return;
    }
    setState("idle");
    toast.show({
      priority: "high",
      title:
        result === "redirected"
          ? m.toast_unsubscribe_redirected()
          : m.toast_unsubscribe_failed(),
      tone: "danger",
    });
  };

  const start = (): void => {
    if (page !== null) {
      // The page shows the domain and has the only button that sends.
      openExternal(page);
      return;
    }
    if (oneClick !== null) {
      setConfirm("one-click");
      return;
    }
    const init = mailto === null ? null : composeFromMailto(mailto);
    if (init !== null) {
      composer.open(init);
    } else if (web !== null) {
      setConfirm("web");
    }
  };

  let dialog = null;
  if (confirm === "one-click" && oneClick !== null) {
    dialog = (
      <AlertDialog
        cancelLabel={m.cancel()}
        confirmLabel={m.unsubscribe()}
        description={m.unsubscribe_one_click_body({ host: hostOf(oneClick) })}
        onConfirm={() => {
          void sendOneClick();
        }}
        onOpenChange={(open) => {
          if (!open) {
            setConfirm(null);
          }
        }}
        open
        title={m.unsubscribe_title()}
      />
    );
  } else if (confirm === "web" && web !== null) {
    dialog = (
      <AlertDialog
        cancelLabel={m.cancel()}
        confirmLabel={m.unsubscribe_open_site()}
        description={m.unsubscribe_web_body({ host: hostOf(web) })}
        onConfirm={() => {
          openExternal(web);
        }}
        onOpenChange={(open) => {
          if (!open) {
            setConfirm(null);
          }
        }}
        open
        title={m.unsubscribe_title()}
      />
    );
  }

  return (
    <>
      <Button
        disabled={state !== "idle"}
        onClick={start}
        size="sm"
        variant="ghost"
      >
        {state === "done" ? m.unsubscribed() : m.unsubscribe()}
      </Button>
      {dialog}
    </>
  );
};
