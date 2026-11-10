// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// "X asks for a read receipt" with Send / Ignore, in an open email whose
// sender asked for one (mail/read-receipts.ts). Either answer sets
// `$mdnsent`, so it isn't asked again here or in another client. With
// "Always" the receipt goes out by itself and nothing is shown, unless it
// isn't safe to (then this asks).

import { cleanDisplayText } from "@nebulamail/mail-render";
import { Button } from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { MailCheck } from "lucide-react";

import type { EmailDetail } from "../../mail/api";
import { findRole, useAccountMailboxes } from "../../mail/hooks";
import { sameAddress } from "../../mail/recipients";
import { useReadReceipt } from "../../mail/use-read-receipts";
import { m } from "../../paraglide/messages.js";

const styles = stylex.create({
  banner: {
    alignItems: "center",
    backgroundColor: colors.infoSoft,
    borderRadius: radius.md,
    color: colors.fg,
    columnGap: "10px",
    display: "flex",
    flexWrap: "wrap",
    fontSize: font.sizeSm,
    marginTop: "12px",
    paddingBlock: "6px",
    paddingInline: "10px",
    rowGap: "6px",
  },
  icon: { color: colors.info, flexShrink: 0 },
  text: { flexGrow: 1, minWidth: 0, overflowWrap: "anywhere" },
  hint: { color: colors.fg3, display: "block" },
  error: { color: colors.danger, display: "block" },
  buttons: { columnGap: "6px", display: "flex" },
});

export const ReadReceiptBanner = ({
  email,
  ours,
}: {
  readonly email: EmailDetail;
  /** We wrote it (or it is a draft): never asks. */
  readonly ours: boolean;
}) => {
  const mailboxes = useAccountMailboxes();
  const inRole = (role: "junk" | "sent" | "drafts"): boolean => {
    const box = findRole(mailboxes.data, role);
    return box !== undefined && email.mailboxIds.includes(box.id);
  };
  const receipt = useReadReceipt(email, {
    // Until the folders are known, nothing happens (it may be in Junk).
    known: mailboxes.data !== undefined,
    inJunk: inRole("junk"),
    ours: ours || inRole("sent") || inRole("drafts"),
  });
  const request = email.readReceipt;
  const handleSend = receipt.send;
  const handleIgnore = receipt.ignore;
  if (receipt.action !== "ask" || request === null) {
    return null;
  }
  const address = request.to.map((item) => cleanDisplayText(item)).join(", ");
  // RFC 8098 §2.1: say so when it wouldn't go back to the sender.
  const elsewhere = !request.to.every((item) =>
    email.from.some((person) => sameAddress(person.email, item))
  );
  return (
    <section
      aria-label={m.settings_read_receipts()}
      data-testid="read-receipt"
      {...stylex.props(styles.banner)}
    >
      <MailCheck aria-hidden size={16} {...stylex.props(styles.icon)} />
      <span {...stylex.props(styles.text)}>
        <bdi>{m.read_receipt_ask({ address })}</bdi>
        {elsewhere ? (
          <span {...stylex.props(styles.hint)}>
            {m.read_receipt_other_address()}
          </span>
        ) : null}
        {receipt.failed ? (
          <span role="alert" {...stylex.props(styles.error)}>
            {m.read_receipt_error()}
          </span>
        ) : null}
      </span>
      <span {...stylex.props(styles.buttons)}>
        <Button
          loading={receipt.busy}
          onClick={handleSend}
          size="sm"
          variant="secondary"
        >
          {m.read_receipt_send()}
        </Button>
        <Button
          disabled={receipt.busy}
          onClick={handleIgnore}
          size="sm"
          variant="ghost"
        >
          {m.read_receipt_ignore()}
        </Button>
      </span>
    </section>
  );
};
