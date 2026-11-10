// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Opening links from an email: only the URL
// the browser resolved, only http(s) and mailto, never into the app. The
// dialog shows the real destination host big, the full address, and why a
// link looks suspicious. mailto opens our composer (to/cc/bcc/subject/body
// only).

import type {
  FrameLink,
  LinkAnalysis,
  LinkWarning,
} from "@nebulamail/mail-render";
import {
  DEFAULT_PRIVACY,
  analyzeLink,
  needsConfirmation,
  parseMailto,
  resolveLinkForOpening,
} from "@nebulamail/mail-render";
import { AlertDialog, useToast } from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { TriangleAlert } from "lucide-react";
import { createContext, use, useState } from "react";
import type { ReactNode } from "react";

import { parseRecipient } from "../../mail/recipients";
import type { ComposeInit } from "../../mail/reply";
import { m } from "../../paraglide/messages.js";
import { useComposer } from "./compose-context";

const styles = stylex.create({
  block: { display: "block" },
  caption: {
    color: colors.fg3,
    display: "block",
    fontSize: font.sizeSm,
    marginTop: "12px",
  },
  host: {
    color: colors.fg,
    display: "block",
    fontSize: "18px",
    fontWeight: font.weightBold,
    letterSpacing: "-0.01em",
    marginTop: "2px",
    overflowWrap: "anywhere",
  },
  url: {
    backgroundColor: colors.hover,
    borderRadius: radius.md,
    color: colors.fg2,
    display: "block",
    fontFamily: font.mono,
    fontSize: "12px",
    marginTop: "4px",
    maxHeight: "96px",
    overflowWrap: "anywhere",
    overflowY: "auto",
    paddingBlock: "6px",
    paddingInline: "8px",
  },
  warning: {
    alignItems: "flex-start",
    backgroundColor: colors.dangerSoft,
    borderRadius: radius.md,
    color: colors.danger,
    columnGap: "8px",
    display: "flex",
    fontSize: font.sizeSm,
    marginTop: "8px",
    paddingBlock: "8px",
    paddingInline: "10px",
  },
  warnings: { display: "block", marginTop: "12px" },
  warningIcon: { flexShrink: 0, marginTop: "2px" },
});

const warningText = (warning: LinkWarning, analysis: LinkAnalysis): string => {
  switch (warning) {
    case "text-domain-mismatch": {
      return m.link_warn_mismatch({ domains: analysis.textDomains.join(", ") });
    }
    case "punycode": {
      return m.link_warn_punycode();
    }
    case "userinfo": {
      return m.link_warn_userinfo();
    }
    case "ip-address": {
      return m.link_warn_ip();
    }
    case "insecure": {
      return m.link_warn_insecure();
    }
    default: {
      return warning satisfies never;
    }
  }
};

/** Opens an http(s) URL in a new tab with no opener and no referrer. */
export const openExternal = (url: string): void => {
  // noopener + noreferrer: the site gets no handle on the app and no Referer.
  globalThis.open(url, "_blank", "noopener,noreferrer");
};

const recipients = (list: readonly string[]) =>
  list.flatMap((entry) => {
    const parsed = parseRecipient(entry);
    return parsed === null ? [] : [parsed];
  });

/** A mailto: URL → a new message (to, cc, bcc, subject, body only). */
export const composeFromMailto = (url: string): ComposeInit | null => {
  const fields = parseMailto(url);
  if (fields === null) {
    return null;
  }
  return {
    bcc: recipients(fields.bcc),
    cc: recipients(fields.cc),
    subject: fields.subject,
    text: fields.body,
    to: recipients(fields.to),
  };
};

interface Pending {
  readonly url: string;
  readonly analysis: LinkAnalysis | null;
}

/** Handles link clicks from an EmailFrame; render `dialog` once. */
type OnLink = (link: FrameLink) => void;

const LinkContext = createContext<OnLink | null>(null);

/**
 * Opens links from emails, events and tasks: blocked, to the composer
 * (mailto:), or after the confirmation when it is needed. One dialog for
 * the whole app (state lifted here), so no page has to remember to render
 * it. Inside ComposeProvider (mailto: opens the composer).
 */
export const LinkConfirmProvider = ({
  children,
}: {
  readonly children: ReactNode;
}) => {
  const [pending, setPending] = useState<Pending | null>(null);
  const toast = useToast();
  const composer = useComposer();

  const onLink = (link: FrameLink): void => {
    const resolved = resolveLinkForOpening(link.href, undefined, {
      appOrigin: globalThis.location.origin,
    });
    if (resolved === null) {
      toast.show({ title: m.link_blocked(), tone: "danger" });
      return;
    }
    if (resolved.protocol === "mailto:") {
      const init = composeFromMailto(resolved.url);
      if (init === null) {
        toast.show({ title: m.link_blocked(), tone: "danger" });
      } else {
        composer.open(init);
      }
      return;
    }
    const analysis = analyzeLink(resolved.url, link.text);
    if (needsConfirmation(DEFAULT_PRIVACY.linkConfirmation, analysis)) {
      setPending({ analysis, url: resolved.url });
    } else {
      openExternal(resolved.url);
    }
  };

  const analysis = pending?.analysis ?? null;
  const dialog =
    pending === null ? null : (
      <AlertDialog
        cancelLabel={m.cancel()}
        confirmLabel={m.link_open()}
        description={
          <span {...stylex.props(styles.block)}>
            <span {...stylex.props(styles.caption)}>{m.link_opens()}</span>
            <span {...stylex.props(styles.host)}>
              {analysis?.host ?? pending.url}
            </span>
            <span {...stylex.props(styles.caption)}>
              {m.link_full_address()}
            </span>
            <span {...stylex.props(styles.url)}>{pending.url}</span>
            {analysis !== null && analysis.warnings.length > 0 ? (
              <span {...stylex.props(styles.warnings)}>
                {analysis.warnings.map((warning) => (
                  <span key={warning} {...stylex.props(styles.warning)}>
                    <TriangleAlert
                      aria-hidden
                      size={14}
                      {...stylex.props(styles.warningIcon)}
                    />
                    {warningText(warning, analysis)}
                  </span>
                ))}
              </span>
            ) : null}
          </span>
        }
        onConfirm={() => {
          openExternal(pending.url);
        }}
        onOpenChange={(open) => {
          if (!open) {
            setPending(null);
          }
        }}
        open
        title={m.link_title()}
        tone={
          analysis === null ||
          analysis.warnings.some((warning) => warning !== "insecure")
            ? "danger"
            : "primary"
        }
      />
    );

  return (
    <LinkContext
      // oxlint-disable-next-line react/jsx-no-constructed-context-values -- the React Compiler memoizes onLink.
      value={onLink}
    >
      {children}
      {dialog}
    </LinkContext>
  );
};

/** What to do when a link is clicked (see LinkConfirmProvider). */
export const useLinkHandler = (): { readonly onLink: OnLink } => {
  const onLink = use(LinkContext);
  if (onLink === null) {
    throw new Error("useLinkHandler must be used inside LinkConfirmProvider");
  }
  return { onLink };
};
