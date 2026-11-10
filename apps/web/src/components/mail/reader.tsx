// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The reading panel: one conversation as stacked cards (Zero, Gmail).
// Older emails fold to one line; the newest, the unread ones and the one
// opened start open. Each open card is a whole email (email-body.tsx).
// Toolbar actions act on the conversation; actions live in
// useReaderActions. The parts: reader-toolbar.tsx, reader-conversation.tsx,
// reader-cards.tsx, email-body.tsx and use-remote-images.ts.

import type { EmailId } from "@nebulamail/jmap-client";
import { DEFAULT_PRIVACY } from "@nebulamail/mail-render";
import { EmptyState, IconButton, Panel, Spinner } from "@nebulamail/ui";
import * as stylex from "@stylexjs/stylex";
import { ArrowLeft } from "lucide-react";
import { useState } from "react";

import type { EmailSummary } from "../../mail/api";
import { useEmail, useThread } from "../../mail/hooks";
import type { Expansion } from "../../mail/threads";
import {
  initialExpansion,
  isExpanded,
  latestEmail,
  shownEmails,
} from "../../mail/threads";
import { m } from "../../paraglide/messages.js";
import type { ReaderView } from "./email-body";
import { useListContent } from "./list-state";
import { NotLoaded } from "./reader-cards";
import { Conversation } from "./reader-conversation";
import { readerStyles } from "./reader-styles";
import { ReaderToolbar } from "./reader-toolbar";
import { useReaderActions } from "./use-reader-actions";

/**
 * The open conversation: `emailId` is any email of it (the list row's, a
 * notification's or a deep link's). Its thread comes from `Thread/get`,
 * filtered for this view (threads.ts); each open card fetches its body.
 */
export const Reader = ({ emailId }: { readonly emailId: EmailId }) => {
  const list = useListContent();
  const query = useThread(emailId);
  // The opened email's body starts loading with its thread, not after.
  useEmail(emailId);
  const thread = query.data;
  const emails =
    thread === undefined || thread === null
      ? []
      : shownEmails(thread.emails, list.state.context, emailId);
  const latest = latestEmail(emails);
  const latestQuery = useEmail(latest?.id);
  const latestDetail = latestQuery.data;
  const [view, setView] = useState<ReaderView>(DEFAULT_PRIVACY.defaultView);

  // Which cards start open is fixed on first load (state set in render).
  const [expansion, setExpansion] = useState<Expansion | null>(null);
  if (expansion === null && emails.length > 0) {
    setExpansion(initialExpansion(emails, emailId));
  }
  const [overrides, setOverrides] = useState<ReadonlyMap<EmailId, boolean>>(
    new Map()
  );
  const setAll = (open: boolean): void => {
    setOverrides(new Map(emails.map((email) => [email.id, open])));
  };
  const actions = useReaderActions({
    collapseAll: () => {
      // The newest stays open, like Gmail.
      setOverrides(
        new Map(emails.map((email) => [email.id, email.id === latest?.id]))
      );
    },
    emailId,
    emails,
    expandAll: () => {
      setAll(true);
    },
    latest: latestDetail,
    // Offline the query waits ("paused"): that counts as failed too.
    latestFailed:
      latestQuery.isError ||
      (latestQuery.isPending && latestQuery.fetchStatus === "paused"),
    retryLatest: () => {
      void latestQuery.refetch();
    },
  });
  // Printing or "ReaderView original" need the card open (they live in it).
  const expanded = (email: EmailSummary): boolean =>
    email.id === actions.printingId ||
    email.id === actions.originalId ||
    (expansion !== null && isExpanded(email, latest?.id, expansion, overrides));

  let content;
  if (query.isPending) {
    content = (
      <div {...stylex.props(readerStyles.center)}>
        <Spinner label={m.reader_loading()} />
      </div>
    );
  } else if (thread === undefined || thread === null || emails.length === 0) {
    content = (
      <NotLoaded
        error={query.isError}
        onRetry={() => {
          void query.refetch();
        }}
      />
    );
  } else {
    content = (
      <Conversation
        actions={actions}
        emailId={emailId}
        emails={emails}
        expanded={expanded}
        latest={latestDetail}
        setExpanded={(id, open) => {
          setOverrides((current) => new Map([...current, [id, open]]));
        }}
        thread={thread}
        view={view}
      />
    );
  }

  return (
    <Panel
      label={m.reader_label()}
      xstyle={[readerStyles.panel, readerStyles.panelMobile]}
    >
      <ReaderToolbar
        actions={actions}
        onToggleView={() => {
          setView((current) => (current === "html" ? "text" : "html"));
        }}
        latestId={latestDetail?.id}
        ready={latestDetail !== undefined && latestDetail !== null}
        view={view}
      />
      {content}
    </Panel>
  );
};

/** Something failed while showing an email: say so, keep the app usable. */
export const ReaderError = () => {
  const list = useListContent();
  return (
    <Panel
      label={m.reader_label()}
      xstyle={[readerStyles.panel, readerStyles.panelMobile]}
    >
      <div {...stylex.props(readerStyles.toolbar)}>
        <IconButton
          icon={ArrowLeft}
          label={m.reader_back()}
          onClick={() => {
            list.actions.close();
          }}
        />
      </div>
      <div {...stylex.props(readerStyles.center)}>
        <EmptyState title={m.reader_error()} />
      </div>
    </Panel>
  );
};
