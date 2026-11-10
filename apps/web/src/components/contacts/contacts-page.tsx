// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The contacts app inside the mail shell: the list on
// the left, the open contact (or a placeholder) on the right; on narrow
// screens the open contact replaces the list, like the mail reader.

import {
  EmptyState,
  IllustrationContacts,
  IllustrationNoResults,
  Panel,
  Spinner,
} from "@nebulamail/ui";
import * as stylex from "@stylexjs/stylex";
import { useParams } from "@tanstack/react-router";
import type { ReactNode } from "react";

import { useContactsSupported } from "../../contacts/hooks";
import { parseContactKey } from "../../contacts/ref";
import { m } from "../../paraglide/messages.js";
import { bp } from "../mail/breakpoints.stylex";
import { ContactList } from "./contact-list";

const styles = stylex.create({
  center: {
    alignItems: "center",
    display: "flex",
    flexGrow: 1,
    justifyContent: "center",
    padding: "24px",
  },
  placeholder: {
    display: { default: "flex", [bp.narrow]: "none" },
    flexGrow: 1,
    minWidth: 0,
  },
  whole: { flexGrow: 1, minWidth: 0 },
});

/** Desktop: the right panel before a contact is opened. */
export const ContactsPlaceholder = () => (
  <Panel label={m.contacts_detail_label()} xstyle={styles.placeholder}>
    <EmptyState
      illustration={<IllustrationContacts compact />}
      pane
      title={m.contacts_pick_title()}
    >
      {m.contacts_pick_body()}
    </EmptyState>
  </Panel>
);

export const ContactsPage = ({
  children,
}: {
  readonly children: ReactNode;
}) => {
  const supported = useContactsSupported();
  const { contactId } = useParams({ strict: false });
  const openId =
    contactId !== undefined && parseContactKey(contactId) !== null
      ? contactId
      : undefined;
  if (supported.data !== true) {
    return (
      <Panel label={m.rail_contacts()} xstyle={styles.whole}>
        <div {...stylex.props(styles.center)}>
          {supported.data === false ? (
            <EmptyState
              illustration={<IllustrationNoResults />}
              title={m.contacts_unsupported()}
            />
          ) : (
            <Spinner label={m.contacts_loading()} />
          )}
        </div>
      </Panel>
    );
  }
  return (
    <>
      <ContactList openId={openId} />
      {children}
    </>
  );
};
