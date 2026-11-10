// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Full-page states outside the mail screens (page not found, unexpected
// error): the same card as the sign-in page. Inside the mail shell (an
// error in the list, contacts or settings) the card fills and centres in
// the content area instead of taking the whole window.

import { AppIcon, Button, Wordmark } from "@nebulamail/ui";
import {
  colors,
  font,
  radius,
  shadow,
  space,
} from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { Link } from "@tanstack/react-router";
import type { ErrorComponentProps } from "@tanstack/react-router";
import { Inbox, RotateCw } from "lucide-react";
import { createContext, use } from "react";
import type { ReactNode } from "react";

import { m } from "../paraglide/messages.js";

const styles = stylex.create({
  brand: {
    alignItems: "center",
    display: "flex",
    flexDirection: "column",
    rowGap: space.md,
  },
  card: {
    backgroundColor: colors.bg,
    borderColor: colors.line,
    borderRadius: radius.xl,
    borderStyle: "solid",
    borderWidth: "1px",
    boxShadow: shadow.sh2,
    boxSizing: "border-box",
    display: "flex",
    flexDirection: "column",
    maxWidth: "380px",
    paddingBlock: space.xxl,
    paddingInline: space.xl,
    rowGap: space.lg,
    textAlign: "center",
    width: "100%",
  },
  main: {
    alignItems: "center",
    backgroundColor: colors.canvas,
    boxSizing: "border-box",
    display: "flex",
    justifyContent: "center",
    minHeight: "100dvh",
    padding: space.lg,
  },
  // In the shell's content area (a flex row): fill it and centre the card.
  pane: {
    alignItems: "center",
    display: "flex",
    flexGrow: 1,
    justifyContent: "center",
    minHeight: 0,
    minWidth: 0,
    overflowY: "auto",
    padding: space.lg,
  },
  text: { color: colors.fg2, fontSize: font.sizeMd, margin: 0 },
  title: {
    color: colors.fg,
    fontSize: font.sizeXl,
    fontWeight: font.weightBold,
    letterSpacing: font.trackingTitle,
    lineHeight: font.lineHeightTight,
    margin: 0,
  },
});

/** True under MailShell: status pages render as a pane, not a page. */
export const InShellContext = createContext(false);

const StatusPage = ({
  title,
  text,
  action,
}: {
  readonly title: string;
  readonly text: string;
  readonly action: ReactNode;
}) => {
  const card = (
    <div {...stylex.props(styles.card)}>
      <div {...stylex.props(styles.brand)}>
        <AppIcon label={null} size={56} />
        <Wordmark size="lg" />
      </div>
      <h1 {...stylex.props(styles.title)}>{title}</h1>
      <p {...stylex.props(styles.text)}>{text}</p>
      {action}
    </div>
  );
  // The shell already has a <main>: a plain section there.
  return use(InShellContext) ? (
    <section {...stylex.props(styles.pane)}>{card}</section>
  ) : (
    <main {...stylex.props(styles.main)}>{card}</main>
  );
};

/** The page for an unknown address. */
export const NotFoundPage = () => (
  <StatusPage
    action={
      // A real link (<a href>): it works before the page is interactive.
      <Button
        fullWidth
        icon={Inbox}
        nativeButton={false}
        render={<Link params={{ mailbox: "inbox" }} to="/mail/$mailbox" />}
        size="lg"
        variant="primary"
      >
        {m.status_go_inbox()}
      </Button>
    }
    text={m.status_not_found_text()}
    title={m.status_not_found_title()}
  />
);

/**
 * An error no screen handled. The error itself is not shown: it can carry
 * server details, and the browser console already has it.
 */
export const ErrorPage = (_props: ErrorComponentProps) => (
  <StatusPage
    action={
      <Button
        fullWidth
        icon={RotateCw}
        onClick={() => {
          globalThis.location.reload();
        }}
        size="lg"
        variant="primary"
      >
        {m.status_reload()}
      </Button>
    }
    text={m.status_error_text()}
    title={m.status_error_title()}
  />
);
