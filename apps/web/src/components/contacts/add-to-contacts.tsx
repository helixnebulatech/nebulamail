// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// From the mail reader: "Add to contacts" next to a sender (a prefilled
// editor, saved only when the user confirms), and "Import contacts" next
// to .vcf attachments (preview first).

import { cleanDisplayText } from "@nebulamail/mail-render";
import { Button, IconButton } from "@nebulamail/ui";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Contact, UserPlus } from "lucide-react";
import { Suspense, lazy, useState } from "react";

import { emptyForm } from "../../contacts/card";
import { useContactsSupported } from "../../contacts/hooks";
import { contactKey } from "../../contacts/ref";
import { isVcard } from "../../contacts/vcard-file";
import { loadWhenOnline } from "../../lib/load-when-online";
import type { Attachment, Person } from "../../mail/api";
import { listContacts } from "../../mail/contacts";
import { mailKeys } from "../../mail/queries";
import { sameAddress } from "../../mail/recipients";
import { m } from "../../paraglide/messages.js";

// The editor and the import dialog load when first opened (or hovered).
const loadEditor = async () =>
  await loadWhenOnline(async () => await import("./contact-editor"));
const loadImport = async () =>
  await loadWhenOnline(async () => await import("./import-dialog"));
const ContactEditor = lazy(async () => {
  const { ContactEditor: Editor } = await loadEditor();
  return { default: Editor };
});
const ImportDialog = lazy(async () => {
  const { ImportDialog: Dialog } = await loadImport();
  return { default: Dialog };
});
const preload = (load: () => Promise<unknown>) => (): void => {
  void load();
};

/** Same cache as the composer's suggestions (mail/use-suggestions.ts). */
const useKnownAddresses = (enabled: boolean) =>
  useQuery({
    enabled,
    queryFn: listContacts,
    queryKey: [...mailKeys.all, "contacts"],
    staleTime: 5 * 60 * 1000,
  });

export const AddToContacts = ({ person }: { readonly person: Person }) => {
  const navigate = useNavigate();
  const supported = useContactsSupported();
  const known = useKnownAddresses(supported.data === true);
  const [open, setOpen] = useState(false);
  // Mounted from the first opening on, so it can animate closing.
  const [used, setUsed] = useState(false);
  if (open && !used) {
    setUsed(true);
  }
  if (supported.data !== true || known.data === undefined) {
    return null;
  }
  const name = person.name ?? "";
  if (known.data.some((contact) => sameAddress(contact.email, person.email))) {
    return null;
  }
  return (
    <>
      <IconButton
        icon={UserPlus}
        label={m.contacts_add_sender()}
        onClick={() => {
          setOpen(true);
        }}
        onFocus={preload(loadEditor)}
        onPointerEnter={preload(loadEditor)}
        size="xs"
      />
      {used ? (
        <Suspense fallback={null}>
          <ContactEditor
            initial={emptyForm({ email: person.email, full: name })}
            onOpenChange={setOpen}
            onSaved={(ref) => {
              void navigate({
                params: { contactId: contactKey(ref) },
                to: "/contacts/$contactId",
              });
            }}
            open={open}
          />
        </Suspense>
      ) : null}
    </>
  );
};

/** Under the attachments: one button per .vcf. */
export const VcardImports = ({
  attachments,
}: {
  readonly attachments: readonly Attachment[];
}) => {
  const supported = useContactsSupported();
  const [importing, setImporting] = useState<Attachment | null>(null);
  // Mounted from the first import on, so it can animate closing.
  const [used, setUsed] = useState(false);
  if (importing !== null && !used) {
    setUsed(true);
  }
  const cards = attachments.filter((attachment) => isVcard(attachment));
  if (supported.data !== true || cards.length === 0) {
    return null;
  }
  return (
    <>
      {cards.map((attachment) => (
        <Button
          icon={Contact}
          key={attachment.blobId}
          onClick={() => {
            setImporting(attachment);
          }}
          onFocus={preload(loadImport)}
          onPointerEnter={preload(loadImport)}
          size="sm"
          variant="secondary"
        >
          <bdi>
            {m.contacts_import_from({
              name: cleanDisplayText(attachment.name),
            })}
          </bdi>
        </Button>
      ))}
      {used ? (
        <Suspense fallback={null}>
          <ImportDialog
            onOpenChange={(next) => {
              if (!next) {
                setImporting(null);
              }
            }}
            open={importing !== null}
            source={
              importing === null
                ? null
                : {
                    accountId: importing.accountId ?? null,
                    blobId: importing.blobId,
                    kind: "attachment",
                  }
            }
          />
        </Suspense>
      ) : null}
    </>
  );
};
