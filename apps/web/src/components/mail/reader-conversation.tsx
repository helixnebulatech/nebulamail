// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// One conversation in the reader: subject, its cards (newest, unread and
// opened ones open) and the reply bar.

import type { EmailId } from "@nebulamail/jmap-client";
import { cleanDisplayText } from "@nebulamail/mail-render";
import { Button } from "@nebulamail/ui";
import { colors, font } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { Forward, Reply, ReplyAll } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { ReactNode, RefObject } from "react";

import type { EmailDetail, EmailSummary, ThreadData } from "../../mail/api";
import type { ReplyMode } from "../../mail/reply";
import { foldLongRuns } from "../../mail/thread-fold";
import type { Entry } from "../../mail/thread-fold";
import { latestEmail } from "../../mail/threads";
import { m } from "../../paraglide/messages.js";
import { bp } from "./breakpoints.stylex";
import { useComposer } from "./compose-context";
import type { ReaderView } from "./email-body";
import { CollapsedCard, DraftCard, MoreCard, OpenCard } from "./reader-cards";
import { readerStyles } from "./reader-styles";
import { UnsubscribeButton } from "./unsubscribe-button";
import { useKeys } from "./use-apple";
import { usePhone } from "./use-phone";
import type { ReaderActions } from "./use-reader-actions";

const styles = stylex.create({
  cards: {
    display: "flex",
    flexDirection: "column",
    listStyle: "none",
    margin: 0,
    padding: 0,
  },
  // Space between blocks: an open card, or a run of folded ones.
  blockStart: { marginTop: "10px" },
  hiddenNote: { marginBottom: "12px", marginTop: 0 },
  // Right after the last message; sticks to the bottom only once the
  // conversation is taller than the pane (then it gets its hairline and
  // background, so the text scrolls under it).
  replyBar: {
    backgroundColor: colors.bg,
    borderTopColor: "transparent",
    borderTopStyle: "solid",
    borderTopWidth: "1px",
    bottom: 0,
    columnGap: "8px",
    display: "flex",
    // Bleeds to the pane's edges (the scroll area's inset), keeping the
    // buttons on the subject's and cards' start line.
    marginInline: { default: "-26px", [bp.mobile]: "-16px" },
    marginTop: "6px",
    paddingBottom: "14px",
    paddingInline: { default: "26px", [bp.mobile]: "16px" },
    paddingTop: "10px",
    position: "sticky",
    transitionDuration: "120ms",
    transitionProperty: "border-color",
    zIndex: 1,
  },
  replyBarStuck: { borderTopColor: colors.line },
  // A compose window floats over the bottom right: the bar would show
  // half under it. Kept in place (no jump), just not shown.
  replyBarCovered: {
    visibility: { default: "hidden", [bp.mobile]: "visible" },
  },
  // Seen at the very end of the scroll area only: while it's out of view
  // the bar is stuck over the text.
  replyEnd: { height: "1px", marginTop: "-1px" },
  // Content-width buttons, side by side. On phones three equal buttons,
  // icon and short label, in the small size (see ReplyBar).
  replyButton: {
    display: "flex",
    flexBasis: { default: "auto", [bp.mobile]: 0 },
    flexGrow: { default: 0, [bp.mobile]: 1 },
    minWidth: { default: "112px", [bp.mobile]: 0 },
  },
  scroll: {
    flexGrow: 1,
    minHeight: 0,
    overflowY: "auto",
    // No bottom padding: the reply bar is the last thing, with its own.
    paddingInline: { default: "26px", [bp.mobile]: "16px" },
    paddingTop: "20px",
  },
  threadCount: { color: colors.fg3 },
  subject: {
    fontSize: font.sizeXxl,
    fontWeight: font.weightBold,
    letterSpacing: "-0.03em",
    lineHeight: 1.25,
    margin: "0 0 8px",
    overflowWrap: "anywhere",
  },
  subjectMeta: {
    alignItems: "center",
    columnGap: "8px",
    display: "flex",
    flexWrap: "wrap",
    fontSize: font.sizeSm,
    marginBottom: "18px",
    minHeight: "4px",
  },
});

const ReplyBar = ({
  covered,
  onReply,
  pending,
  stuck,
}: {
  /** A compose window is open over it. */
  readonly covered: boolean;
  readonly onReply: (mode: ReplyMode) => void;
  /** The email to answer is still loading. */
  readonly pending: boolean;
  /** Stuck to the pane's bottom, over the text. */
  readonly stuck: boolean;
}) => {
  const keys = useKeys();
  // Key hints mean nothing on a phone (Shortcut already hides on touch).
  const phone = usePhone();
  const hint = (spec: string) => (phone ? {} : { shortcut: keys(spec) });
  // Phones: the small size (44px tall under a finger all the same) so the
  // icon and the short label fit three across at 360px.
  const size = phone ? "sm" : "md";
  return (
    <div
      data-print-hide
      {...stylex.props(
        styles.replyBar,
        stuck ? styles.replyBarStuck : null,
        covered ? styles.replyBarCovered : null
      )}
    >
      <span {...stylex.props(styles.replyButton)}>
        <Button
          disabled={pending}
          fullWidth
          size={size}
          icon={Reply}
          onClick={() => {
            onReply("reply");
          }}
          {...hint("r")}
        >
          {m.reply()}
        </Button>
      </span>
      <span {...stylex.props(styles.replyButton)}>
        <Button
          aria-label={m.reply_all()}
          disabled={pending}
          fullWidth
          size={size}
          icon={ReplyAll}
          onClick={() => {
            onReply("replyAll");
          }}
          {...hint("a")}
        >
          {phone ? m.reply_all_short() : m.reply_all()}
        </Button>
      </span>
      <span {...stylex.props(styles.replyButton)}>
        <Button
          aria-label={m.forward()}
          disabled={pending}
          fullWidth
          size={size}
          icon={Forward}
          onClick={() => {
            onReply("forward");
          }}
          {...hint("f")}
        >
          {m.forward()}
        </Button>
      </span>
    </div>
  );
};

/**
 * Whether the reply bar is stuck: true while `end` (after the bar, at the
 * very end of the scroll area) is out of view.
 */
const useStuck = (end: RefObject<HTMLElement | null>): boolean => {
  const [stuck, setStuck] = useState(false);
  useEffect(() => {
    const element = end.current;
    const observer = new IntersectionObserver(
      ([entry]) => {
        setStuck(entry !== undefined && !entry.isIntersecting);
      },
      { root: element?.parentElement ?? null }
    );
    if (element !== null) {
      observer.observe(element);
    }
    return () => {
      observer.disconnect();
    };
  }, [end]);
  return stuck;
};

interface ConversationProps {
  readonly emailId: EmailId;
  readonly thread: ThreadData;
  readonly emails: readonly EmailSummary[];
  readonly latest: EmailDetail | null | undefined;
  readonly actions: ReaderActions;
  readonly expanded: (email: EmailSummary) => boolean;
  readonly setExpanded: (id: EmailId, open: boolean) => void;
  readonly view: ReaderView;
}

export const Conversation = ({
  actions,
  emailId,
  emails,
  expanded,
  latest,
  setExpanded,
  thread,
  view,
}: ConversationProps) => {
  const scrolled = useRef(false);
  const end = useRef<HTMLDivElement>(null);
  const stuck = useStuck(end);
  const composing = useComposer().windows.some((window) => !window.minimized);
  const handleReply = actions.reply;
  const [first] = emails;
  const subject = cleanDisplayText(first?.subject ?? "") || m.row_no_subject();
  const latestId = latestEmail(emails)?.id;
  const hidden = thread.total - thread.emails.length;
  // "N more messages" stays unfolded for this conversation only.
  const [unfolded, setUnfolded] = useState<EmailId | null>(null);
  const entries = foldLongRuns(
    emails,
    (email) => !email.draft && !expanded(email) && email.id !== emailId,
    unfolded !== null && unfolded === first?.id
  );

  return (
    <>
      {/* What the browser's own print (menu, Ctrl+P from inside the
          email) prints: styles/app.css, «@media print». */}
      <div data-print-email {...stylex.props(styles.scroll)}>
        <h1 {...stylex.props(styles.subject)}>
          <bdi>{subject}</bdi>
        </h1>
        <div {...stylex.props(styles.subjectMeta)}>
          {emails.length > 1 ? (
            <span {...stylex.props(styles.threadCount)}>
              {m.row_thread({ count: emails.length })}
            </span>
          ) : null}
          {latest === undefined || latest === null ? null : (
            <UnsubscribeButton email={latest} key={latest.id} />
          )}
        </div>
        {hidden > 0 ? (
          <p
            role="note"
            {...stylex.props(readerStyles.notice, styles.hiddenNote)}
          >
            {m.thread_older_hidden({
              shown: thread.emails.length,
              total: thread.total,
            })}
          </p>
        ) : null}
        <ol aria-label={m.thread_label()} {...stylex.props(styles.cards)}>
          {entries.map((entry, index) => {
            const folded = (other: Entry | undefined) =>
              other !== undefined &&
              (other.kind === "more" ||
                other.email.draft ||
                !expanded(other.email));
            const place = {
              first: !folded(entries[index - 1]),
              last: !folded(entries[index + 1]),
            };
            const startsBlock = index > 0 && (!folded(entry) || place.first);
            if (entry.kind === "more") {
              return (
                <li
                  key={`more:${entry.from}`}
                  {...stylex.props(startsBlock ? styles.blockStart : null)}
                >
                  <MoreCard
                    count={entry.count}
                    onExpand={() => {
                      setUnfolded(first?.id ?? null);
                    }}
                    place={place}
                  />
                </li>
              );
            }
            const { email } = entry;
            const open = expanded(email);
            // A link to an older email of the conversation scrolls to it.
            const scrollTo =
              email.id === emailId && email.id !== latestId
                ? (element: HTMLLIElement | null) => {
                    if (element !== null && !scrolled.current) {
                      scrolled.current = true;
                      element.scrollIntoView({ block: "start" });
                    }
                  }
                : undefined;
            let card: ReactNode;
            if (email.draft) {
              card = <DraftCard email={email} place={place} />;
            } else if (open) {
              card = (
                <OpenCard
                  actions={actions}
                  email={email}
                  onCollapse={
                    emails.length > 1
                      ? () => {
                          setExpanded(email.id, false);
                        }
                      : null
                  }
                  view={view}
                />
              );
            } else {
              card = (
                <CollapsedCard
                  email={email}
                  onExpand={() => {
                    setExpanded(email.id, true);
                  }}
                  own={actions.own}
                  place={place}
                />
              );
            }
            return (
              <li
                key={email.id}
                ref={scrollTo}
                {...stylex.props(startsBlock ? styles.blockStart : null)}
              >
                {card}
              </li>
            );
          })}
        </ol>
        <ReplyBar
          covered={composing}
          onReply={handleReply}
          pending={actions.replyPending}
          stuck={stuck}
        />
        <div aria-hidden ref={end} {...stylex.props(styles.replyEnd)} />
      </div>
    </>
  );
};
