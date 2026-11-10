// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The top-bar search: "/" focuses it, Enter searches the server (`?q=` on
// the list route), Escape clears. A popover lists the search
// operators with examples in the user's language.

import { IconButton, Popover, SearchField, Shortcut } from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useNavigate, useParams, useSearch } from "@tanstack/react-router";
import { CircleHelp } from "lucide-react";
import { useState } from "react";

import { m } from "../../paraglide/messages.js";
import { bp } from "./breakpoints.stylex";
import { useMailUi } from "./mail-ui";
import { useKeys } from "./use-apple";

const styles = stylex.create({
  code: {
    backgroundColor: colors.hover,
    borderRadius: radius.xs,
    color: colors.fg,
    fontFamily: font.mono,
    fontSize: font.sizeSm,
    paddingBlock: "1px",
    paddingInline: "5px",
    whiteSpace: "nowrap",
  },
  contents: { display: "contents" },
  desc: { color: colors.fg2, margin: 0 },
  // As wide as the app bar's search column (app-bar.tsx), ≤520px.
  form: {
    display: "block",
    flexGrow: 1,
    minWidth: 0,
    width: "100%",
  },
  intro: { color: colors.fg2, fontSize: font.sizeSm, margin: 0 },
  note: { color: colors.fg3, fontSize: font.sizeSm, margin: 0 },
  op: {
    alignItems: "baseline",
    columnGap: "12px",
    display: "grid",
    fontSize: font.sizeSm,
    gridTemplateColumns: "auto 1fr",
    margin: 0,
    rowGap: "6px",
  },
  desktopOnly: { display: { default: "inline-flex", [bp.mobile]: "none" } },
  hints: { alignItems: "center", columnGap: "4px", display: "flex" },
});

const OPERATORS = [
  [m.search_op_from, m.search_op_from_desc],
  [m.search_op_to, m.search_op_to_desc],
  [m.search_op_subject, m.search_op_subject_desc],
  [m.search_op_attachment, m.search_op_attachment_desc],
  [m.search_op_unread, m.search_op_unread_desc],
  [m.search_op_before, m.search_op_before_desc],
  [m.search_op_after, m.search_op_after_desc],
] as const;

export const SearchBox = () => {
  const ui = useMailUi();
  const keys = useKeys();
  const navigate = useNavigate();
  const params = useParams({ strict: false });
  const search: { readonly q?: string } = useSearch({ strict: false });
  const urlQuery = search.q ?? "";
  const [text, setText] = useState(urlQuery);
  const [lastUrlQuery, setLastUrlQuery] = useState(urlQuery);
  // Follow the URL (back/forward, palette) without an effect.
  if (urlQuery !== lastUrlQuery) {
    setLastUrlQuery(urlQuery);
    setText(urlQuery);
  }

  const run = (query: string): void => {
    const trimmed = query.trim();
    void navigate({
      params: { mailbox: params.mailbox ?? "inbox" },
      search: trimmed === "" ? {} : { q: trimmed },
      to: "/mail/$mailbox",
    });
  };

  return (
    <search {...stylex.props(styles.form)}>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          run(text);
          ui.blurSearch();
        }}
      >
        <SearchField
          hint={
            <span {...stylex.props(styles.hints)}>
              <span {...stylex.props(styles.desktopOnly)}>
                <Shortcut keys={keys("/")} />
              </span>
              {/* The operators' help is for keyboards; phones keep the
                  room for typing. */}
              <span {...stylex.props(styles.desktopOnly)}>
                <Popover.Root>
                  <Popover.Trigger
                    render={
                      <IconButton
                        icon={CircleHelp}
                        label={m.search_help()}
                        size="xs"
                      />
                    }
                  />
                  <Popover.Content size="sm" align="end">
                    <Popover.Header>
                      <Popover.Title>{m.search_help()}</Popover.Title>
                      <Popover.Description>
                        {m.search_help_intro()}
                      </Popover.Description>
                    </Popover.Header>
                    <Popover.Section>
                      <dl {...stylex.props(styles.op)}>
                        {OPERATORS.map(([example, description]) => (
                          <div
                            key={example()}
                            {...stylex.props(styles.contents)}
                          >
                            <dt>
                              <code {...stylex.props(styles.code)}>
                                {example()}
                              </code>
                            </dt>
                            <dd {...stylex.props(styles.desc)}>
                              {description()}
                            </dd>
                          </div>
                        ))}
                      </dl>
                    </Popover.Section>
                    <Popover.Section>
                      <p {...stylex.props(styles.note)}>{m.search_op_both()}</p>
                    </Popover.Section>
                  </Popover.Content>
                </Popover.Root>
              </span>
            </span>
          }
          label={m.search_label()}
          onChange={(event) => {
            setText(event.target.value);
          }}
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              if (text === "" && urlQuery === "") {
                event.currentTarget.blur();
              } else {
                setText("");
                run("");
              }
            }
          }}
          placeholder={m.search_placeholder()}
          ref={(element) => {
            ui.registerSearch(
              element instanceof HTMLInputElement ? element : null
            );
          }}
          value={text}
        />
      </form>
    </search>
  );
};
