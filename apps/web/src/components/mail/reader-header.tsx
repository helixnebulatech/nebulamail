// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Sender block of the open email: avatar, name (bidi-safe), "Verified"
// when DKIM/SPF/DMARC passed (or a warning when they failed), recipients,
// date, and every address in a details popover.

import { cleanDisplayText } from "@nebulamail/mail-render";
import { Avatar, Badge, Popover, Tooltip } from "@nebulamail/ui";
import { colors, font } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { ChevronDown, CircleCheck, Globe, TriangleAlert } from "lucide-react";
import type { ReactNode } from "react";

import { formatFullDate, formatThreadDate } from "../../lib/format";
import { useNow } from "../../lib/use-now";
import type { EmailDetail, Person } from "../../mail/api";
import type { AuthVerdict } from "../../mail/auth-results";
import { sameAddress } from "../../mail/recipients";
import { senderDomainWarning } from "../../mail/sender-domain";
import { m } from "../../paraglide/messages.js";
import { getLocale } from "../../paraglide/runtime.js";
import { AddToContacts } from "../contacts/add-to-contacts";
import { bp } from "./breakpoints.stylex";
import { NewSenderBadge, SenderNameBadge } from "./sender-warnings";
import { shared } from "./styles";

const styles = stylex.create({
  address: { color: colors.fg3, fontSize: font.sizeSm },
  details: {
    alignItems: "center",
    backgroundColor: "transparent",
    borderStyle: "none",
    color: colors.fg2,
    columnGap: "2px",
    cursor: "pointer",
    display: "inline-flex",
    fontFamily: "inherit",
    fontSize: font.sizeSm,
    padding: 0,
    textDecorationColor: colors.line2,
    textDecorationLine: "underline",
    textUnderlineOffset: "2px",
  },
  dl: {
    columnGap: "12px",
    display: "grid",
    fontSize: font.sizeSm,
    gridTemplateColumns: "auto 1fr",
    margin: 0,
    rowGap: "6px",
  },
  dt: { color: colors.fg3 },
  dd: { margin: 0, overflowWrap: "anywhere" },
  head: { alignItems: "flex-start", columnGap: "12px", display: "flex" },
  // On phones one line: "to me · date · Details", the recipients cut
  // short first; the date and "Details" stay together (the button never
  // ends up alone on a line).
  meta: {
    alignItems: "baseline",
    color: colors.fg3,
    display: { default: "block", [bp.mobile]: "flex" },
    fontSize: font.sizeSm,
    minWidth: 0,
  },
  to: {
    flexShrink: 1,
    // "to me" stays whole; a long list of names is what gets cut.
    minWidth: { default: null, [bp.mobile]: "3em" },
    overflow: { default: null, [bp.mobile]: "hidden" },
    textOverflow: { default: null, [bp.mobile]: "ellipsis" },
    whiteSpace: { default: null, [bp.mobile]: "nowrap" },
  },
  toSep: { flexShrink: 0, whiteSpace: "pre" },
  when: { display: "inline", flexShrink: 0, whiteSpace: "nowrap" },
  name: { fontWeight: 650 },
  who: {
    alignItems: "center",
    columnGap: "8px",
    display: "flex",
    flexWrap: "wrap",
    minWidth: 0,
    rowGap: "2px",
  },
  whoCol: { flexGrow: 1, minWidth: 0 },
});

const label = (person: Person): string =>
  cleanDisplayText(person.name ?? "") || cleanDisplayText(person.email);

const full = (person: Person): string => {
  const name = cleanDisplayText(person.name ?? "");
  const email = cleanDisplayText(person.email);
  return name === "" ? email : `${name} <${email}>`;
};

/** The part after "@", shown in the badge's explanation. */
const domainOf = (address: string): string =>
  cleanDisplayText(address.slice(address.lastIndexOf("@") + 1));

export const VerifiedBadge = ({
  sender,
  verdict,
}: {
  readonly sender: string | undefined;
  readonly verdict: AuthVerdict | null;
}) => {
  if (verdict === "pass" && sender !== undefined) {
    // Says which domain was checked and what that doesn't mean
    // (docs/verified-sender.md).
    return (
      <Tooltip content={m.verified_tooltip({ domain: domainOf(sender) })}>
        <span>
          <Badge icon={CircleCheck} tone="ok">
            {m.verified()}
          </Badge>
        </span>
      </Tooltip>
    );
  }
  if (verdict === "fail") {
    return (
      <Tooltip content={m.not_verified_tooltip()}>
        <span>
          <Badge icon={TriangleAlert} tone="danger">
            {m.not_verified()}
          </Badge>
        </span>
      </Tooltip>
    );
  }
  return null;
};

/**
 * An international or look-alike sender domain (`senderDomainWarning`):
 * says how it is really written. Shown even with "Verified": DMARC passes
 * for a look-alike domain the attacker owns.
 */
export const SenderDomainBadge = ({ email }: { readonly email: string }) => {
  const warning = senderDomainWarning(email);
  if (warning === null) {
    return null;
  }
  const ascii = cleanDisplayText(warning.ascii);
  return warning.kind === "lookalike" ? (
    <Tooltip content={m.sender_lookalike_tooltip({ ascii })}>
      <span>
        <Badge icon={TriangleAlert} tone="danger">
          {m.sender_lookalike()}
        </Badge>
      </span>
    </Tooltip>
  ) : (
    <Tooltip content={m.sender_idn_tooltip({ ascii })}>
      <span>
        <Badge icon={Globe} tone="warn">
          {m.sender_idn()}
        </Badge>
      </span>
    </Tooltip>
  );
};

const Row = ({
  term,
  children,
}: {
  readonly term: string;
  readonly children: ReactNode;
}) => (
  <>
    <dt {...stylex.props(styles.dt)}>{term}</dt>
    <dd {...stylex.props(styles.dd)}>{children}</dd>
  </>
);

export interface ReaderHeaderProps {
  readonly email: EmailDetail;
  readonly verdict: AuthVerdict | null;
  readonly own: readonly string[];
  readonly actions?: ReactNode;
}

export const ReaderHeader = ({
  actions,
  email,
  own,
  verdict,
}: ReaderHeaderProps) => {
  const [sender] = email.from;
  const name = sender === undefined ? m.row_no_sender() : label(sender);
  const locale = getLocale();
  const now = useNow();
  const date = formatFullDate(email.sentAt ?? email.receivedAt, locale);
  const shortDate = formatThreadDate(
    email.sentAt ?? email.receivedAt,
    now,
    locale
  );
  const recipients = [...email.to, ...email.cc]
    .map((person) =>
      own.some((address) => sameAddress(address, person.email))
        ? m.reader_me()
        : label(person)
    )
    .join(", ");

  return (
    <div {...stylex.props(styles.head)}>
      <Avatar colorKey={sender?.email ?? name} name={name} size={36} />
      <div {...stylex.props(styles.whoCol)}>
        <div {...stylex.props(styles.who)}>
          <bdi {...stylex.props(styles.name, shared.ellipsis)}>{name}</bdi>
          <VerifiedBadge sender={sender?.email} verdict={verdict} />
          {sender === undefined ? null : (
            <SenderDomainBadge email={sender.email} />
          )}
          {sender === undefined ? null : <SenderNameBadge person={sender} />}
          <NewSenderBadge email={email} own={own} />
          {sender === undefined ? null : (
            <bdi {...stylex.props(styles.address, shared.ellipsis)}>
              &lt;{cleanDisplayText(sender.email)}&gt;
            </bdi>
          )}
          {sender === undefined ? null : <AddToContacts person={sender} />}
        </div>
        <div {...stylex.props(styles.meta)}>
          {recipients === "" ? null : (
            <>
              <bdi {...stylex.props(styles.to)}>
                {m.reader_to({ names: recipients })}
              </bdi>
              <span {...stylex.props(styles.toSep)}>{" · "}</span>
            </>
          )}
          <span {...stylex.props(styles.when)}>
            <time dateTime={email.sentAt ?? email.receivedAt} title={date}>
              {shortDate}
            </time>
            {" · "}
            <Popover.Root>
              <Popover.Trigger
                render={
                  <button
                    type="button"
                    {...stylex.props(styles.details, shared.focusRing)}
                  >
                    {m.details()}
                    <ChevronDown aria-hidden size={12} />
                  </button>
                }
              />
              <Popover.Content size="md" align="start">
                <Popover.Header>
                  <Popover.Title>{m.details()}</Popover.Title>
                </Popover.Header>
                <Popover.Section>
                  <dl {...stylex.props(styles.dl)}>
                    <Row term={m.detail_from()}>
                      {email.from.map(full).join(", ")}
                    </Row>
                    {email.to.length > 0 ? (
                      <Row term={m.detail_to()}>
                        {email.to.map(full).join(", ")}
                      </Row>
                    ) : null}
                    {email.cc.length > 0 ? (
                      <Row term={m.detail_cc()}>
                        {email.cc.map(full).join(", ")}
                      </Row>
                    ) : null}
                    {email.replyTo.length > 0 ? (
                      <Row term={m.detail_reply_to()}>
                        {email.replyTo.map(full).join(", ")}
                      </Row>
                    ) : null}
                    <Row term={m.detail_date()}>{date}</Row>
                  </dl>
                </Popover.Section>
              </Popover.Content>
            </Popover.Root>
          </span>
        </div>
      </div>
      {actions}
    </div>
  );
};
