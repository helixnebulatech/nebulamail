// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// One conversation in the list (Superhuman/Proton layout): unread dot in
// the gutter, avatar (the checkbox takes its place on hover), sender and
// thread count, subject, preview, icons and time.
// Comfortable: three lines. Compact: subject and preview share a line, and
// the whole row is one line once the list is wide (container query).
// Hover shows Gmail's quick actions; the focused row gets the galaxy bar.
// Every string from the email goes through cleanDisplayText (bidi).

import type { EmailId } from "@nebulamail/jmap-client";
import { cleanDisplayText } from "@nebulamail/mail-render";
import { Avatar, Checkbox, ContextMenu, IconButton } from "@nebulamail/ui";
import {
  colors,
  density,
  font,
  motion,
  radius,
  touch,
} from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { Link } from "@tanstack/react-router";
import { Archive, Mail, MailOpen, Paperclip, Star, Trash2 } from "lucide-react";
import { memo, useState } from "react";
import type { ComponentType, MouseEvent, ReactElement, Ref } from "react";

import {
  formatFullDate,
  formatListClock,
  formatListTime,
} from "../../lib/format";
import type { ConversationRow, EmailSummary, Person } from "../../mail/api";
import type { RecipientStatus } from "../../mail/delivery";
import { previewWithoutSubject } from "../../mail/preview-text";
import { sameAddress } from "../../mail/recipients";
import type { ConversationView, ThreadContext } from "../../mail/threads";
import { conversationView, participantNames } from "../../mail/threads";
import { m } from "../../paraglide/messages.js";
import { getLocale } from "../../paraglide/runtime.js";
import { DeliveryIcon } from "./delivery-status";
import { ScheduledRowMark } from "./scheduled-mark";
import { shared } from "./styles";

const ROW_PADDING = `calc(10px + (${density.row} - 46px) / 2)`;

/** Compact rows: one line once the list is this wide (full-page list). */
const WIDE_LIST = "@container mail-rows (min-width: 720px)";
/** Compact avatar (the comfortable one is 32 px). */
const COMPACT_AVATAR = 28;
/** List avatars by density, shared with the contact list. */
export const LIST_AVATAR = {
  comfortable: 32,
  compact: COMPACT_AVATAR,
} as const;
/**
 * The row's start gutter: the open row's bar (4–7 px), the unread dot (11–17 px)
 * and 3 px clear of the avatar, so both show on an open unread row.
 */
const GUTTER = "20px";

const styles = stylex.create({
  actions: {
    alignItems: "center",
    backgroundColor: colors.hover,
    borderRadius: radius.md,
    columnGap: "2px",
    // Hover UI, for the mouse only: no hover on touch screens (the reader
    // has every action), and the keyboard cursor doesn't open it (the
    // buttons aren't tabbable; the keys do the same: E, #, Shift+U, S).
    display: { default: "flex", [touch.noHover]: "none" },
    insetInlineEnd: "8px",
    opacity: { default: 0, [stylex.when.ancestor(":hover")]: 1 },
    paddingInline: "2px",
    pointerEvents: {
      default: "none",
      [stylex.when.ancestor(":hover")]: "auto",
    },
    position: "absolute",
    top: "6px",
    // A fade into the pill, so the text under it never ends mid-letter.
    "::before": {
      backgroundImage: `linear-gradient(to right, transparent, ${colors.hover})`,
      bottom: 0,
      content: '""',
      insetInlineEnd: "100%",
      pointerEvents: "none",
      position: "absolute",
      top: 0,
      width: "24px",
    },
  },
  actionsActive: {
    backgroundColor: colors.sel,
    "::before": {
      backgroundImage: `linear-gradient(to right, transparent, ${colors.sel})`,
    },
  },
  /** Compact rows: over the whole row (the time sits on either line). */
  actionsCentered: { bottom: "4px", top: "4px" },
  block: { display: "block" },
  /** Gives way to the checkbox (styles.check) on mouse hover. */
  avatarSlot: {
    flexShrink: 0,
    height: "32px",
    opacity: { default: 1, [stylex.when.ancestor(":hover")]: 0 },
    position: "relative",
    transitionDuration: motion.fast,
    transitionProperty: "opacity",
    transitionTimingFunction: motion.easing,
    width: "32px",
  },
  check: {
    alignItems: "center",
    display: "grid",
    insetInlineStart: GUTTER,
    justifyContent: "center",
    opacity: { default: 0, [stylex.when.ancestor(":hover")]: 1 },
    position: "absolute",
    top: ROW_PADDING,
    transitionDuration: motion.fast,
    transitionProperty: "opacity",
    transitionTimingFunction: motion.easing,
    height: "32px",
    width: "32px",
  },
  checkCompact: {
    height: `${COMPACT_AVATAR}px`,
    top: "50%",
    transform: "translateY(-50%)",
    width: `${COMPACT_AVATAR}px`,
  },
  checkVisible: { opacity: 1 },
  /**
   * Compact text: two lines (who · time, then subject — preview) in a
   * narrow list, one line (who · subject — preview · time) in a wide one.
   */
  compactText: {
    alignItems: "center",
    columnGap: { default: "8px", [WIDE_LIST]: "16px" },
    display: "grid",
    flexGrow: 1,
    gridTemplateAreas: {
      default: '"who meta" "body body"',
      [WIDE_LIST]: '"who body meta"',
    },
    gridTemplateColumns: {
      default: "minmax(0, 1fr) auto",
      [WIDE_LIST]: "clamp(140px, 20%, 220px) minmax(0, 1fr) auto",
    },
    minWidth: 0,
    rowGap: "2px",
  },
  compactWho: {
    alignItems: "center",
    columnGap: "6px",
    display: "flex",
    gridArea: "who",
    minWidth: 0,
  },
  compactBody: {
    gridArea: "body",
    lineHeight: "20px",
    minWidth: 0,
  },
  compactMeta: { gridArea: "meta" },
  compactLink: {
    alignItems: "center",
    paddingBlock: { default: "7px", [WIDE_LIST]: "6px" },
  },
  compactSubject: { color: colors.fg2, fontSize: font.sizeMd },
  compactPreview: { color: colors.fg3, fontSize: font.sizeMd },
  compactAvatar: {
    height: `${COMPACT_AVATAR}px`,
    width: `${COMPACT_AVATAR}px`,
  },
  count: {
    color: colors.fg3,
    fontSize: font.sizeSm,
    fontWeight: font.weightMedium,
  },
  /**
   * Unread: a dot in the gutter left of the avatar, centred on the first
   * text line (not a badge on the avatar).
   */
  dot: {
    backgroundColor: colors.acc,
    borderRadius: "50%",
    height: "6px",
    insetInlineStart: "11px",
    pointerEvents: "none",
    position: "absolute",
    top: `calc(${ROW_PADDING} + 7px)`,
    width: "6px",
  },
  /** On `sel` (open, checked, cursor): accText keeps ≥3:1 in both themes. */
  dotOnSel: { backgroundColor: colors.accText },
  dotCompact: {
    top: { default: "14px", [WIDE_LIST]: "calc(50% - 3px)" },
  },
  flag: { color: colors.star, fill: colors.star, flexShrink: 0 },
  hideAvatar: { opacity: 0 },
  icon: { color: colors.fg3, flexShrink: 0 },
  line1: {
    alignItems: "center",
    columnGap: "6px",
    display: "flex",
    height: "20px",
  },
  link: {
    color: "inherit",
    columnGap: "12px",
    display: "flex",
    paddingBlock: ROW_PADDING,
    paddingInlineEnd: "16px",
    paddingInlineStart: GUTTER,
    textDecoration: "none",
    borderRadius: radius.lg,
    outlineOffset: "-2px",
  },
  preview: {
    color: colors.fg3,
    fontSize: font.sizeSm,
  },
  row: {
    backgroundColor: { ":hover": colors.hover, default: "transparent" },
    borderRadius: radius.lg,
    position: "relative",
  },
  /**
   * The open conversation: the `sel` fill and the galaxy bar, 4 px in and
   * centred (the command palette's active item uses the same bar).
   */
  rowActive: {
    backgroundColor: { ":hover": colors.sel, default: colors.sel },
    "::before": {
      backgroundColor: colors.acc,
      borderRadius: "3px",
      content: '""',
      height: "18px",
      insetInlineStart: "4px",
      position: "absolute",
      top: "calc(50% - 9px)",
      width: "3px",
      zIndex: 1,
    },
  },
  rowChecked: {
    backgroundColor: { ":hover": colors.sel, default: colors.sel },
  },
  /**
   * The keyboard cursor (J/K) on a row that isn't open: the hover fill and
   * a thin inset ring, so it never looks like the open conversation (fill
   * and bar). On the open row the open style wins. No focus outline on top
   * (linkCursor), and no hover UI.
   */
  rowFocused: {
    backgroundColor: colors.hover,
    boxShadow: `inset 0 0 0 1px ${colors.focus}`,
  },
  linkCursor: {
    outlineStyle: { ":focus-visible": "none", default: "none" },
  },
  sender: { color: colors.fg2, fontWeight: font.weightMedium },
  senderUnread: { color: colors.fg, fontWeight: font.weightSemibold },
  subject: {
    color: colors.fg2,
    fontSize: font.sizeMd,
    marginTop: "1px",
  },
  subjectUnread: { color: colors.fg, fontWeight: font.weightSemibold },
  text: { flexGrow: 1, minWidth: 0 },
  /** Star, clip, delivery and time centred on one line, a fixed gap. */
  time: {
    alignItems: "center",
    color: colors.fg3,
    columnGap: "4px",
    display: "inline-flex",
    flexShrink: 0,
    fontSize: font.sizeSm,
    fontVariantNumeric: "tabular-nums",
    marginInlineStart: "auto",
    whiteSpace: "nowrap",
  },
  timeUnread: { color: colors.fg },
  /** "Yesterday 5:50 PM" where the list is wide; "Yesterday" otherwise. */
  clockWide: { display: { default: "none", [WIDE_LIST]: "inline" } },
});

const personName = (person: Person): string =>
  cleanDisplayText(person.name ?? "") || cleanDisplayText(person.email);

/** "Ana, Bob, me": first names once several people wrote (like Gmail). */
const participantsText = (
  people: readonly Person[],
  own: readonly string[]
): string => {
  const short = people.length > 1;
  return participantNames(
    people.map((person) => {
      if (own.some((address) => sameAddress(address, person.email))) {
        return m.list_me();
      }
      const name = personName(person);
      return short ? (name.split(/\s/u)[0] ?? name) : name;
    })
  );
};

/** The visible strings of a row, all bidi-safe. */
const rowText = (
  email: EmailSummary,
  conversation: ConversationView,
  showRecipients: boolean,
  own: readonly string[]
) => {
  const [sender] = email.from;
  const fallback =
    sender === undefined ? m.row_no_sender() : personName(sender);
  const senderName =
    conversation.participants.length > 0
      ? participantsText(conversation.participants, own)
      : fallback;
  const recipients = email.to.map(personName).join(", ");
  const [firstRecipient] = email.to;
  // Sent and Drafts show (and colour) the person it went to.
  const shown = showRecipients ? firstRecipient : sender;
  const subject = cleanDisplayText(email.subject);
  return {
    avatarKey: shown?.email ?? senderName,
    avatarName: shown === undefined ? senderName : personName(shown),
    preview: previewWithoutSubject(cleanDisplayText(email.preview), subject),
    subject: subject || m.row_no_subject(),
    who: showRecipients
      ? m.row_to({ names: recipients === "" ? m.row_no_sender() : recipients })
      : senderName,
  };
};

const RowMeta = ({
  conversation,
  delivery,
  email,
  now,
  scheduledAt,
}: {
  readonly conversation: ConversationView;
  readonly delivery: readonly RecipientStatus[] | undefined;
  readonly email: EmailSummary;
  readonly now: number;
  readonly scheduledAt: string | undefined;
}) => {
  const locale = getLocale();
  const time =
    now === 0 ? "" : formatListTime(email.receivedAt, new Date(now), locale);
  const clock =
    now === 0 ? null : formatListClock(email.receivedAt, new Date(now), locale);
  const { flagged, hasAttachment, unread } = conversation;
  const labels = [
    ...(flagged ? [m.row_flagged()] : []),
    ...(hasAttachment ? [m.row_attachment()] : []),
  ];
  return (
    <span {...stylex.props(styles.time, unread ? styles.timeUnread : null)}>
      {labels.length > 0 ? (
        <span {...stylex.props(shared.srOnly)}>{labels.join(", ")}</span>
      ) : null}
      {flagged ? (
        <Star aria-hidden size={13} {...stylex.props(styles.flag)} />
      ) : null}
      {hasAttachment ? (
        <Paperclip aria-hidden size={13} {...stylex.props(styles.icon)} />
      ) : null}
      {/* Scheduled: when it goes out instead of when it was scheduled. */}
      {scheduledAt === undefined ? (
        <>
          <DeliveryIcon recipients={delivery} />
          <time
            dateTime={email.receivedAt}
            title={formatFullDate(email.receivedAt, locale)}
          >
            {time}
            {clock === null ? null : (
              <span {...stylex.props(styles.clockWide)}> {clock}</span>
            )}
          </time>
        </>
      ) : (
        <ScheduledRowMark sendAt={scheduledAt} />
      )}
    </span>
  );
};

type RowText = ReturnType<typeof rowText>;

const UnreadMark = ({ unread }: { readonly unread: boolean }) =>
  unread ? (
    <span {...stylex.props(shared.srOnly)}>{m.row_unread()}: </span>
  ) : null;

/** Sender (or recipients) and the thread's message count. */
const Who = ({
  count,
  text,
  unread,
}: {
  readonly count: number;
  readonly text: RowText;
  readonly unread: boolean;
}) => (
  <>
    <bdi
      {...stylex.props(
        styles.sender,
        unread ? styles.senderUnread : null,
        shared.ellipsis
      )}
    >
      {text.who}
    </bdi>
    {count > 1 ? (
      <span {...stylex.props(styles.count)}>
        <span aria-hidden>{count}</span>
        <span {...stylex.props(shared.srOnly)}>{m.row_thread({ count })}</span>
      </span>
    ) : null}
  </>
);

/** Compact text: subject and preview on one line (styles.compactText). */
const CompactText = ({
  conversation,
  count,
  delivery,
  email,
  now,
  scheduledAt,
  text,
  unread,
}: {
  readonly conversation: ConversationView;
  readonly count: number;
  readonly delivery: readonly RecipientStatus[] | undefined;
  readonly email: EmailSummary;
  readonly now: number;
  readonly scheduledAt: string | undefined;
  readonly text: RowText;
  readonly unread: boolean;
}) => (
  <span {...stylex.props(styles.compactText)}>
    <span {...stylex.props(styles.compactWho)}>
      <Who count={count} text={text} unread={unread} />
    </span>
    <span {...stylex.props(styles.compactBody, shared.ellipsis, styles.block)}>
      <bdi
        {...stylex.props(
          styles.compactSubject,
          unread ? styles.subjectUnread : null
        )}
      >
        <UnreadMark unread={unread} />
        {text.subject}
      </bdi>
      {text.preview === "" ? null : (
        <span {...stylex.props(styles.compactPreview)}>
          {" \u2014 "}
          {text.preview}
        </span>
      )}
    </span>
    <span {...stylex.props(styles.compactMeta)}>
      <RowMeta
        conversation={conversation}
        delivery={delivery}
        email={email}
        now={now}
        scheduledAt={scheduledAt}
      />
    </span>
  </span>
);

interface QuickActionsProps {
  readonly email: EmailSummary;
  readonly conversation: ConversationView;
  readonly highlighted: boolean;
  /** Compact rows: over the whole row instead of its first line. */
  readonly centered: boolean;
  /**
   * The row handlers get the row's id, so the list can pass the same
   * functions to every row and render after render (EmailRow is memo).
   */
  /** Each one is left out (undefined) when the folder doesn't allow it. */
  readonly onArchive: ((id: EmailId) => void) | undefined;
  readonly onTrash: ((id: EmailId) => void) | undefined;
  readonly onSeen: ((id: EmailId, seen: boolean) => void) | undefined;
  readonly onFlag: ((id: EmailId, flagged: boolean) => void) | undefined;
}

/** Gmail's hover actions. Not in the tab order: the shortcuts do the same. */
const QuickActions = ({
  centered,
  conversation,
  email,
  highlighted,
  onArchive,
  onFlag,
  onSeen,
  onTrash,
}: QuickActionsProps) => (
  <span
    {...stylex.props(
      styles.actions,
      highlighted ? styles.actionsActive : null,
      centered ? styles.actionsCentered : null
    )}
  >
    {onArchive === undefined ? null : (
      <IconButton
        icon={Archive}
        label={m.action_archive()}
        onClick={() => {
          onArchive(email.id);
        }}
        shortcut={["E"]}
        size="sm"
        tabIndex={-1}
      />
    )}
    {onTrash === undefined ? null : (
      <IconButton
        icon={Trash2}
        label={m.action_trash()}
        onClick={() => {
          onTrash(email.id);
        }}
        shortcut={["#"]}
        size="sm"
        tabIndex={-1}
      />
    )}
    {onSeen === undefined ? null : (
      <IconButton
        icon={conversation.unread ? MailOpen : Mail}
        label={
          conversation.unread ? m.action_mark_read() : m.action_mark_unread()
        }
        onClick={() => {
          onSeen(email.id, conversation.unread);
        }}
        size="sm"
        tabIndex={-1}
      />
    )}
    {onFlag === undefined ? null : (
      <IconButton
        icon={Star}
        label={conversation.flagged ? m.action_unflag() : m.action_flag()}
        onClick={() => {
          onFlag(email.id, !conversation.flagged);
        }}
        pressed={conversation.flagged}
        shortcut={["S"]}
        size="sm"
        tabIndex={-1}
      />
    )}
  </span>
);

/** What the row's context menu needs to know about the row. */
export interface RowMenuProps {
  readonly id: EmailId;
  readonly unread: boolean;
  readonly flagged: boolean;
}

export interface EmailRowProps extends Omit<
  QuickActionsProps,
  "highlighted" | "conversation" | "email" | "centered"
> {
  /** Compact density (Settings → Density): denser layout, see the top. */
  readonly compact: boolean;
  readonly email: ConversationRow;
  /** The list's thread context: what the row shows of its conversation. */
  readonly context: ThreadContext;
  readonly mailbox: string;
  readonly now: number;
  readonly active: boolean;
  /** Holds the list's keyboard cursor (tabbable). */
  readonly focused: boolean;
  /** Draw the cursor bar (see ListState.cursorShown). */
  readonly cursor: boolean;
  readonly checked: boolean;
  readonly selecting: boolean;
  /** Sent/Drafts: show who it went to. */
  readonly showRecipients: boolean;
  /** Sent: what happened to it, per recipient (delivery.ts). */
  readonly delivery?: readonly RecipientStatus[] | undefined;
  /** Sent: when it goes out, if it is still scheduled. */
  readonly scheduledAt?: string | undefined;
  /** The user's addresses, shown as "me". */
  readonly own: readonly string[];
  /** The row's link, mounted (element) or gone (null), for the cursor. */
  readonly onLink?: (id: EmailId, element: HTMLAnchorElement | null) => void;
  /**
   * The right-click (long-press on touch) menu's items. A component, not
   * elements, so the memo'd row gets the same prop on every render.
   */
  readonly menu?: ComponentType<RowMenuProps> | undefined;
  /** Position in the list, for the virtual list's measuring and ARIA. */
  readonly index: number;
  readonly itemRef?: Ref<HTMLLIElement>;
  /** Rows in the whole list; -1 while more are still to load. */
  readonly setSize: number;
  readonly onOpen: (id: EmailId) => void;
  readonly onFocus: (id: EmailId) => void;
  readonly onCheck: (id: EmailId, checked: boolean) => void;
}

/** Unread: the dot in the gutter (screen readers hear UnreadMark). */
const RowDot = ({
  compact,
  onSel,
  unread,
}: {
  readonly compact: boolean;
  readonly onSel: boolean;
  readonly unread: boolean;
}) =>
  unread ? (
    <span
      aria-hidden
      {...stylex.props(
        styles.dot,
        compact ? styles.dotCompact : null,
        onSel ? styles.dotOnSel : null
      )}
    />
  ) : null;

/** The row inside its context menu, when the list gives one. */
const WithRowMenu = ({
  children,
  menu: Items,
  ...row
}: RowMenuProps & {
  readonly children: ReactElement;
  readonly menu: ComponentType<RowMenuProps> | undefined;
}) =>
  Items === undefined ? (
    children
  ) : (
    <ContextMenu menu={<Items {...row} />}>{children}</ContextMenu>
  );

const isPlainClick = (event: MouseEvent<HTMLAnchorElement>): boolean =>
  !event.metaKey && !event.ctrlKey && !event.shiftKey && event.button === 0;

const Row = ({
  active,
  checked,
  compact,
  context,
  email,
  cursor,
  delivery,
  focused,
  index,
  itemRef,
  onLink,
  mailbox,
  now,
  onCheck,
  onFocus,
  onOpen,
  own,
  scheduledAt,
  selecting,
  setSize,
  showRecipients,
  menu: RowMenu,
  ...quick
}: EmailRowProps) => {
  const conversation = conversationView(email, context);
  const linkRef = (element: HTMLAnchorElement | null): void => {
    onLink?.(email.id, element);
  };
  const text = rowText(email, conversation, showRecipients, own);
  const { count, unread } = conversation;
  const showCheck = selecting || checked;
  /** On the `sel` fill: open or checked (the cursor alone uses `hover`). */
  const highlighted = active || checked;
  // The cursor's ring only while the keyboard drives the list (J/K, Tab,
  // Esc back from the reader): after opening and closing an email with the
  // mouse the row keeps the focus, but nothing should look selected.
  const [keyboard, setKeyboard] = useState(false);

  const row = (
    <li
      aria-posinset={index + 1}
      aria-setsize={setSize}
      data-index={index}
      ref={itemRef}
      {...stylex.props(
        stylex.defaultMarker(),
        styles.row,
        active ? styles.rowActive : null,
        checked ? styles.rowChecked : null,
        cursor && keyboard && !active && !checked ? styles.rowFocused : null
      )}
    >
      <Link
        aria-current={active ? "true" : undefined}
        onClick={(event) => {
          // New tab / window keeps the browser's behaviour.
          if (isPlainClick(event)) {
            event.preventDefault();
            onOpen(email.id);
          }
        }}
        onBlur={() => {
          setKeyboard(false);
        }}
        onFocus={(event) => {
          setKeyboard(event.currentTarget.matches(":focus-visible"));
          onFocus(email.id);
        }}
        params={{ emailId: email.id, mailbox }}
        ref={linkRef}
        search={(previous) => previous}
        tabIndex={focused ? 0 : -1}
        to="/mail/$mailbox/$emailId"
        {...stylex.props(
          styles.link,
          compact ? styles.compactLink : null,
          shared.focusRing,
          cursor ? styles.linkCursor : null
        )}
      >
        <span
          {...stylex.props(
            styles.avatarSlot,
            compact ? styles.compactAvatar : null,
            showCheck ? styles.hideAvatar : null
          )}
        >
          <Avatar
            colorKey={text.avatarKey}
            name={text.avatarName}
            size={compact ? COMPACT_AVATAR : 32}
          />
        </span>
        {compact ? (
          <CompactText
            conversation={conversation}
            count={count}
            delivery={delivery}
            email={email}
            now={now}
            scheduledAt={scheduledAt}
            text={text}
            unread={unread}
          />
        ) : (
          <span {...stylex.props(styles.text)}>
            <span {...stylex.props(styles.line1)}>
              <Who count={count} text={text} unread={unread} />
              <RowMeta
                conversation={conversation}
                delivery={delivery}
                email={email}
                now={now}
                scheduledAt={scheduledAt}
              />
            </span>
            <bdi
              {...stylex.props(
                styles.subject,
                unread ? styles.subjectUnread : null,
                shared.ellipsis,
                styles.block
              )}
            >
              <UnreadMark unread={unread} />
              {text.subject}
            </bdi>
            <span
              {...stylex.props(styles.preview, shared.ellipsis, styles.block)}
            >
              {text.preview}
            </span>
          </span>
        )}
      </Link>
      <RowDot compact={compact} onSel={highlighted} unread={unread} />
      <span
        {...stylex.props(
          styles.check,
          compact ? styles.checkCompact : null,
          showCheck ? styles.checkVisible : null
        )}
      >
        <Checkbox
          aria-label={`${m.select_row()}: ${text.subject}`}
          checked={checked}
          onCheckedChange={(value) => {
            onCheck(email.id, value);
          }}
          tabIndex={-1}
        />
      </span>
      <QuickActions
        centered={compact}
        conversation={conversation}
        email={email}
        highlighted={highlighted}
        {...quick}
      />
    </li>
  );
  return (
    <WithRowMenu
      flagged={conversation.flagged}
      id={email.id}
      menu={RowMenu}
      unread={unread}
    >
      {row}
    </WithRowMenu>
  );
};

/**
 * Memo: the list redraws on every cursor move, selection and push, and
 * passes stable props (handlers taking the id), so only the rows whose
 * data changed render again.
 */
export const EmailRow = memo(Row);
