// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Import contacts from a .vcf (an email attachment or a file): the server
// reads it (`ContactCard/parse`), we show what it contains, and nothing is
// saved until the user confirms (never automatic).

import type { AddressBookId } from "@nebulamail/jmap-client";
import {
  Avatar,
  Button,
  Checkbox,
  Dialog,
  Spinner,
  useToast,
} from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useQuery } from "@tanstack/react-query";
import { useState } from "react";

import type { ImportPreview, VcfSource } from "../../contacts/api";
import { VcfTooBigError, previewVcf } from "../../contacts/api";
import { useAddressBooks, useImportContacts } from "../../contacts/hooks";
import { m } from "../../paraglide/messages.js";
import { shared } from "../mail/styles";

const styles = stylex.create({
  center: {
    alignItems: "center",
    display: "flex",
    justifyContent: "center",
    padding: "24px",
  },
  footer: {
    columnGap: "8px",
    display: "flex",
    justifyContent: "flex-end",
    marginTop: "16px",
  },
  list: {
    borderColor: colors.line,
    borderRadius: radius.md,
    borderStyle: "solid",
    borderWidth: "1px",
    maxHeight: "50dvh",
    overflowY: "auto",
  },
  muted: { color: colors.fg3, fontSize: font.sizeSm, marginBlock: "8px" },
  row: {
    alignItems: "center",
    borderBottomColor: colors.line,
    borderBottomStyle: "solid",
    borderBottomWidth: { ":last-child": 0, default: "1px" },
    columnGap: "10px",
    display: "flex",
    paddingBlock: "8px",
    paddingInline: "10px",
  },
  rowText: { display: "flex", flexDirection: "column", minWidth: 0 },
  sub: { color: colors.fg3, fontSize: "12.5px" },
});

const PreviewList = ({
  onImported,
  preview,
}: {
  readonly preview: ImportPreview;
  readonly onImported: () => void;
}) => {
  const toast = useToast();
  const books = useAddressBooks();
  const importer = useImportContacts();
  const [unchecked, setUnchecked] = useState<ReadonlySet<number>>(new Set());
  // Into our own default (first) book.
  const book: AddressBookId | undefined = (books.data ?? []).find(
    (entry) => entry.mayWrite && entry.accountId === null
  )?.id;
  const chosen = preview.candidates.filter(
    (candidate) => !unchecked.has(candidate.index)
  );

  const run = async (): Promise<void> => {
    if (book === undefined) {
      return;
    }
    try {
      const result = await importer.mutateAsync({
        book: { accountId: null, id: book },
        cards: chosen.map((candidate) => candidate.card),
      });
      toast.show({
        title: m.contacts_imported({ count: result.created }),
        ...(result.failed > 0
          ? {
              description: m.contacts_import_some_failed({
                count: result.failed,
              }),
              tone: "danger" as const,
            }
          : {}),
      });
      onImported();
    } catch {
      toast.show({
        priority: "high",
        title: m.contacts_import_failed(),
        tone: "danger",
      });
    }
  };

  if (preview.candidates.length === 0) {
    return <p>{m.contacts_import_nothing()}</p>;
  }
  return (
    <>
      <p {...stylex.props(styles.muted)}>{m.contacts_import_review()}</p>
      <ul {...stylex.props(shared.resetList, styles.list)}>
        {preview.candidates.map((candidate) => {
          const name =
            candidate.name === "" ? m.contacts_no_name() : candidate.name;
          return (
            <li key={candidate.index} {...stylex.props(styles.row)}>
              <Checkbox
                aria-label={m.contacts_import_pick({ name })}
                checked={!unchecked.has(candidate.index)}
                onCheckedChange={(checked) => {
                  setUnchecked((current) => {
                    const next = new Set(current);
                    if (checked) {
                      next.delete(candidate.index);
                    } else {
                      next.add(candidate.index);
                    }
                    return next;
                  });
                }}
              />
              <Avatar name={name} size={30} />
              <span {...stylex.props(styles.rowText)}>
                <bdi {...stylex.props(shared.ellipsis)}>{name}</bdi>
                <bdi {...stylex.props(styles.sub, shared.ellipsis)}>
                  {[candidate.emails[0] ?? "", candidate.organization]
                    .filter((text) => text !== "")
                    .join(" · ")}
                </bdi>
              </span>
            </li>
          );
        })}
      </ul>
      {preview.unreadable > 0 || preview.omitted > 0 ? (
        <p {...stylex.props(styles.muted)}>
          {[
            preview.unreadable > 0
              ? m.contacts_import_unreadable({ count: preview.unreadable })
              : "",
            preview.omitted > 0
              ? m.contacts_import_omitted({ count: preview.omitted })
              : "",
          ]
            .filter((text) => text !== "")
            .join(" ")}
        </p>
      ) : null}
      <div {...stylex.props(styles.footer)}>
        <Button
          disabled={chosen.length === 0 || book === undefined}
          loading={importer.isPending}
          onClick={() => {
            void run();
          }}
          variant="primary"
        >
          {m.contacts_import_confirm({ count: chosen.length })}
        </Button>
      </div>
    </>
  );
};

const sourceKey = (source: VcfSource): readonly unknown[] =>
  source.kind === "file"
    ? ["file", source.file.name, source.file.size, source.file.lastModified]
    : ["attachment", source.accountId ?? "", source.blobId];

const ImportBody = ({
  onDone,
  source,
}: {
  readonly source: VcfSource;
  readonly onDone: () => void;
}) => {
  const preview = useQuery({
    gcTime: 0,
    queryFn: async () => await previewVcf(source),
    queryKey: ["contacts", "import", ...sourceKey(source)],
    retry: false,
    staleTime: Infinity,
  });
  if (preview.status === "pending") {
    return (
      <div {...stylex.props(styles.center)}>
        <Spinner label={m.contacts_import_reading()} />
      </div>
    );
  }
  if (preview.status === "error") {
    return (
      <p role="alert">
        {preview.error instanceof VcfTooBigError
          ? m.contacts_import_too_big()
          : m.contacts_import_failed()}
      </p>
    );
  }
  return <PreviewList onImported={onDone} preview={preview.data} />;
};

export interface ImportDialogProps {
  readonly source: VcfSource | null;
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
}

export const ImportDialog = ({
  onOpenChange,
  open,
  source,
}: ImportDialogProps) => (
  <Dialog.Root open={open} onOpenChange={onOpenChange}>
    <Dialog.Content closeLabel={m.close()}>
      <Dialog.Title>{m.contacts_import_title()}</Dialog.Title>
      <Dialog.Description>{m.contacts_import_description()}</Dialog.Description>
      <Dialog.Body>
        {open && source !== null ? (
          <ImportBody
            onDone={() => {
              onOpenChange(false);
            }}
            source={source}
          />
        ) : null}
      </Dialog.Body>
    </Dialog.Content>
  </Dialog.Root>
);
