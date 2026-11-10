// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The contacts list panel: one row per card with roving focus (j/k or
// arrows, Enter opens), filtered by the top bar's search (in the browser:
// Stalwart matches whole words only) and the sidebar's All / Groups
// (contacts-shell.tsx; on phones a switch here). New contact (n), new
// group, import a .vcf from the device and export everything as .vcf.

import {
  Avatar,
  Button,
  EmptyState,
  IconButton,
  IllustrationNoResults,
  Menu,
  MenuItem,
  Panel,
  SegmentedControl,
  Spinner,
  useToast,
} from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useNavigate, useRouteContext } from "@tanstack/react-router";
import {
  BookUser,
  Download,
  FileUp,
  MoreHorizontal,
  UserPlus,
  Users,
} from "lucide-react";
import { useDeferredValue, useEffect, useId, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";

import type { ContactSummary } from "../../contacts/api";
import { exportVcf } from "../../contacts/api";
import { emptyForm } from "../../contacts/card";
import { useContactList, useContactsPush } from "../../contacts/hooks";
import { contactKey } from "../../contacts/ref";
import { searchContacts, sortContacts } from "../../contacts/search";
import { useShortcut } from "../../lib/shortcuts";
import { saveBlob } from "../../mail/download";
import { m } from "../../paraglide/messages.js";
import { getLocale } from "../../paraglide/runtime.js";
import { bp } from "../mail/breakpoints.stylex";
import type { Command } from "../mail/commands";
import { useCommands } from "../mail/commands";
import { LIST_AVATAR } from "../mail/email-row";
import { isInOverlay } from "../mail/mail-ui";
import { shared } from "../mail/styles";
import { BooksDialog } from "./books-dialog";
import { ContactEditor } from "./contact-editor";
import { useContactsUi } from "./contacts-ui";
import { ImportDialog } from "./import-dialog";

/** Rows rendered at first; "Show more" adds this many each time. */
const PAGE = 200;

const styles = stylex.create({
  center: {
    alignItems: "center",
    display: "flex",
    flexDirection: "column",
    flexGrow: 1,
    justifyContent: "center",
    padding: "24px",
    rowGap: "12px",
  },
  count: {
    color: colors.fg3,
    fontSize: font.sizeSm,
    marginInlineStart: { default: "4px", [bp.mobile]: "8px" },
  },
  head: {
    alignItems: "center",
    borderBottomColor: colors.line,
    borderBottomStyle: "solid",
    borderBottomWidth: "1px",
    columnGap: "6px",
    display: "flex",
    minHeight: "48px",
    paddingInline: "16px 8px",
  },
  heading: {
    fontSize: font.sizeXxl,
    fontWeight: 750,
    letterSpacing: "-0.02em",
    margin: 0,
  },
  // The top bar names the page on wider screens (contacts-shell.tsx);
  // phones show it here.
  headingPhoneOnly: {
    borderWidth: 0,
    clip: { default: "rect(0 0 0 0)", [bp.mobile]: "auto" },
    height: { default: "1px", [bp.mobile]: "auto" },
    marginInline: { default: "-1px", [bp.mobile]: 0 },
    overflow: "hidden",
    position: { default: "absolute", [bp.mobile]: "static" },
    whiteSpace: "nowrap",
    width: { default: "1px", [bp.mobile]: "auto" },
  },
  more: { display: "flex", justifyContent: "center", padding: "10px" },
  panel: {
    // The same width as the mail list (list-panel.tsx), so the divider
    // stays put when switching between mail and contacts.
    flexBasis: { default: "clamp(340px, 36%, 470px)", [bp.narrow]: "100%" },
    flexGrow: { default: 0, [bp.narrow]: 1 },
    flexShrink: 0,
    minWidth: 0,
  },
  panelHidden: { display: { default: "flex", [bp.narrow]: "none" } },
  panelMobile: {
    borderRadius: { default: radius.xl, [bp.mobile]: 0 },
    borderWidth: { default: "1px", [bp.mobile]: 0 },
  },
  row: {
    alignItems: "center",
    backgroundColor: { ":hover": colors.hover, default: "transparent" },
    borderRadius: radius.lg,
    columnGap: "12px",
    display: "flex",
    // As a two-line mail row (email-row.tsx): same height, avatar and
    // start edge, so people line up between Mail and Contacts.
    minHeight: "56px",
    paddingBlock: "6px",
    paddingInlineEnd: "16px",
    paddingInlineStart: "20px",
    width: "100%",
    boxSizing: "border-box",
  },
  rowActive: {
    backgroundColor: { ":hover": colors.sel, default: colors.sel },
    color: colors.selFg,
  },
  /** A group's icon in the avatar's box, so names line up. */
  groupIcon: {
    alignItems: "center",
    display: "grid",
    flexShrink: 0,
    justifyContent: "center",
  },
  /** The ring inside the row: rows sit tight and would touch it. */
  rowFocus: { outlineOffset: "-2px" },
  rowName: { fontSize: font.sizeMd, fontWeight: font.weightSemibold },
  rowSub: { color: colors.fg3, fontSize: font.sizeSm },
  rowText: {
    display: "flex",
    flexDirection: "column",
    minWidth: 0,
    rowGap: "1px",
  },
  rows: { flexGrow: 1, minHeight: 0, overflowY: "auto", padding: "4px 6px" },
  spacer: { flexGrow: 1 },
  // Phones only: there the sidebar (with All / Groups) is a drawer.
  tools: {
    alignItems: "center",
    columnGap: "8px",
    display: { default: "none", [bp.mobile]: "flex" },
    flexWrap: "wrap",
    paddingBlock: "10px 6px",
    paddingInline: "12px",
    rowGap: "8px",
  },
});

const subtitle = (contact: ContactSummary): string => {
  const main = contact.group
    ? m.contacts_group_count({ count: contact.members.length })
    : ([contact.emails[0], contact.organization, contact.phones[0]].find(
        (text) => text !== undefined && text !== ""
      ) ?? "");
  // Cards of address books shared with us say whose they are.
  return contact.owner === null
    ? main
    : [main, contact.owner].filter((text) => text !== "").join(" · ");
};

/** Runs `run` unless the key press happened in a dialog or menu. */
const guard =
  (run: () => void) =>
  (event: KeyboardEvent): void => {
    if (!isInOverlay(event.target)) {
      run();
    }
  };

export const ContactRow = ({
  active,
  contact,
  focused,
  onFocus,
  onKeyDown,
  onOpen,
}: {
  readonly contact: ContactSummary;
  readonly active: boolean;
  readonly focused: boolean;
  readonly onOpen: () => void;
  readonly onFocus: () => void;
  readonly onKeyDown: (event: ReactKeyboardEvent<HTMLButtonElement>) => void;
}) => {
  const name = contact.name === "" ? m.contacts_no_name() : contact.name;
  const avatar =
    LIST_AVATAR[useRouteContext({ from: "__root__" }).preferences.density];
  return (
    <li>
      <button
        aria-current={active ? "true" : undefined}
        data-contact-id={contact.key}
        onClick={onOpen}
        onFocus={onFocus}
        onKeyDown={onKeyDown}
        tabIndex={focused ? 0 : -1}
        type="button"
        {...stylex.props(
          shared.resetButton,
          styles.row,
          active ? styles.rowActive : null,
          shared.focusRing,
          styles.rowFocus
        )}
      >
        {contact.group ? (
          <span
            aria-hidden
            {...stylex.props(styles.groupIcon)}
            style={{ height: avatar, width: avatar }}
          >
            <Users size={20} />
          </span>
        ) : (
          <Avatar
            colorKey={contact.emails[0] ?? name}
            name={name}
            size={avatar}
          />
        )}
        <span {...stylex.props(styles.rowText)}>
          <bdi {...stylex.props(styles.rowName, shared.ellipsis)}>{name}</bdi>
          <bdi {...stylex.props(styles.rowSub, shared.ellipsis)}>
            {subtitle(contact)}
          </bdi>
        </span>
      </button>
    </li>
  );
};

const focusRow = (id: string | undefined): void => {
  if (id === undefined) {
    return;
  }
  // Rows are found by id: they re-render as the list filters.
  document
    .querySelector<HTMLButtonElement>(`[data-contact-id="${CSS.escape(id)}"]`)
    ?.focus();
};

export interface ContactListProps {
  /** The open contact's key (contacts/ref.ts). */
  readonly openId: string | undefined;
}

export const ContactList = ({ openId }: ContactListProps) => {
  const navigate = useNavigate();
  const toast = useToast();
  const list = useContactList();
  useContactsPush();
  const {
    books,
    creating,
    filter,
    query,
    registerListKeys,
    setBooks,
    setCreating,
    setFilter,
  } = useContactsUi();
  const [limit, setLimit] = useState(PAGE);
  // A new search or view starts from the first page again.
  const [lastSearch, setLastSearch] = useState(`${filter} ${query}`);
  if (lastSearch !== `${filter} ${query}`) {
    setLastSearch(`${filter} ${query}`);
    setLimit(PAGE);
  }
  const [focused, setFocused] = useState<string | undefined>(undefined);
  const [importing, setImporting] = useState<File | null>(null);
  const fileId = useId();

  const all = list.data?.contacts ?? [];
  const sorted = sortContacts(all, getLocale());
  // The list follows the typing a moment later: the box never waits.
  const deferredQuery = useDeferredValue(query);
  const visible = searchContacts(
    filter === "groups" ? sorted.filter((contact) => contact.group) : sorted,
    deferredQuery
  );
  const shown = visible.slice(0, limit);
  const focusedId =
    focused !== undefined && shown.some((contact) => contact.key === focused)
      ? focused
      : (openId ?? shown[0]?.key);

  const open = (id: string): void => {
    setFocused(id);
    void navigate({ params: { contactId: id }, to: "/contacts/$contactId" });
  };

  // The top bar's search box: Arrow down to the list, Enter opens the first.
  useEffect(() => {
    registerListKeys({
      focusFirst: () => {
        focusRow(shown[0]?.key);
      },
      openFirst: () => {
        if (shown[0] !== undefined) {
          open(shown[0].key);
        }
      },
    });
    return () => {
      registerListKeys(null);
    };
  });

  const step = (delta: 1 | -1): void => {
    const index = shown.findIndex((contact) => contact.key === focusedId);
    const next = shown[Math.max(0, Math.min(shown.length - 1, index + delta))];
    if (next !== undefined) {
      setFocused(next.key);
      focusRow(next.key);
    }
  };

  const exportAll = async (): Promise<void> => {
    try {
      const { missingPhotos, text } = await exportVcf(
        all.map((contact) => ({ accountId: contact.accountId, id: contact.id }))
      );
      saveBlob(new Blob([text], { type: "text/vcard" }), "contacts.vcf");
      if (missingPhotos > 0) {
        toast.show({
          title: m.contacts_export_photos_missing({ count: missingPhotos }),
        });
      }
    } catch {
      toast.show({
        priority: "high",
        title: m.contacts_export_failed(),
        tone: "danger",
      });
    }
  };

  useShortcut(
    "n",
    guard(() => {
      setCreating("individual");
    })
  );
  useShortcut(
    "j",
    guard(() => {
      step(1);
    })
  );
  useShortcut(
    "k",
    guard(() => {
      step(-1);
    })
  );

  const commands: Command[] = [
    {
      group: "actions",
      icon: UserPlus,
      id: "contacts-new",
      label: m.contacts_new(),
      run: () => {
        setCreating("individual");
      },
      shortcut: "n",
    },
    {
      group: "actions",
      icon: Users,
      id: "contacts-new-group",
      label: m.contacts_new_group(),
      run: () => {
        setCreating("group");
      },
    },
    {
      group: "actions",
      icon: BookUser,
      id: "contacts-books",
      label: m.contacts_books(),
      run: () => {
        setBooks(true);
      },
    },
    {
      group: "actions",
      icon: Download,
      id: "contacts-export",
      label: m.contacts_export_all(),
      run: () => {
        void exportAll();
      },
    },
  ];
  useCommands(commands);

  const body = () => {
    if (list.status === "pending") {
      return (
        <div {...stylex.props(styles.center)}>
          <Spinner label={m.contacts_loading()} />
        </div>
      );
    }
    if (list.status === "error") {
      return (
        <div {...stylex.props(styles.center)}>
          <EmptyState title={m.contacts_error()}>
            <Button
              onClick={() => {
                void list.refetch();
              }}
              variant="secondary"
            >
              {m.retry()}
            </Button>
          </EmptyState>
        </div>
      );
    }
    if (shown.length === 0) {
      return (
        <div {...stylex.props(styles.center)}>
          <EmptyState
            illustration={<IllustrationNoResults />}
            title={
              all.length === 0 ? m.contacts_empty() : m.contacts_none_found()
            }
          >
            {all.length === 0 ? m.contacts_empty_body() : null}
          </EmptyState>
        </div>
      );
    }
    return (
      <div {...stylex.props(styles.rows)}>
        <ul aria-label={m.contacts_label()} {...stylex.props(shared.resetList)}>
          {shown.map((contact) => (
            <ContactRow
              active={contact.key === openId}
              contact={contact}
              focused={contact.key === focusedId}
              key={contact.key}
              onFocus={() => {
                setFocused(contact.key);
              }}
              onKeyDown={(event) => {
                if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                  event.preventDefault();
                  step(event.key === "ArrowDown" ? 1 : -1);
                }
              }}
              onOpen={() => {
                open(contact.key);
              }}
            />
          ))}
        </ul>
        {visible.length > shown.length ? (
          <div {...stylex.props(styles.more)}>
            <Button
              onClick={() => {
                setLimit((current) => current + PAGE);
              }}
              variant="secondary"
            >
              {m.contacts_show_more({ count: visible.length - shown.length })}
            </Button>
          </div>
        ) : null}
      </div>
    );
  };

  return (
    <Panel
      label={m.contacts_label()}
      xstyle={[
        styles.panel,
        styles.panelMobile,
        openId === undefined ? null : styles.panelHidden,
      ]}
    >
      <div {...stylex.props(styles.head)}>
        <h1 {...stylex.props(styles.heading, styles.headingPhoneOnly)}>
          {m.rail_contacts()}
        </h1>
        {list.data === undefined ? null : (
          <span {...stylex.props(styles.count)}>
            {m.contacts_count({ count: list.data.total })}
          </span>
        )}
        <span {...stylex.props(styles.spacer)} />
        <input
          accept=".vcf,.vcard,text/vcard,text/x-vcard"
          id={fileId}
          onChange={(event) => {
            const file = event.currentTarget.files?.[0];
            event.currentTarget.value = "";
            if (file !== undefined) {
              setImporting(file);
            }
          }}
          tabIndex={-1}
          type="file"
          aria-hidden
          {...stylex.props(shared.srOnly)}
        />
        <Menu
          align="end"
          trigger={
            <IconButton icon={MoreHorizontal} label={m.contacts_more()} />
          }
        >
          <MenuItem
            icon={Users}
            onClick={() => {
              setCreating("group");
            }}
          >
            {m.contacts_new_group()}
          </MenuItem>
          <MenuItem
            icon={FileUp}
            onClick={() => {
              document
                .querySelector<HTMLInputElement>(`#${CSS.escape(fileId)}`)
                ?.click();
            }}
          >
            {m.contacts_import_file()}
          </MenuItem>
          <MenuItem
            icon={BookUser}
            onClick={() => {
              setBooks(true);
            }}
          >
            {m.contacts_books()}
          </MenuItem>
          <MenuItem
            disabled={all.length === 0}
            icon={Download}
            onClick={() => {
              void exportAll();
            }}
          >
            {m.contacts_export_all()}
          </MenuItem>
        </Menu>
      </div>
      <div {...stylex.props(styles.tools)}>
        <SegmentedControl
          label={m.contacts_filter()}
          onValueChange={setFilter}
          options={[
            { label: m.contacts_filter_all(), value: "all" },
            { label: m.contacts_filter_groups(), value: "groups" },
          ]}
          value={filter}
        />
      </div>
      {body()}
      <ContactEditor
        contacts={all}
        {...(creating === "group"
          ? { initial: emptyForm({ kind: "group" }) }
          : {})}
        onOpenChange={(next) => {
          if (!next) {
            setCreating(null);
          }
        }}
        onSaved={(ref) => {
          open(contactKey(ref));
        }}
        open={creating !== null}
      />
      <BooksDialog onOpenChange={setBooks} open={books} />
      <ImportDialog
        onOpenChange={(next) => {
          if (!next) {
            setImporting(null);
          }
        }}
        open={importing !== null}
        source={importing === null ? null : { file: importing, kind: "file" }}
      />
    </Panel>
  );
};
