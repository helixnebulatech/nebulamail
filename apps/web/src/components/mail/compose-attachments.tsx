// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Attachments in a compose window: the uploaded ones (remove) and the ones
// still uploading (progress, stop).

import { cleanDisplayText } from "@nebulamail/mail-render";
import { IconButton, ProgressBar } from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { FileText, TriangleAlert, X } from "lucide-react";

import { formatBytes } from "../../lib/format";
import type { DraftAttachment } from "../../mail/attachments";
import type { Upload } from "../../mail/use-uploads";
import { m } from "../../paraglide/messages.js";
import { getLocale } from "../../paraglide/runtime.js";
import { shared } from "./styles";

const styles = stylex.create({
  card: {
    alignItems: "center",
    backgroundColor: colors.bg,
    borderColor: colors.line,
    borderRadius: radius.md,
    borderStyle: "solid",
    borderWidth: "1px",
    columnGap: "8px",
    display: "flex",
    maxWidth: "240px",
    minWidth: "160px",
    paddingBlock: "5px",
    paddingInlineEnd: "4px",
    paddingInlineStart: "8px",
  },
  failed: { borderColor: colors.danger },
  icon: { color: colors.fg3, flexShrink: 0 },
  iconFailed: { color: colors.danger, flexShrink: 0 },
  list: {
    display: "flex",
    flexWrap: "wrap",
    gap: "6px",
    paddingBottom: "4px",
    paddingInline: "16px",
  },
  meta: { color: colors.fg3, display: "block", fontSize: "11.5px" },
  metaFailed: { color: colors.danger },
  name: {
    display: "block",
    fontSize: "12.5px",
    fontWeight: font.weightSemibold,
  },
  text: {
    display: "flex",
    flexDirection: "column",
    flexGrow: 1,
    minWidth: 0,
    rowGap: "2px",
  },
});

export interface ComposeAttachmentsProps {
  readonly attachments: readonly DraftAttachment[];
  readonly uploads: readonly Upload[];
  readonly onRemove: (attachment: DraftAttachment) => void;
  readonly onCancel: (upload: Upload) => void;
}

export const ComposeAttachments = ({
  attachments,
  onCancel,
  onRemove,
  uploads,
}: ComposeAttachmentsProps) => {
  if (attachments.length === 0 && uploads.length === 0) {
    return null;
  }
  const locale = getLocale();
  return (
    <ul
      aria-label={m.compose_attachments()}
      {...stylex.props(shared.resetList, styles.list)}
    >
      {attachments.map((attachment) => {
        const name = cleanDisplayText(attachment.name);
        return (
          <li key={attachment.blobId} {...stylex.props(styles.card)}>
            <FileText aria-hidden size={16} {...stylex.props(styles.icon)} />
            <span {...stylex.props(styles.text)}>
              <span {...stylex.props(styles.name, shared.ellipsis)}>
                <bdi>{name}</bdi>
              </span>
              <span {...stylex.props(styles.meta)}>
                {formatBytes(attachment.size, locale)}
              </span>
            </span>
            <IconButton
              icon={X}
              label={m.compose_remove_attachment({ name })}
              onClick={() => {
                onRemove(attachment);
              }}
              size="xs"
            />
          </li>
        );
      })}
      {uploads.map((upload) => {
        const name = cleanDisplayText(upload.name);
        return (
          <li
            key={upload.key}
            {...stylex.props(styles.card, upload.failed ? styles.failed : null)}
          >
            {upload.failed ? (
              <TriangleAlert
                aria-hidden
                size={16}
                {...stylex.props(styles.iconFailed)}
              />
            ) : (
              <FileText aria-hidden size={16} {...stylex.props(styles.icon)} />
            )}
            <span {...stylex.props(styles.text)}>
              <span {...stylex.props(styles.name, shared.ellipsis)}>
                <bdi>{name}</bdi>
              </span>
              {upload.failed ? (
                <span
                  role="alert"
                  {...stylex.props(styles.meta, styles.metaFailed)}
                >
                  {m.compose_upload_failed()}
                </span>
              ) : (
                <ProgressBar
                  aria-label={m.compose_uploading({ name })}
                  busyText={m.compose_uploading_busy()}
                  max={1}
                  value={upload.progress}
                />
              )}
            </span>
            <IconButton
              icon={X}
              label={
                upload.failed
                  ? m.compose_remove_attachment({ name })
                  : m.compose_cancel_upload({ name })
              }
              onClick={() => {
                onCancel(upload);
              }}
              size="xs"
            />
          </li>
        );
      })}
    </ul>
  );
};
