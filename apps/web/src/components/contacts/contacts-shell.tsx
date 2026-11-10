// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Contacts' own frame inside the shared shell (shell.tsx): its sidebar
// ("New contact", All / Groups, address books), its top bar (the title and
// the contacts search, where mail has its search) and the phones'
// floating button. Nothing of mail's folders shows here.

import {
  ComposeButton,
  IconButton,
  NavItem,
  SearchField,
  Shortcut,
} from "@nebulamail/ui";
import * as stylex from "@stylexjs/stylex";
import {
  BookUser,
  PanelLeftClose,
  UserPlus,
  Users,
  UsersRound,
} from "lucide-react";

import { useAddressBooks, useContactList } from "../../contacts/hooks";
import { useScrollShrink } from "../../lib/use-scroll-shrink";
import { m } from "../../paraglide/messages.js";
import { AppBar, AppBarTitle } from "../mail/app-bar";
import { useMailUi } from "../mail/mail-ui";
import type { ShellSidebar } from "../mail/shell";
import { sidebarLayout } from "../mail/sidebar";
import { RailItem, SidebarRail } from "../mail/sidebar-rail";
import { SidebarSection } from "../mail/sidebar-section";
import { SidebarTop } from "../mail/sidebar-top";
import { useKeys } from "../mail/use-apple";
import { useContactsUi } from "./contacts-ui";

const styles = stylex.create({
  // The app bar's search column (app-bar.tsx): mail's place and width.
  search: { display: "block", flexGrow: 1, minWidth: 0, width: "100%" },
});

/** Cards and groups, for the sidebar's counts. */
const useCounts = (): { readonly all?: number; readonly groups?: number } => {
  const list = useContactList();
  if (list.data === undefined) {
    return {};
  }
  return {
    all: list.data.total,
    groups: list.data.contacts.filter((contact) => contact.group).length,
  };
};

const ContactsSidebarView = ({
  collapsed,
  onNavigate,
  onToggleCollapsed,
}: Parameters<ShellSidebar>[0]) => {
  const contacts = useContactsUi();
  const keys = useKeys();
  const counts = useCounts();
  const bookList = useAddressBooks();
  const show = (filter: "all" | "groups"): void => {
    contacts.setFilter(filter);
    onNavigate?.();
  };
  const create = (): void => {
    onNavigate?.();
    contacts.setCreating("individual");
  };
  const books = (): void => {
    onNavigate?.();
    contacts.setBooks(true);
  };

  if (collapsed) {
    return (
      <SidebarRail
        label={m.contacts_nav()}
        onExpand={onToggleCollapsed}
        primary={
          <ComposeButton
            icon={UserPlus}
            collapsed
            onClick={create}
            shortcut={keys("n")}
          >
            {m.contacts_new()}
          </ComposeButton>
        }
      >
        <RailItem
          active={contacts.filter === "all"}
          icon={Users}
          label={m.contacts_view_all()}
          onClick={() => {
            show("all");
          }}
        />
        <RailItem
          active={contacts.filter === "groups"}
          icon={UsersRound}
          label={m.contacts_filter_groups()}
          onClick={() => {
            show("groups");
          }}
        />
        <RailItem icon={BookUser} label={m.contacts_books()} onClick={books} />
      </SidebarRail>
    );
  }

  return (
    <nav aria-label={m.contacts_nav()} {...stylex.props(sidebarLayout.root)}>
      <SidebarTop
        current="contacts"
        onNavigate={onNavigate}
        xstyle={sidebarLayout.compose}
      >
        <ComposeButton icon={UserPlus} onClick={create} shortcut={keys("n")}>
          {m.contacts_new()}
        </ComposeButton>
      </SidebarTop>
      <div {...stylex.props(sidebarLayout.nav)}>
        <NavItem
          active={contacts.filter === "all"}
          icon={Users}
          mutedCount
          onClick={() => {
            show("all");
          }}
          {...(counts.all === undefined ? {} : { count: counts.all })}
        >
          {m.contacts_view_all()}
        </NavItem>
        <NavItem
          active={contacts.filter === "groups"}
          icon={UsersRound}
          mutedCount
          onClick={() => {
            show("groups");
          }}
          {...(counts.groups === undefined || counts.groups === 0
            ? {}
            : { count: counts.groups })}
        >
          {m.contacts_filter_groups()}
        </NavItem>
      </div>
      {/* Headed like mail's "Folders": the books, each opening the
          dialog that manages and shares them. */}
      <SidebarSection onToggle={null} open>
        {m.contacts_books()}
      </SidebarSection>
      <div {...stylex.props(sidebarLayout.nav)}>
        {(bookList.data ?? []).map((book) => (
          <NavItem
            icon={BookUser}
            key={book.key}
            onClick={books}
            title={book.name}
          >
            <bdi>{book.name}</bdi>
          </NavItem>
        ))}
      </div>
      <span {...stylex.props(sidebarLayout.spacer)} />
      {onToggleCollapsed === undefined ? null : (
        <div {...stylex.props(sidebarLayout.foot)}>
          <IconButton
            icon={PanelLeftClose}
            label={m.sidebar_collapse()}
            onClick={onToggleCollapsed}
            tooltipSide="right"
          />
        </div>
      )}
    </nav>
  );
};

export const contactsSidebar: ShellSidebar = (props) => (
  <ContactsSidebarView {...props} />
);

const ContactsSearch = () => {
  const ui = useMailUi();
  const keys = useKeys();
  const contacts = useContactsUi();
  return (
    <search {...stylex.props(styles.search)}>
      <SearchField
        hint={<Shortcut keys={keys("/")} />}
        label={m.contacts_search()}
        onChange={(event) => {
          contacts.setQuery(event.target.value);
        }}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown") {
            event.preventDefault();
            contacts.listKeys.current?.focusFirst();
          } else if (event.key === "Enter") {
            contacts.listKeys.current?.openFirst();
          } else if (event.key === "Escape") {
            // Clears the search only; doesn't also close the contact.
            event.preventDefault();
            contacts.setQuery("");
            event.currentTarget.blur();
          }
        }}
        placeholder={m.contacts_search()}
        ref={(element) => {
          // "/" focuses this box while the contacts page is open.
          ui.registerSearch(
            element instanceof HTMLInputElement ? element : null
          );
        }}
        value={contacts.query}
      />
    </search>
  );
};

export const ContactsBar = () => {
  const ui = useMailUi();
  return (
    <AppBar
      menuLabel={m.menu_open()}
      onMenu={() => {
        ui.setDrawerOpen(true);
      }}
      search={<ContactsSearch />}
      start={<AppBarTitle hideOnMobile>{m.rail_contacts()}</AppBarTitle>}
    />
  );
};

/** Phones: "New contact" where mail has "New message". */
export const ContactsFab = () => {
  const contacts = useContactsUi();
  const shrunk = useScrollShrink();
  return (
    <ComposeButton
      icon={UserPlus}
      collapsed={shrunk ? "fab" : false}
      onClick={() => {
        contacts.setCreating("individual");
      }}
    >
      {m.contacts_new()}
    </ComposeButton>
  );
};
