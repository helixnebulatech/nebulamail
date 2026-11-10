// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { EmptyState, IllustrationOpenMail, Panel } from "@nebulamail/ui";
import * as stylex from "@stylexjs/stylex";

import { m } from "../../paraglide/messages.js";
import { bp } from "./breakpoints.stylex";
import { useListContent } from "./list-state";
import { useReadingPane } from "./use-reading-pane";

const styles = stylex.create({
  panel: {
    display: { default: "flex", [bp.narrow]: "none" },
    flexGrow: 1,
    minWidth: 0,
  },
});

/**
 * The reading pane before a message is opened (desktop only, and only when
 * emails open next to the list). "Pick a message" only when there is one to
 * pick: an empty folder or search already says so in the list, and two
 * empty states side by side are noise (then the pane stays empty: a mark
 * there read as the loading spinner).
 */
export const ReaderPlaceholder = () => {
  const pane = useReadingPane();
  const { state } = useListContent();
  if (pane === "full") {
    return null;
  }
  const hasRows =
    state.status === "success" && state.pages.some((page) => page.length > 0);
  return (
    <Panel label={m.reader_label()} xstyle={styles.panel}>
      {hasRows ? (
        <EmptyState
          illustration={<IllustrationOpenMail compact />}
          pane
          title={m.reader_empty_title()}
        >
          {m.reader_empty_body()}
        </EmptyState>
      ) : null}
    </Panel>
  );
};
