// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The read receipt setting and what an open email does about it
// (read-receipts.ts). The setting lives on the device and is synced
//"ask" by default.

import { cleanDisplayText } from "@nebulamail/mail-render";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";

import { useLocalChoice } from "../lib/use-local-flag";
import { m } from "../paraglide/messages.js";
import { recordChange } from "../prefs-sync/sync";
import { useMailAccount } from "./account-scope";
import type { EmailDetail } from "./api";
import { getClient } from "./client";
import { mailKeys } from "./queries";
import type { ReadReceiptMode, ReceiptAction } from "./read-receipts";
import {
  DEFAULT_READ_RECEIPTS,
  READ_RECEIPTS_KEY,
  READ_RECEIPT_MODES,
  ignoreReadReceipt,
  receiptAction,
  sendReadReceipt,
  supportsReceipts,
} from "./read-receipts";

export const useReadReceiptMode = (): readonly [
  ReadReceiptMode,
  (mode: ReadReceiptMode) => void,
] => {
  const [mode, setMode] = useLocalChoice(
    READ_RECEIPTS_KEY,
    READ_RECEIPT_MODES,
    DEFAULT_READ_RECEIPTS
  );
  return [
    mode,
    (next) => {
      setMode(next);
      recordChange("readReceipts", next);
    },
  ] as const;
};

/** The session has `urn:ietf:params:jmap:mdn` (false while unknown). */
export const useReceiptsSupported = (): boolean => {
  const { data } = useQuery({
    queryFn: async () => supportsReceipts(await getClient()),
    queryKey: [...mailKeys.all, "support", "mdn"],
    staleTime: Number.POSITIVE_INFINITY,
  });
  return data === true;
};

/** Emails whose automatic receipt already went (or is going) out. */
const autoSent = new Set<string>();

export interface ReadReceiptState {
  readonly action: ReceiptAction;
  readonly busy: boolean;
  readonly failed: boolean;
  readonly send: () => void;
  readonly ignore: () => void;
}

const receiptText = (email: EmailDetail) => {
  const subject = cleanDisplayText(email.subject);
  return {
    body: m.read_receipt_body({ subject: subject || m.row_no_subject() }),
    subject: m.read_receipt_subject({ subject }),
  };
};

/**
 * What to do about this email's request. With "always" (and safe, see
 * read-receipts.ts) the receipt goes out once when the email is shown.
 */
export const useReadReceipt = (
  email: EmailDetail,
  facts: {
    /** The folders are loaded (otherwise `inJunk` isn't known yet). */
    readonly known: boolean;
    readonly inJunk: boolean;
    readonly ours: boolean;
  }
): ReadReceiptState => {
  const [mode] = useReadReceiptMode();
  const supported = useReceiptsSupported();
  const account = useMailAccount();
  const client = useQueryClient();
  const action = facts.known
    ? receiptAction(email.readReceipt, {
        inJunk: facts.inJunk,
        mode,
        ours: facts.ours,
        ownAccount: account === null,
        supported,
      })
    : "none";

  const answered = (): void => {
    client.setQueriesData<EmailDetail | null>(
      { queryKey: mailKeys.emailsOf(null) },
      (cached) =>
        cached === undefined ||
        cached === null ||
        cached.id !== email.id ||
        cached.readReceipt === null
          ? cached
          : {
              ...cached,
              readReceipt: { ...cached.readReceipt, answered: true },
            }
    );
  };

  const mutation = useMutation({
    mutationFn: async (choice: "send" | "auto" | "ignore") => {
      if (choice === "ignore") {
        await ignoreReadReceipt(email.id);
        return;
      }
      await sendReadReceipt(
        email.id,
        [...email.to, ...email.cc].map((person) => person.email),
        receiptText(email),
        choice === "auto",
        account
      );
    },
    onSuccess: answered,
  });
  const { mutate } = mutation;

  useEffect(() => {
    if (action === "send" && !autoSent.has(email.id)) {
      autoSent.add(email.id);
      mutate("auto");
    }
  }, [action, email.id, mutate]);

  return {
    action,
    busy: mutation.isPending,
    failed: mutation.isError,
    ignore: () => {
      mutate("ignore");
    },
    send: () => {
      mutate("send");
    },
  };
};
