// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// One open email of the reader: sender with its own verification, its own
// privacy bar and images, the sandboxed body (EmailFrame), attachments and
// its print frame.

import type { EmailId } from "@nebulamail/jmap-client";
import type { RenderResult } from "@nebulamail/mail-render";
import { cleanDisplayText } from "@nebulamail/mail-render";
import { Spinner, ThemeScope } from "@nebulamail/ui";
import { colors, motion, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { Suspense, lazy, useEffect, useState } from "react";
import type { ReactNode } from "react";

import { invitationPart } from "../../calendar/invitation-part";
import { formatFullDate } from "../../lib/format";
import { loadWhenOnline } from "../../lib/load-when-online";
import { useDarkScheme } from "../../lib/use-dark-scheme";
import type { EmailDetail } from "../../mail/api";
import { shownVerdict } from "../../mail/auth-results";
import { usePreloadFull } from "../../mail/preload-status";
import { sameAddress } from "../../mail/recipients";
import { DIRECT_FRAME_PAGE } from "../../mail/remote-images";
import { useRenderedEmail } from "../../mail/rendering";
import { useVerifiedBadge } from "../../mail/use-verified-badge";
import { m } from "../../paraglide/messages.js";
import { getLocale } from "../../paraglide/runtime.js";
import { EmailFrame } from "../email-frame";
import type { PrintHeader } from "../print-frame";
import { PrintFrame } from "../print-frame";
import { AttachmentList } from "./attachment-list";
import { useLinkHandler } from "./link-dialog";
import { useMailUi } from "./mail-ui";
import { PrivacyBar } from "./privacy-bar";
import { ReadReceiptBanner } from "./read-receipt-banner";
import { ReaderHeader } from "./reader-header";
import { readerStyles } from "./reader-styles";
import { SentStatus } from "./scheduled-mark";
import { useRemoteImages } from "./use-remote-images";

/** How the body is shown (the reader's "View as text / HTML"). */
export type ReaderView = "html" | "text";

const appear = stylex.keyframes({
  from: { opacity: 0 },
  to: { opacity: 1 },
});

// Loaded only for an email with a calendar part (invitation-cards.tsx).
const InvitationCards = lazy(async () => {
  const { InvitationCards: Cards } = await loadWhenOnline(
    async () => await import("./invitation-cards")
  );
  return { default: Cards };
});

const styles = stylex.create({
  // The loader only shows if loading takes a moment: quick loads show none.
  delayed: {
    animationDelay: "250ms",
    animationDuration: motion.normal,
    animationFillMode: "both",
    animationName: appear,
  },
  // One container: the card. The sheet has no frame of its own (white on
  // the white card in light mode; a rounded sheet on the dark card).
  paper: {
    borderRadius: radius.lg,
    marginTop: "14px",
    overflow: "hidden",
    transitionDuration: motion.fast,
    transitionProperty: "opacity",
  },
  // Plain text sits on the card in the theme's colours, aligned with the
  // header above it.
  paperText: { borderRadius: 0, marginTop: "12px" },
  // Until the email has loaded: no empty white sheet flashing in dark mode.
  paperPending: { opacity: 0 },
  sheet: {
    backgroundColor: colors.paper,
    color: colors.paperFg,
    paddingBlock: "8px",
    paddingInline: "10px",
  },
});

const fallbackNotice = (result: RenderResult): string | null => {
  const reason = result.fallbackReason;
  if (reason === undefined || reason === "no-html") {
    return null;
  }
  return reason === "integrity-check-failed"
    ? m.fallback_integrity()
    : m.fallback_limits();
};

interface BodyProps {
  readonly email: EmailDetail;
  readonly own: readonly string[];
  /** Buttons at the end of the sender line (reply, more, collapse). */
  readonly headerActions: ReactNode;
  readonly view: ReaderView;
  /**
   * Print what is shown (same render, same allowed images): the print run
   * (a new frame for each), or null.
   */
  readonly printing: number | null;
  readonly onPrinted: (run: number) => void;
}

const people = (list: EmailDetail["from"]): string =>
  list
    .map((person) => {
      const name = cleanDisplayText(person.name ?? "");
      return name === "" ? person.email : `${name} <${person.email}>`;
    })
    .join(", ");

const printHeader = (email: EmailDetail): PrintHeader => ({
  rows: [
    [m.detail_from(), people(email.from)],
    [m.detail_to(), people(email.to)],
    [m.detail_cc(), people(email.cc)],
    [m.detail_date(), formatFullDate(email.receivedAt, getLocale())],
  ],
  subject: cleanDisplayText(email.subject) || m.row_no_subject(),
});

/**
 * The body's sheet. An HTML email keeps a light sheet in dark mode: its own
 * colours assume white, and adapting them isn't done yet.
 * Plain text has no colours of its own: it takes the theme's, on the card.
 */
const EmailSheet = ({
  children,
  plain,
  ready,
}: {
  readonly children: ReactNode;
  readonly plain: boolean;
  /** Hidden until loaded: no empty white sheet flashing in dark mode. */
  readonly ready: boolean;
}) => (
  <div
    {...stylex.props(
      styles.paper,
      plain ? styles.paperText : null,
      ready ? null : styles.paperPending
    )}
  >
    {plain ? (
      children
    ) : (
      <ThemeScope mode="light">
        <div {...stylex.props(styles.sheet)}>{children}</div>
      </ThemeScope>
    )}
  </div>
);

/**
 * The print frame (same render, same allowed images). Paper is white:
 * plain text shown in dark colours prints in light ones, rendered again
 * only then.
 */
const PrintCopy = ({
  email,
  onDone,
  page,
  remote,
  result,
  scheme,
  settings,
  title,
}: {
  readonly email: EmailDetail;
  readonly onDone: () => void;
  readonly page: string | undefined;
  readonly remote: ReturnType<typeof useRemoteImages>["remote"];
  readonly result: RenderResult;
  readonly scheme: "light" | "dark";
  readonly settings: ReturnType<typeof useRemoteImages>["settings"];
  readonly title: string;
}) => {
  const light = useRenderedEmail(
    scheme === "dark" && result.view === "text" ? email : null,
    { remote, settings, view: "text" }
  );
  return (
    <PrintFrame
      header={printHeader(email)}
      onDone={onDone}
      page={page}
      rendered={(light ?? result).frame}
      title={title}
    />
  );
};

/**
 * One email, open: sender with its own "Verified" check, its own privacy
 * bar and image permission, and its body through mail-render in a
 * sandboxed iframe. Every card of a conversation is separate.
 */
export const EmailBody = ({
  email,
  headerActions,
  onPrinted,
  own,
  printing,
  view,
}: BodyProps) => {
  const ui = useMailUi();
  const links = useLinkHandler();
  const images = useRemoteImages(email);
  // Only asked when "Show images" goes through the proxy (never static).
  const preloadFull = usePreloadFull(
    !import.meta.env.NEBULA_STATIC &&
      images.access.kind === "offered" &&
      images.access.from === "proxy"
  );
  const scheme = useDarkScheme() ? "dark" : "light";
  const result = useRenderedEmail(email, {
    remote: images.remote,
    scheme,
    // "never" also stops mail-render itself from restoring any image.
    settings: images.settings,
    view,
  });
  const badge = useVerifiedBadge();
  const verdict = shownVerdict(email.senderVerdict, badge.enabled);
  const blocked = result?.report.trackersBlocked ?? 0;
  const { reportBlocked } = ui;
  // The email whose body has loaded once; later reloads of the same email
  // (images shown, HTML/text) don't hide it again.
  const [readyId, setReadyId] = useState<EmailId | null>(null);

  useEffect(() => {
    reportBlocked(email.id, blocked);
  }, [email.id, blocked, reportBlocked]);

  // "Always" or a trusted sender: the images load without a click, by the
  // route of the settings, once the render knows which ones there are.
  const remoteUrls = result?.remoteImages;
  const { showAtOnce } = images;
  useEffect(() => {
    if (remoteUrls !== undefined) {
      showAtOnce(remoteUrls);
    }
  }, [remoteUrls, showAtOnce]);

  if (result === null) {
    return (
      <div {...stylex.props(readerStyles.card, readerStyles.center)}>
        <span {...stylex.props(styles.delayed)}>
          <Spinner label={m.reader_loading()} />
        </span>
      </div>
    );
  }
  const notice = fallbackNotice(result);
  const subject = cleanDisplayText(email.subject) || m.row_no_subject();
  // Ours (sent, a draft): delivery status, never a receipt request.
  const ours =
    email.draft ||
    email.from.some((person) =>
      own.some((address) => sameAddress(address, person.email))
    );
  const handleLink = links.onLink;
  // Images straight from the sender: the app's CSP doesn't allow them.
  const framePage =
    images.remote?.kind === "direct"
      ? (DIRECT_FRAME_PAGE ?? undefined)
      : undefined;
  const plain = result.view === "text";
  const frame = (
    <EmailFrame
      onLink={handleLink}
      page={framePage}
      onReady={() => {
        setReadyId(email.id);
      }}
      rendered={result.frame}
      scheme={plain ? scheme : "light"}
      title={subject}
    />
  );

  return (
    <article {...stylex.props(readerStyles.card)}>
      <ReaderHeader
        actions={headerActions}
        email={email}
        own={own}
        verdict={verdict}
      />
      {ours ? <SentStatus emailId={email.id} /> : null}
      <PrivacyBar
        auth={verdict}
        images={images.access}
        onShowImages={() => {
          void images.show(result.remoteImages);
        }}
        onTrustSender={() => {
          images.trustSender(result.remoteImages);
        }}
        preloadFull={preloadFull}
        result={result}
      />
      <ReadReceiptBanner email={email} ours={ours} />
      {notice === null ? null : (
        <p role="note" {...stylex.props(readerStyles.notice)}>
          {notice}
        </p>
      )}
      {email.truncated ? (
        <p role="note" {...stylex.props(readerStyles.notice)}>
          {m.truncated()}
        </p>
      ) : null}
      {invitationPart(email.attachments) === null ? null : (
        <Suspense fallback={null}>
          <InvitationCards email={email} />
        </Suspense>
      )}
      <EmailSheet plain={plain} ready={readyId === email.id}>
        {frame}
      </EmailSheet>
      <AttachmentList attachments={email.attachments} />
      {printing === null ? null : (
        <PrintCopy
          email={email}
          key={printing}
          onDone={() => {
            onPrinted(printing);
          }}
          page={framePage}
          remote={images.remote}
          result={result}
          scheme={scheme}
          settings={images.settings}
          title={subject}
        />
      )}
    </article>
  );
};
