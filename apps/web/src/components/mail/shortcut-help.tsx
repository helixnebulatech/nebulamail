// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// "?" — every keyboard shortcut, grouped (keyboard first).

import { Dialog, Shortcut } from "@nebulamail/ui";
import { colors, font } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";

import { m } from "../../paraglide/messages.js";
import { useMailUi } from "./mail-ui";
import { useKeys } from "./use-apple";

const styles = stylex.create({
  // Balanced columns (CSS multi-column): the groups flow into as many
  // 240px columns as fit and the columns end at about the same height,
  // where a grid left one column far longer than the other.
  grid: {
    columnGap: "28px",
    columnWidth: "240px",
    // Room under the dialog's title before the first group.
    paddingTop: "10px",
  },
  // A long group may continue in the next column (so the two end level),
  // but its heading never stays behind alone, nor a row split in two.
  group: {
    display: "block",
    paddingBottom: "20px",
  },
  // Sentence case, like the sidebars' section headings.
  heading: {
    breakAfter: "avoid",
    breakInside: "avoid",
    color: colors.fg3,
    fontSize: font.sizeSm,
    fontWeight: font.weightSemibold,
    margin: "0 0 6px",
  },
  row: {
    alignItems: "center",
    breakInside: "avoid",
    columnGap: "12px",
    display: "flex",
    justifyContent: "space-between",
    minHeight: "30px",
  },
  term: { color: colors.fg },
  list: { margin: 0 },
  keys: { margin: 0 },
});

interface Entry {
  readonly spec: string;
  readonly label: () => string;
}

const SECTIONS: readonly {
  readonly title: () => string;
  readonly entries: readonly Entry[];
}[] = [
  {
    entries: [
      { label: m.compose_new, spec: "c" },
      { label: m.sc_search, spec: "/" },
      { label: m.sc_palette, spec: "mod+k" },
      { label: m.sc_send, spec: "mod+enter" },
      { label: m.sc_schedule, spec: "mod+shift+enter" },
      { label: m.sc_help, spec: "?" },
    ],
    title: m.shortcuts_general,
  },
  {
    entries: [
      { label: m.sc_next, spec: "j" },
      { label: m.sc_prev, spec: "k" },
      { label: m.sc_open, spec: "enter" },
      { label: m.select_row, spec: "x" },
      { label: m.sc_back, spec: "escape" },
    ],
    title: m.shortcuts_list,
  },
  {
    entries: [
      { label: m.action_archive, spec: "e" },
      { label: m.action_trash, spec: "#" },
      { label: m.action_mark_unread, spec: "shift+u" },
      { label: m.action_mark_read, spec: "shift+i" },
      { label: m.action_flag, spec: "s" },
      { label: m.action_move, spec: "v" },
      { label: m.action_spam, spec: "!" },
      { label: m.reply, spec: "r" },
      { label: m.reply_all, spec: "a" },
      { label: m.forward, spec: "f" },
    ],
    title: m.shortcuts_message,
  },
  {
    entries: [
      { label: m.nav_inbox, spec: "g i" },
      { label: m.nav_starred, spec: "g s" },
      { label: m.nav_sent, spec: "g t" },
      { label: m.nav_drafts, spec: "g d" },
      { label: m.rail_calendar, spec: "g c" },
      { label: m.rail_contacts, spec: "g p" },
      { label: m.rail_tasks, spec: "g k" },
    ],
    title: m.shortcuts_goto,
  },
  {
    entries: [
      { label: m.contacts_new, spec: "n" },
      { label: m.contacts_edit, spec: "e" },
      { label: m.contacts_delete, spec: "#" },
      { label: m.contacts_write, spec: "r" },
    ],
    title: m.rail_contacts,
  },
];

export const ShortcutHelp = () => {
  const ui = useMailUi();
  const keys = useKeys();
  const handleOpenChange = ui.setHelpOpen;
  return (
    <Dialog.Root open={ui.helpOpen} onOpenChange={handleOpenChange}>
      <Dialog.Content closeLabel={m.close()} size="lg">
        <Dialog.Title>{m.help_shortcuts()}</Dialog.Title>
        <Dialog.Body>
          <div {...stylex.props(styles.grid)}>
            {SECTIONS.map((section) => (
              <section key={section.title()} {...stylex.props(styles.group)}>
                <h3 {...stylex.props(styles.heading)}>{section.title()}</h3>
                <dl {...stylex.props(styles.list)}>
                  {section.entries.map((entry) => (
                    <div key={entry.spec} {...stylex.props(styles.row)}>
                      <dt {...stylex.props(styles.term)}>{entry.label()}</dt>
                      <dd {...stylex.props(styles.keys)}>
                        <Shortcut keys={keys(entry.spec)} />
                      </dd>
                    </div>
                  ))}
                </dl>
              </section>
            ))}
          </div>
        </Dialog.Body>
      </Dialog.Content>
    </Dialog.Root>
  );
};
