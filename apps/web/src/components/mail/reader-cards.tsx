// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The cards of a conversation: open (with its body), folded to one line,
// a saved draft, or one that couldn't load.

import { cleanDisplayText } from "@nebulamail/mail-render";
import {
  Avatar,
  Badge,
  Button,
  EmptyState,
  IconButton,
  Menu,
  MenuItem,
  Spinner,
  useToast,
} from "@nebulamail/ui";
import { colors, font, radius, touch } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useNavigate } from "@tanstack/react-router";
import {
  ChevronUp,
  ChevronsUpDown,
  Download,
  Ellipsis,
  FileCode,
  Filter,
  Forward,
  PenLine,
  Printer,
  Reply,
  ReplyAll,
} from "lucide-react";

import { formatFullDate, formatThreadDate } from "../../lib/format";
import { useNow } from "../../lib/use-now";
import type { EmailSummary } from "../../mail/api";
import { useEmail } from "../../mail/hooks";
import { sameAddress } from "../../mail/recipients";
import { m } from "../../paraglide/messages.js";
import { getLocale } from "../../paraglide/runtime.js";
import { useComposer } from "./compose-context";
import type { ReaderView } from "./email-body";
import { EmailBody } from "./email-body";
import { OriginalDialog } from "./original-dialog";
import { readerStyles } from "./reader-styles";
import { shared } from "./styles";
import type { ReaderActions } from "./use-reader-actions";

const styles = stylex.create({
  cardActions: { alignItems: "center", columnGap: "2px", display: "flex" },
  folded: {
    alignItems: "center",
    // No hover on touch screens: a tap would leave the card highlighted.
    backgroundColor: {
      ":hover": { default: colors.hover, [touch.noHover]: colors.bg },
      default: colors.bg,
    },
    borderColor: colors.line,
    borderRadius: 0,
    borderStyle: "solid",
    borderTopWidth: 0,
    borderWidth: "1px",
    color: colors.fg,
    columnGap: "10px",
    cursor: "pointer",
    display: "flex",
    fontFamily: "inherit",
    fontSize: font.sizeMd,
    paddingBlock: "8px",
    paddingInline: "14px",
    // The focus ring sits above the next card of the block.
    position: "relative",
    textAlign: "start",
    width: "100%",
    zIndex: { ":focus-visible": 1, default: "auto" },
  },
  // Folded cards next to each other form one bordered block (hairline
  // between them): the first one closes the top, the last the bottom.
  foldedFirst: {
    borderStartEndRadius: radius.lg,
    borderStartStartRadius: radius.lg,
    borderTopWidth: "1px",
  },
  foldedLast: {
    borderEndEndRadius: radius.lg,
    borderEndStartRadius: radius.lg,
  },
  foldedName: {
    color: colors.fg,
    flexShrink: 0,
    fontWeight: font.weightMedium,
    maxWidth: "40%",
  },
  foldedPreview: { color: colors.fg3, flexGrow: 1, minWidth: 0 },
  foldedTime: {
    color: colors.fg3,
    flexShrink: 0,
    fontSize: font.sizeSm,
    fontVariantNumeric: "tabular-nums",
    whiteSpace: "nowrap",
  },
  foldedUnread: { color: colors.fg, fontWeight: font.weightSemibold },
  // Neutral, not an accent link: it is part of the thread, not navigation.
  more: { color: colors.fg2, fontWeight: font.weightMedium },
  moreLabel: { flexGrow: 1 },
  // Where the avatars of the other rows are: 28px, centred.
  moreStack: {
    alignItems: "center",
    display: "flex",
    flexShrink: 0,
    justifyContent: "center",
    width: "28px",
  },
});

/** Where a folded card sits in its run of folded cards. */
export interface FoldedPlace {
  readonly first: boolean;
  readonly last: boolean;
}

const foldedStyle = (place: FoldedPlace) => [
  styles.folded,
  place.first ? styles.foldedFirst : null,
  place.last ? styles.foldedLast : null,
];

const personLabel = (person: EmailSummary["from"][number]): string =>
  cleanDisplayText(person.name ?? "") || cleanDisplayText(person.email);

/** A card folded to one line (Zero/Gmail): sender, preview, date. */
export const CollapsedCard = ({
  email,
  onExpand,
  own,
  place,
}: {
  readonly email: EmailSummary;
  readonly onExpand: () => void;
  readonly own: readonly string[];
  readonly place: FoldedPlace;
}) => {
  const now = useNow();
  const [sender] = email.from;
  const fromMe =
    sender !== undefined &&
    own.some((address) => sameAddress(address, sender.email));
  const name = sender === undefined ? m.row_no_sender() : personLabel(sender);
  return (
    <button
      aria-expanded={false}
      onClick={onExpand}
      type="button"
      {...stylex.props(...foldedStyle(place), shared.focusRing)}
    >
      <Avatar colorKey={sender?.email ?? name} name={name} size={28} />
      <bdi
        {...stylex.props(
          styles.foldedName,
          email.unread ? styles.foldedUnread : null,
          shared.ellipsis
        )}
      >
        {fromMe ? m.reader_me() : name}
      </bdi>
      <span {...stylex.props(styles.foldedPreview, shared.ellipsis)}>
        {cleanDisplayText(email.preview)}
      </span>
      <time
        dateTime={email.receivedAt}
        title={formatFullDate(email.receivedAt, getLocale())}
        {...stylex.props(styles.foldedTime)}
      >
        {formatThreadDate(email.receivedAt, now, getLocale())}
      </time>
    </button>
  );
};

/**
 * The middle of a long run of folded messages, folded once more into one
 * row (Gmail, Superhuman): "3 more messages". Shows them all when pressed.
 */
export const MoreCard = ({
  count,
  onExpand,
  place,
}: {
  readonly count: number;
  readonly onExpand: () => void;
  readonly place: FoldedPlace;
}) => (
  <button
    aria-expanded={false}
    onClick={onExpand}
    type="button"
    {...stylex.props(...foldedStyle(place), styles.more, shared.focusRing)}
  >
    <span aria-hidden {...stylex.props(styles.moreStack)}>
      <ChevronsUpDown size={14} />
    </span>
    <span {...stylex.props(styles.moreLabel)}>{m.thread_more({ count })}</span>
  </button>
);

/** A saved reply in the conversation: opens in the composer. */
export const DraftCard = ({
  email,
  place,
}: {
  readonly email: EmailSummary;
  readonly place: FoldedPlace;
}) => {
  const composer = useComposer();
  const toast = useToast();
  const recipients = email.to.map(personLabel).join(", ");
  return (
    <button
      onClick={() => {
        composer.openDraft(email.id).catch(() => {
          toast.show({
            priority: "high",
            title: m.reader_error(),
            tone: "danger",
          });
        });
      }}
      type="button"
      {...stylex.props(...foldedStyle(place), shared.focusRing)}
    >
      <Badge icon={PenLine} tone="warn">
        {m.thread_draft()}
      </Badge>
      <bdi {...stylex.props(styles.foldedName, shared.ellipsis)}>
        {recipients === ""
          ? m.row_no_sender()
          : m.row_to({ names: recipients })}
      </bdi>
      <span {...stylex.props(styles.foldedPreview, shared.ellipsis)}>
        {cleanDisplayText(email.preview)}
      </span>
      <span {...stylex.props(styles.foldedTime)}>{m.thread_edit_draft()}</span>
    </button>
  );
};

export const NotLoaded = ({
  error,
  onRetry,
}: {
  readonly error: boolean;
  readonly onRetry: () => void;
}) => (
  <div {...stylex.props(readerStyles.center)}>
    <EmptyState
      title={error ? m.reader_error() : m.reader_not_found()}
      {...(error
        ? {
            action: (
              <Button onClick={onRetry} size="sm">
                {m.retry()}
              </Button>
            ),
          }
        : {})}
    />
  </div>
);

interface OpenCardProps {
  readonly email: EmailSummary;
  readonly actions: ReaderActions;
  readonly view: ReaderView;
  /** null: the only card, it can't fold. */
  readonly onCollapse: (() => void) | null;
}

/** A card open: fetches its own body (cached per email). */
export const OpenCard = ({
  actions,
  email,
  onCollapse,
  view,
}: OpenCardProps) => {
  const navigate = useNavigate();
  const query = useEmail(email.id);
  const detail = query.data;
  const handlePrinted = actions.printDone;

  if (query.isPending) {
    return (
      <div {...stylex.props(readerStyles.card, readerStyles.center)}>
        <Spinner label={m.reader_loading()} />
      </div>
    );
  }
  if (detail === undefined || detail === null) {
    return (
      <div {...stylex.props(readerStyles.card)}>
        <NotLoaded
          error={query.isError}
          onRetry={() => {
            void query.refetch();
          }}
        />
      </div>
    );
  }
  const [sender] = detail.from;
  const headerActions = (
    <span {...stylex.props(styles.cardActions)}>
      <IconButton
        icon={Reply}
        label={
          sender === undefined
            ? m.reply()
            : m.reply_to_name({
                name: cleanDisplayText(sender.name ?? sender.email),
              })
        }
        onClick={() => {
          actions.reply("reply", detail);
        }}
        size="sm"
      />
      <Menu
        align="end"
        trigger={
          <IconButton
            icon={Ellipsis}
            label={m.thread_message_actions()}
            size="sm"
          />
        }
      >
        <MenuItem
          icon={ReplyAll}
          onClick={() => {
            actions.reply("replyAll", detail);
          }}
        >
          {m.reply_all()}
        </MenuItem>
        <MenuItem
          icon={Forward}
          onClick={() => {
            actions.reply("forward", detail);
          }}
        >
          {m.forward()}
        </MenuItem>
        <MenuItem
          icon={Printer}
          onClick={() => {
            actions.print(detail.id);
          }}
        >
          {m.action_print()}
        </MenuItem>
        <MenuItem
          icon={Download}
          onClick={() => {
            actions.downloadEml(detail);
          }}
        >
          {m.action_download_eml()}
        </MenuItem>
        <MenuItem
          icon={FileCode}
          onClick={() => {
            actions.showOriginal(detail.id);
          }}
        >
          {m.action_view_original()}
        </MenuItem>
        <MenuItem
          icon={Filter}
          onClick={() => {
            void navigate({
              search: { from: detail.id },
              to: "/settings/filters",
            });
          }}
        >
          {m.filter_from_email()}
        </MenuItem>
      </Menu>
      {onCollapse === null ? null : (
        <IconButton
          icon={ChevronUp}
          label={m.thread_collapse()}
          onClick={onCollapse}
          size="sm"
        />
      )}
    </span>
  );

  return (
    <>
      <EmailBody
        email={detail}
        headerActions={headerActions}
        onPrinted={handlePrinted}
        own={actions.own}
        printing={actions.printingId === detail.id ? actions.printRun : null}
        view={view}
      />
      {actions.originalId === detail.id ? (
        <OriginalDialog
          email={detail}
          onDownload={() => {
            actions.downloadEml(detail);
          }}
          onOpenChange={(open) => {
            actions.showOriginal(open ? detail.id : null);
          }}
        />
      ) : null}
    </>
  );
};
