// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Attachments of the open email: name (bidi-safe), size, a warning for
// dangerous types, download with the token as a blob.

import type { AttachmentWarning } from "@nebulamail/mail-render";
import { analyzeAttachment } from "@nebulamail/mail-render";
import { Tooltip, useToast } from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { Download, TriangleAlert } from "lucide-react";
import { useState } from "react";

import { formatBytes } from "../../lib/format";
import type { Attachment } from "../../mail/api";
import { attachmentFileName, downloadAttachment } from "../../mail/download";
import { m } from "../../paraglide/messages.js";
import { getLocale } from "../../paraglide/runtime.js";
import { VcardImports } from "../contacts/add-to-contacts";
import { shared } from "./styles";

const styles = stylex.create({
  badge: {
    alignItems: "center",
    backgroundColor: colors.infoSoft,
    borderRadius: radius.md,
    color: colors.info,
    display: "grid",
    flexShrink: 0,
    fontSize: "10px",
    fontWeight: 750,
    height: "32px",
    justifyContent: "center",
    textTransform: "uppercase",
    width: "32px",
  },
  badgeDanger: { backgroundColor: colors.dangerSoft, color: colors.danger },
  card: {
    alignItems: "center",
    backgroundColor: { ":hover": colors.hover, default: colors.bg },
    borderColor: colors.line,
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: "1px",
    color: colors.fg,
    columnGap: "10px",
    display: "flex",
    maxWidth: "280px",
    paddingBlock: "8px",
    paddingInline: "10px",
    width: "100%",
  },
  cardDanger: { borderColor: colors.danger },
  download: { color: colors.fg3, flexShrink: 0, marginInlineStart: "6px" },
  heading: {
    color: colors.fg3,
    fontSize: font.sizeSm,
    fontWeight: font.weightSemibold,
    margin: "16px 0 8px",
  },
  list: { display: "flex", flexWrap: "wrap", gap: "8px" },
  meta: { color: colors.fg3, display: "block", fontSize: "11.5px" },
  name: {
    display: "block",
    fontSize: "12.5px",
    fontWeight: font.weightSemibold,
  },
  text: { flexGrow: 1, minWidth: 0 },
  warning: {
    alignItems: "center",
    color: colors.danger,
    columnGap: "4px",
    display: "flex",
    fontSize: "11.5px",
  },
});

const WARNING_TEXT: Partial<
  Record<AttachmentWarning, (extension: string) => string>
> = {
  "bidi-in-name": () => m.attachment_warn_bidi(),
  "dangerous-extension": () => m.attachment_warn_dangerous(),
  "double-extension": (extension) => m.attachment_warn_double({ extension }),
  "mime-mismatch": () => m.attachment_warn_mime(),
};

const AttachmentCard = ({
  attachment,
}: {
  readonly attachment: Attachment;
}) => {
  const toast = useToast();
  const [busy, setBusy] = useState(false);
  const analysis = analyzeAttachment({
    name: attachment.name,
    type: attachment.type,
  });
  const name =
    analysis.displayName === "" ? m.attachment_unnamed() : analysis.displayName;
  const warnings = analysis.warnings.flatMap((warning) => {
    const text = WARNING_TEXT[warning];
    return text === undefined ? [] : [text(analysis.extension)];
  });
  const danger = warnings.length > 0;
  const size = formatBytes(attachment.size, getLocale());

  const download = async (): Promise<void> => {
    setBusy(true);
    try {
      await downloadAttachment(
        attachment,
        attachmentFileName(analysis, m.attachment_unnamed())
      );
    } catch {
      toast.show({
        priority: "high",
        title: m.toast_download_failed({ name }),
        tone: "danger",
      });
    }
    setBusy(false);
  };

  const card = (
    <button
      aria-busy={busy || undefined}
      aria-label={`${m.attachment_download({ name })} (${size})${danger ? `. ${warnings.join(" ")}` : ""}`}
      onClick={() => {
        void download();
      }}
      type="button"
      {...stylex.props(
        shared.resetButton,
        styles.card,
        danger ? styles.cardDanger : null,
        shared.focusRing
      )}
    >
      <span
        aria-hidden
        {...stylex.props(styles.badge, danger ? styles.badgeDanger : null)}
      >
        {danger ? (
          <TriangleAlert size={16} />
        ) : (
          analysis.extension.slice(0, 4) || "—"
        )}
      </span>
      <span {...stylex.props(styles.text)}>
        <bdi {...stylex.props(styles.name, shared.ellipsis)}>{name}</bdi>
        {danger ? (
          <span {...stylex.props(styles.warning)}>
            <span {...stylex.props(shared.ellipsis)}>{warnings[0]}</span>
          </span>
        ) : (
          <span {...stylex.props(styles.meta)}>{size}</span>
        )}
      </span>
      <Download aria-hidden size={15} {...stylex.props(styles.download)} />
    </button>
  );

  return (
    <li>
      {danger ? <Tooltip content={warnings.join(" ")}>{card}</Tooltip> : card}
    </li>
  );
};

const isCalendar = (attachment: Attachment): boolean =>
  attachment.type.toLowerCase() === "text/calendar" ||
  attachment.type.toLowerCase() === "application/ics" ||
  attachment.name.toLowerCase().endsWith(".ics");

/**
 * The files to list. An invitation usually comes twice: a nameless
 * text/calendar part (read by the invitation card) and an .ics file. The
 * nameless one is left out when the named file is there; alone, it stays
 * so it can still be downloaded.
 */
const shownAttachments = (
  attachments: readonly Attachment[]
): readonly Attachment[] => {
  const namedCalendar = attachments.some(
    (attachment) => isCalendar(attachment) && attachment.name !== ""
  );
  return namedCalendar
    ? attachments.filter(
        (attachment) => !(isCalendar(attachment) && attachment.name === "")
      )
    : attachments;
};

export const AttachmentList = ({
  attachments: all,
}: {
  readonly attachments: readonly Attachment[];
}) => {
  const attachments = shownAttachments(all);
  return attachments.length === 0 ? null : (
    <section aria-label={m.attachments_label({ count: attachments.length })}>
      <h3 {...stylex.props(styles.heading)}>
        {m.attachments_label({ count: attachments.length })}
      </h3>
      <ul {...stylex.props(shared.resetList, styles.list)}>
        {attachments.map((attachment) => (
          <AttachmentCard attachment={attachment} key={attachment.blobId} />
        ))}
      </ul>
      <VcardImports attachments={attachments} />
    </section>
  );
};
