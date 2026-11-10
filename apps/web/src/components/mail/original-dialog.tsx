// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// "View original": the raw message, headers included,
// as plain text in a <pre>. React sets it as a text node, so nothing in it
// is ever parsed as HTML. Big messages show the first 512 KB with a note.

import { Button, Dialog, Spinner } from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useQuery } from "@tanstack/react-query";
import { Download } from "lucide-react";

import type { EmailDetail } from "../../mail/api";
import { MAX_ORIGINAL_BYTES, readOriginal } from "../../mail/download";
import { m } from "../../paraglide/messages.js";

const styles = stylex.create({
  center: { display: "flex", justifyContent: "center", padding: "24px" },
  note: {
    backgroundColor: colors.warnSoft,
    borderRadius: radius.md,
    color: colors.fg,
    fontSize: font.sizeSm,
    marginBlock: "0 8px",
    paddingBlock: "6px",
    paddingInline: "10px",
  },
  pre: {
    backgroundColor: colors.hover,
    borderRadius: radius.md,
    color: colors.fg,
    fontFamily: font.mono,
    fontSize: "12px",
    lineHeight: 1.45,
    margin: 0,
    maxHeight: "60dvh",
    overflow: "auto",
    overflowWrap: "anywhere",
    padding: "10px",
    whiteSpace: "pre-wrap",
  },
});

export const OriginalDialog = ({
  email,
  onDownload,
  onOpenChange,
}: {
  readonly email: EmailDetail;
  readonly onDownload: () => void;
  readonly onOpenChange: (open: boolean) => void;
}) => {
  const original = useQuery({
    // Raw mail isn't kept around after the dialog closes.
    gcTime: 0,
    queryFn: async () => await readOriginal(email),
    queryKey: ["mail", "original", email.id],
  });

  let body;
  if (original.isPending) {
    body = (
      <div {...stylex.props(styles.center)}>
        <Spinner label={m.reader_loading()} />
      </div>
    );
  } else if (original.isError) {
    body = <p {...stylex.props(styles.note)}>{m.original_error()}</p>;
  } else {
    body = (
      <>
        {original.data.truncated ? (
          <p role="note" {...stylex.props(styles.note)}>
            {m.original_truncated({
              size: `${Math.round(MAX_ORIGINAL_BYTES / 1024)} KB`,
            })}
          </p>
        ) : null}
        <pre {...stylex.props(styles.pre)}>{original.data.text}</pre>
      </>
    );
  }

  return (
    <Dialog.Root open onOpenChange={onOpenChange}>
      <Dialog.Content closeLabel={m.close()} size="lg">
        <Dialog.Title>{m.action_view_original()}</Dialog.Title>
        <Dialog.Body>{body}</Dialog.Body>
        <Dialog.Footer>
          <Button icon={Download} onClick={onDownload}>
            {m.action_download_eml()}
          </Button>
        </Dialog.Footer>
      </Dialog.Content>
    </Dialog.Root>
  );
};
