// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// ⌘K / Ctrl K (Superhuman, Linear): one box for actions, folders and
// settings, with their shortcuts in view. Arrow keys move, Enter runs.
// Typing text that isn't a command offers a mail search.

import { Dialog, Mark, SearchField, Shortcut } from "@nebulamail/ui";
import { colors, density, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useNavigate, useParams } from "@tanstack/react-router";
import { Search } from "lucide-react";
import { useId, useState } from "react";
import type { KeyboardEvent } from "react";

import { m } from "../../paraglide/messages.js";
import { matchesQuery } from "./command-filter";
import type { Command, CommandGroup } from "./commands";
import { byRank, useCommandList } from "./commands";
import { useMailUi } from "./mail-ui";
import { shared } from "./styles";
import { useKeys } from "./use-apple";

const styles = stylex.create({
  empty: { color: colors.fg3, margin: 0, padding: "16px 12px" },
  // Below the list, never over it: its own background and no negative
  // margin, so the list's last row can't show through.
  footer: {
    alignItems: "center",
    backgroundColor: colors.raised,
    borderTopColor: colors.line,
    borderTopStyle: "solid",
    borderTopWidth: "1px",
    color: colors.fg3,
    columnGap: "14px",
    display: "flex",
    fontSize: font.sizeSm,
    marginInline: "-22px",
    marginBottom: "-20px",
    paddingBlock: "10px",
    paddingInline: "16px",
    position: "relative",
  },
  footerItem: {
    alignItems: "center",
    columnGap: "5px",
    display: "inline-flex",
  },
  group: {
    color: colors.fg3,
    fontSize: font.sizeXs,
    fontWeight: font.weightSemibold,
    paddingBlock: "8px 4px",
    paddingInline: "12px",
  },
  icon: { color: colors.fg3, flexShrink: 0 },
  item: {
    alignItems: "center",
    borderRadius: radius.md,
    columnGap: "12px",
    cursor: "pointer",
    display: "flex",
    fontSize: font.sizeMd,
    height: density.menuItem,
    paddingInline: "12px",
    position: "relative",
  },
  itemActive: {
    "::before": {
      // Same place as the list's J/K cursor bar: 4px inside the fill,
      // 16px tall and centred.
      backgroundColor: colors.acc,
      borderRadius: "3px",
      content: '""',
      height: "18px",
      insetInlineStart: "4px",
      position: "absolute",
      top: "calc(50% - 9px)",
      width: "3px",
    },
    backgroundColor: colors.sel,
  },
  label: { flexGrow: 1 },
  // Bleeds to the dialog's edges, so its hairline runs edge to edge.
  field: { marginInline: "-22px", marginTop: "-20px" },
  list: {
    marginInline: "-16px",
    marginTop: "6px",
    // A row cut in half at the bottom edge looks broken: the last 16px fade
    // out instead, and the padding lets the last row scroll clear of it.
    maskImage:
      "linear-gradient(to bottom, #000 calc(100% - 16px), transparent)",
    maxHeight: "min(420px, 60dvh)",
    overflowY: "auto",
    paddingBottom: "16px",
    paddingInline: "6px",
  },
  note: {
    alignItems: "center",
    columnGap: "6px",
    display: "inline-flex",
    marginInlineStart: "auto",
  },
});

const GROUPS: readonly CommandGroup[] = ["actions", "goto", "settings"];

const GROUP_LABEL: Record<CommandGroup, () => string> = {
  actions: m.palette_actions,
  goto: m.palette_goto,
  settings: m.palette_settings,
};

const Option = ({
  active,
  command,
  id,
  keys,
  onHover,
  onRun,
}: {
  readonly active: boolean;
  readonly command: Command;
  readonly id: string;
  readonly keys: readonly string[] | null;
  readonly onHover: () => void;
  readonly onRun: () => void;
}) => (
  // Options are picked with the arrow keys from the combobox input; the
  // mouse can also click them (the listbox pattern, no own tab stop).
  // oxlint-disable-next-line jsx-a11y/click-events-have-key-events -- keyboard handled by the combobox (aria-activedescendant).
  <div
    aria-selected={active}
    id={id}
    onClick={onRun}
    onMouseMove={onHover}
    // oxlint-disable-next-line jsx-a11y/prefer-tag-over-role -- WAI-ARIA combobox pattern; no native element offers grouped options with shortcuts.
    role="option"
    tabIndex={-1}
    {...stylex.props(styles.item, active ? styles.itemActive : null)}
  >
    {command.icon === undefined ? null : (
      <command.icon aria-hidden size={16} {...stylex.props(styles.icon)} />
    )}
    <span {...stylex.props(styles.label, shared.ellipsis)}>
      {command.label}
    </span>
    {keys === null ? null : <Shortcut keys={keys} />}
  </div>
);

const PaletteBody = ({ onClose }: { readonly onClose: () => void }) => {
  const list = useCommandList();
  const keys = useKeys();
  const navigate = useNavigate();
  const params = useParams({ strict: false });
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const listId = useId();

  const trimmed = query.trim();
  const searchCommand: Command | null =
    trimmed === ""
      ? null
      : {
          group: "actions",
          icon: Search,
          id: "search-mail",
          label: m.palette_search_mail({ query: trimmed }),
          run: () => {
            void navigate({
              params: { mailbox: params.mailbox ?? "inbox" },
              search: { q: trimmed },
              to: "/mail/$mailbox",
            });
          },
        };
  const matches = list().filter((command) => matchesQuery(command, query));
  const ordered = [
    ...GROUPS.flatMap((group) =>
      byRank(matches.filter((command) => command.group === group))
    ),
    ...(searchCommand === null ? [] : [searchCommand]),
  ];
  const current = Math.min(active, Math.max(0, ordered.length - 1));

  const run = (command: Command | undefined): void => {
    if (command === undefined) {
      return;
    }
    onClose();
    command.run();
  };

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>): void => {
    switch (event.key) {
      case "ArrowDown": {
        event.preventDefault();
        setActive((current + 1) % Math.max(1, ordered.length));
        break;
      }
      case "ArrowUp": {
        event.preventDefault();
        setActive((current - 1 + ordered.length) % Math.max(1, ordered.length));
        break;
      }
      case "Enter": {
        event.preventDefault();
        run(ordered[current]);
        break;
      }
      default:
    }
  };

  const optionId = (index: number): string => `${listId}-${index}`;

  return (
    <>
      <div {...stylex.props(styles.field)}>
        <SearchField
          aria-activedescendant={
            ordered.length > 0 ? optionId(current) : undefined
          }
          aria-autocomplete="list"
          aria-controls={listId}
          aria-expanded
          // oxlint-disable-next-line jsx-a11y/no-autofocus -- the palette exists to be typed into.
          autoFocus
          embedded
          hint={<Shortcut keys={["Esc"]} />}
          label={m.palette_label()}
          onChange={(event) => {
            setQuery(event.target.value);
            setActive(0);
          }}
          onKeyDown={onKeyDown}
          placeholder={m.palette_placeholder()}
          // oxlint-disable-next-line jsx-a11y/prefer-tag-over-role -- WAI-ARIA combobox pattern; no native element offers grouped options with shortcuts.
          role="combobox"
          value={query}
        />
      </div>
      <div
        aria-label={m.palette_title()}
        id={listId}
        // oxlint-disable-next-line jsx-a11y/prefer-tag-over-role -- WAI-ARIA combobox pattern; no native element offers grouped options with shortcuts.
        role="listbox"
        {...stylex.props(styles.list)}
      >
        {ordered.length === 0 ? (
          <p {...stylex.props(styles.empty)}>{m.palette_empty()}</p>
        ) : null}
        {GROUPS.map((group) => {
          const items = ordered.filter(
            (command) => command.group === group && command !== searchCommand
          );
          return items.length === 0 ? null : (
            <div
              aria-label={GROUP_LABEL[group]()}
              key={group}
              // oxlint-disable-next-line jsx-a11y/prefer-tag-over-role -- WAI-ARIA combobox pattern; no native element offers grouped options with shortcuts.
              role="group"
            >
              <div aria-hidden {...stylex.props(styles.group)}>
                {GROUP_LABEL[group]()}
              </div>
              {items.map((command) => {
                const index = ordered.indexOf(command);
                return (
                  <Option
                    active={index === current}
                    command={command}
                    id={optionId(index)}
                    key={command.id}
                    keys={
                      command.shortcut === undefined
                        ? null
                        : keys(command.shortcut)
                    }
                    onHover={() => {
                      setActive(index);
                    }}
                    onRun={() => {
                      run(command);
                    }}
                  />
                );
              })}
            </div>
          );
        })}
        {searchCommand === null ? null : (
          <Option
            active={ordered.indexOf(searchCommand) === current}
            command={searchCommand}
            id={optionId(ordered.indexOf(searchCommand))}
            keys={null}
            onHover={() => {
              setActive(ordered.indexOf(searchCommand));
            }}
            onRun={() => {
              run(searchCommand);
            }}
          />
        )}
      </div>
      <div {...stylex.props(styles.footer)}>
        <span {...stylex.props(styles.footerItem)}>
          <Shortcut keys={["↑", "↓"]} />
          {m.palette_navigate()}
        </span>
        <span {...stylex.props(styles.footerItem)}>
          <Shortcut keys={["↵"]} />
          {m.palette_run()}
        </span>
        <span {...stylex.props(styles.note)}>
          <Mark label={null} size={14} />
          {m.palette_server_note()}
        </span>
      </div>
    </>
  );
};

export const CommandPalette = () => {
  const ui = useMailUi();
  const handleOpenChange = ui.setPaletteOpen;
  return (
    <Dialog.Root open={ui.paletteOpen} onOpenChange={handleOpenChange}>
      <Dialog.Content
        closeButton={false}
        closeLabel={m.close()}
        placement="top"
        size="lg"
      >
        <Dialog.Title visuallyHidden>{m.palette_title()}</Dialog.Title>
        <Dialog.Body>
          {ui.paletteOpen ? (
            <PaletteBody
              onClose={() => {
                ui.setPaletteOpen(false);
              }}
            />
          ) : null}
        </Dialog.Body>
      </Dialog.Content>
    </Dialog.Root>
  );
};
