// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Text that comes inside events is untrusted: titles,
// places and names are shown as text without bidi controls, inside <bdi>;
// HTML descriptions only through @nebulamail/mail-render's pipeline, in the
// same sandboxed iframe as emails, with remote content blocked; links open
// only after the same confirmation as links in mail.

import type { FrameLink } from "@nebulamail/mail-render";
import {
  DEFAULT_PRIVACY,
  cleanDisplayText,
  linkifySegments,
  renderEmail,
  stripBidiControls,
  stripInvisible,
} from "@nebulamail/mail-render";
import { ThemeScope } from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import type { MouseEvent } from "react";

import { readCspNonce } from "../../lib/csp-nonce";
import { m } from "../../paraglide/messages.js";
import { EmailFrame } from "../email-frame";
import { useLinkHandler } from "../mail/link-dialog";

const styles = stylex.create({
  link: {
    color: colors.accText,
    overflowWrap: "anywhere",
    textDecoration: { ":hover": "underline", default: "none" },
  },
  paper: {
    backgroundColor: colors.paper,
    borderColor: colors.line,
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: "1px",
    overflow: "hidden",
    paddingBlock: "8px",
    paddingInline: "10px",
  },
  text: {
    color: colors.fg,
    fontSize: font.sizeMd,
    lineHeight: font.lineHeight,
    margin: 0,
    overflowWrap: "anywhere",
    whiteSpace: "pre-wrap",
  },
});

/** One line of untrusted text: no bidi tricks, isolated direction. */
export const SafeText = ({ text }: { readonly text: string }) => (
  <bdi>{cleanDisplayText(text)}</bdi>
);

/** Display form of a title ("(No title)" when empty). */
export const titleText = (title: string): string => {
  const clean = cleanDisplayText(title);
  return clean === "" ? m.cal_no_title() : clean;
};

/** Multi-line untrusted text, without bidi or invisible characters. */
const cleanMultiline = (text: string): string =>
  stripInvisible(stripBidiControls(text));

/** Opens a link from an event like a link in an email (confirmation). */
export const useEventLinks = () => {
  const links = useLinkHandler();
  const open = (href: string, text: string, element: Element): void => {
    const link: FrameLink = { element, href, text };
    links.onLink(link);
  };
  return {
    handleFrameLink: links.onLink,
    handleOpen: open,
  };
};

const PlainDescription = ({
  onOpen,
  text,
}: {
  readonly text: string;
  readonly onOpen: (href: string, text: string, element: Element) => void;
}) => (
  <p dir="auto" {...stylex.props(styles.text)}>
    {linkifySegments(cleanMultiline(text)).map((segment, index) =>
      segment.href === undefined ? (
        // oxlint-disable-next-line react/no-array-index-key -- segments have no id and never reorder.
        <span key={index}>{segment.text}</span>
      ) : (
        <a
          // oxlint-disable-next-line react/no-array-index-key -- segments have no id and never reorder.
          key={index}
          href={segment.href}
          onClick={(event: MouseEvent<HTMLAnchorElement>) => {
            event.preventDefault();
            onOpen(event.currentTarget.href, segment.text, event.currentTarget);
          }}
          rel="noopener noreferrer"
          {...stylex.props(styles.link)}
        >
          {segment.text}
        </a>
      )
    )}
  </p>
);

/** An event's description: sanitized HTML in a sandbox, or linkified text. */
export const EventDescription = ({
  html,
  text,
  title,
}: {
  readonly text: string;
  readonly html: boolean;
  /** Accessible name of the iframe. */
  readonly title: string;
}) => {
  const links = useEventLinks();
  if (!html) {
    return <PlainDescription onOpen={links.handleOpen} text={text} />;
  }
  const nonce = typeof document === "undefined" ? undefined : readCspNonce();
  const result =
    typeof document === "undefined"
      ? null
      : renderEmail({ html: text }, DEFAULT_PRIVACY, {
          images: { kind: "none" },
          userApproved: false,
          ...(nonce === undefined ? {} : { styleNonce: nonce }),
        });
  if (result === null) {
    return null;
  }
  return (
    <div {...stylex.props(styles.paper)}>
      <ThemeScope mode="light">
        <EmailFrame
          onLink={links.handleFrameLink}
          rendered={result.frame}
          title={title}
        />
      </ThemeScope>
    </div>
  );
};
