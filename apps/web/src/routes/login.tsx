// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { AppIcon, Button, Wordmark } from "@nebulamail/ui";
import {
  colors,
  font,
  radius,
  shadow,
  space,
} from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { createFileRoute, redirect } from "@tanstack/react-router";
import { LogIn, TriangleAlert } from "lucide-react";
import { useState } from "react";
import { z } from "zod";

import { beginLogin, completeLogin, getAuthState } from "../lib/auth";
import { m } from "../paraglide/messages.js";

const Search = z.object({
  error: z
    .enum(["denied", "expired", "invalid", "unavailable"])
    .optional()
    .catch(undefined),
  returnTo: z.string().optional().catch(undefined),
});

const ERRORS = {
  denied: m.login_error_denied,
  expired: m.login_error_expired,
  invalid: m.login_error_invalid,
  unavailable: m.login_error_unavailable,
} as const;

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
  error: {
    alignItems: "flex-start",
    backgroundColor: colors.dangerSoft,
    borderRadius: radius.md,
    color: colors.danger,
    columnGap: space.sm,
    display: "flex",
    fontSize: font.sizeSm,
    margin: 0,
    paddingBlock: space.sm,
    paddingInline: space.md,
    textAlign: "start",
  },
  errorIcon: { flexShrink: 0, marginTop: "2px" },
  intro: { color: colors.fg2, fontSize: font.sizeMd, margin: 0 },
  main: {
    alignItems: "center",
    backgroundColor: colors.canvas,
    boxSizing: "border-box",
    display: "flex",
    isolation: "isolate",
    justifyContent: "center",
    minHeight: "100dvh",
    overflow: "hidden",
    padding: space.lg,
    position: "relative",
  },
  note: { color: colors.fg3, fontSize: font.sizeSm, margin: 0 },
  tagline: {
    color: colors.fg,
    fontSize: font.sizeXl,
    fontWeight: font.weightBold,
    letterSpacing: "-0.03em",
    lineHeight: 1.2,
    margin: 0,
  },
});

/**
 * Static build: the app itself starts OAuth (PKCE and
 * `state` are made in the browser), then leaves for Stalwart's login form.
 */
const StaticLoginButton = ({
  returnTo,
}: {
  readonly returnTo: string | undefined;
}) => {
  const [busy, setBusy] = useState(false);
  const navigate = Route.useNavigate();
  return (
    <Button
      disabled={busy}
      fullWidth
      icon={LogIn}
      onClick={() => {
        setBusy(true);
        beginLogin(returnTo).catch(() => {
          setBusy(false);
          void navigate({
            search: { error: "unavailable", returnTo },
            to: "/login",
          });
        });
      }}
      size="lg"
      variant="primary"
    >
      {m.login_button()}
    </Button>
  );
};

const LoginPage = () => {
  const { error, returnTo } = Route.useSearch();
  return (
    <main {...stylex.props(styles.main)}>
      <div {...stylex.props(styles.card)}>
        <div {...stylex.props(styles.brand)}>
          <AppIcon label={null} size={56} />
          <Wordmark size="lg" />
        </div>
        <h1 {...stylex.props(styles.tagline)}>{m.login_tagline()}</h1>
        <p {...stylex.props(styles.intro)}>{m.login_intro()}</p>
        {error === undefined ? null : (
          <p role="alert" {...stylex.props(styles.error)}>
            <TriangleAlert
              aria-hidden
              size={16}
              {...stylex.props(styles.errorIcon)}
            />
            {ERRORS[error]()}
          </p>
        )}
        {import.meta.env.NEBULA_STATIC ? (
          <StaticLoginButton returnTo={returnTo} />
        ) : (
          // A plain GET form: the login is a full-page redirect to the provider.
          <form action="/auth/login" method="get">
            {returnTo === undefined ? null : (
              <input name="returnTo" type="hidden" value={returnTo} />
            )}
            <Button
              fullWidth
              icon={LogIn}
              size="lg"
              type="submit"
              variant="primary"
            >
              {m.login_button()}
            </Button>
          </form>
        )}
        <p {...stylex.props(styles.note)}>{m.login_privacy_note()}</p>
      </div>
    </main>
  );
};

export const Route = createFileRoute("/login")({
  beforeLoad: async () => {
    // Static build: Stalwart sends the user back here with the code.
    if (import.meta.env.NEBULA_STATIC && typeof document !== "undefined") {
      const result = await completeLogin(globalThis.location.search);
      if (result.kind === "done") {
        throw redirect({ href: result.returnTo });
      }
      if (result.kind === "failed") {
        throw redirect({ search: { error: result.error }, to: "/login" });
      }
    }
    const { authenticated } = await getAuthState();
    if (authenticated) {
      throw redirect({ to: "/" });
    }
  },
  component: LoginPage,
  head: () => ({ meta: [{ title: `${m.login_button()} · ${m.app_name()}` }] }),
  validateSearch: Search,
});
