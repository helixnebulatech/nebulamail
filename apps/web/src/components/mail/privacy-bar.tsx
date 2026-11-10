// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Visible privacy (Helix principle 1): one line per
// email with what was blocked, "Show images" (through the proxy or from
// the sender, as the settings say) and the full report in a
// popover. With a full preload quota (H15, mail/preload-status.ts), a line
// under the bar says showing the images may tell the sender.

import type { PrivacyReport, RenderResult } from "@nebulamail/mail-render";
import { SPY_PIXEL_SERVICE, cleanDisplayText } from "@nebulamail/mail-render";
import { Button, Popover, PrivacyShield, ShieldGlyph } from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import {
  Ban,
  ChevronDown,
  CircleCheck,
  Image,
  Link2,
  ShieldAlert,
  TriangleAlert,
} from "lucide-react";
import { useState } from "react";

import type { AuthVerdict } from "../../mail/auth-results";
import type { PreloadFull } from "../../mail/preload-status";
import { m } from "../../paraglide/messages.js";
import { getLocale } from "../../paraglide/runtime.js";
import { bp } from "./breakpoints.stylex";
import { shared } from "./styles";

const styles = stylex.create({
  bar: {
    alignItems: "center",
    backgroundColor: colors.accSoft,
    borderRadius: radius.lg,
    color: colors.fg,
    columnGap: "10px",
    display: "flex",
    flexWrap: { default: "nowrap", [bp.mobile]: "wrap" },
    fontSize: font.sizeMd,
    marginTop: "12px",
    paddingBlock: "8px",
    paddingInlineEnd: "8px",
    paddingInlineStart: "12px",
    rowGap: "6px",
  },
  buttons: {
    alignItems: "center",
    columnGap: "4px",
    display: "flex",
    flexShrink: 0,
  },
  code: {
    backgroundColor: colors.hover,
    borderRadius: radius.xs,
    fontFamily: font.mono,
    fontSize: font.sizeXs,
    paddingBlock: "1px",
    paddingInline: "5px",
  },
  strike: {
    textDecorationColor: colors.accText,
    textDecorationLine: "line-through",
  },
  chips: { display: "flex", flexWrap: "wrap", gap: "4px", marginTop: "6px" },
  entry: {
    alignItems: "baseline",
    columnGap: "8px",
    display: "flex",
    fontSize: font.sizeSm,
    justifyContent: "space-between",
    paddingBlock: "3px",
  },
  entryService: { color: colors.fg3, flexShrink: 0 },
  // Nothing to act on: a quiet line in its own soft box.
  quiet: {
    alignItems: "center",
    backgroundColor: colors.canvas,
    borderRadius: radius.md,
    color: colors.fg2,
    columnGap: "8px",
    display: "flex",
    fontSize: font.sizeSm,
    marginTop: "12px",
    paddingBlock: "5px",
    paddingInlineEnd: "6px",
    paddingInlineStart: "10px",
  },
  sectionBody: {
    color: colors.fg2,
    fontSize: font.sizeSm,
    lineHeight: 1.45,
    margin: 0,
    marginInlineStart: "24px",
    marginTop: "4px",
  },
  sectionTitle: {
    alignItems: "center",
    columnGap: "8px",
    display: "flex",
    fontSize: font.sizeMd,
    fontWeight: font.weightSemibold,
    margin: 0,
  },
  sectionIcon: { color: colors.accText, flexShrink: 0 },
  okIcon: { color: colors.ok, flexShrink: 0 },
  warnIcon: { color: colors.danger, flexShrink: 0 },
  list: {
    listStyle: "none",
    margin: 0,
    marginInlineStart: "24px",
    marginTop: "6px",
    padding: 0,
  },
  strong: { color: colors.accText, fontWeight: 650 },
  text: { flexGrow: 1, minWidth: 0 },
  notice: {
    color: colors.fg2,
    fontSize: font.sizeSm,
    margin: 0,
    marginTop: "6px",
  },
});

const hostOf = (url: string | undefined): string => {
  if (url === undefined) {
    return "";
  }
  try {
    return new URL(url).hostname;
  } catch {
    return "";
  }
};

/** Query parameters tidy-url removed from a link (utm_source, fbclid…). */
const removedParams = (report: PrivacyReport): string[] => {
  const names = new Set<string>();
  for (const link of report.linksCleaned) {
    try {
      const before = new URL(link.original).searchParams;
      const after = new URL(link.cleaned).searchParams;
      for (const name of before.keys()) {
        if (!after.has(name)) {
          names.add(cleanDisplayText(name));
        }
      }
    } catch {
      // Not a URL we can compare; the count still says it was cleaned.
    }
  }
  return [...names].slice(0, 8);
};

const unsafeCount = (report: PrivacyReport): number =>
  Object.values(report.unsafeImagesRemoved).reduce(
    (sum, count) => sum + (count ?? 0),
    0
  ) +
  report.unsafeLinksRemoved +
  report.elementsRemoved;

const AUTH_TEXT = {
  fail: m.privacy_report_auth_fail,
  pass: m.privacy_report_auth_ok,
  unknown: m.privacy_report_auth_unknown,
} as const;

const Report = ({
  auth,
  images,
  report,
}: {
  readonly auth: AuthVerdict | null;
  /** What was done with the remote images. */
  readonly images: string;
  readonly report: PrivacyReport;
}) => {
  const params = removedParams(report);
  const unsafe = unsafeCount(report);
  return (
    <>
      <Popover.Section>
        <p {...stylex.props(styles.sectionTitle)}>
          <Ban aria-hidden size={15} {...stylex.props(styles.sectionIcon)} />
          {report.trackersBlocked > 0
            ? m.privacy_trackers({ count: report.trackersBlocked })
            : m.privacy_report_no_trackers()}
        </p>
        {report.trackers.length > 0 ? (
          <ul {...stylex.props(styles.list)}>
            {report.trackers.map((tracker) => (
              <li key={tracker.service} {...stylex.props(styles.entry)}>
                <code
                  {...stylex.props(styles.code, styles.strike, shared.ellipsis)}
                >
                  {cleanDisplayText(hostOf(tracker.urls[0]))}
                </code>
                <span {...stylex.props(styles.entryService)}>
                  {tracker.service === SPY_PIXEL_SERVICE
                    ? m.spy_pixel()
                    : cleanDisplayText(tracker.service)}
                  {" · "}
                  {m.tracker_hits({ count: tracker.count })}
                </span>
              </li>
            ))}
          </ul>
        ) : null}
      </Popover.Section>
      {report.linksCleaned.length > 0 ? (
        <Popover.Section>
          <p {...stylex.props(styles.sectionTitle)}>
            <Link2
              aria-hidden
              size={15}
              {...stylex.props(styles.sectionIcon)}
            />
            {m.privacy_report_links({ count: report.linksCleaned.length })}
          </p>
          {params.length > 0 ? (
            <span {...stylex.props(styles.chips, styles.list)}>
              {params.map((name) => (
                <code key={name} {...stylex.props(styles.code)}>
                  {name}
                </code>
              ))}
            </span>
          ) : null}
          <p {...stylex.props(styles.sectionBody)}>
            {m.privacy_report_links_body()}
          </p>
        </Popover.Section>
      ) : null}
      {report.remoteImagesBlocked > 0 || report.remoteImagesLoaded > 0 ? (
        <Popover.Section>
          <p {...stylex.props(styles.sectionTitle)}>
            <Image
              aria-hidden
              size={15}
              {...stylex.props(styles.sectionIcon)}
            />
            {report.remoteImagesLoaded > 0
              ? m.privacy_report_images_loaded({
                  count: report.remoteImagesLoaded,
                })
              : m.privacy_report_images({ count: report.remoteImagesBlocked })}
          </p>
          <p {...stylex.props(styles.sectionBody)}>{images}</p>
        </Popover.Section>
      ) : null}
      {unsafe > 0 ? (
        <Popover.Section>
          <p {...stylex.props(styles.sectionTitle)}>
            <ShieldAlert
              aria-hidden
              size={15}
              {...stylex.props(styles.sectionIcon)}
            />
            {m.privacy_report_unsafe({ count: unsafe })}
          </p>
        </Popover.Section>
      ) : null}
      <Popover.Section>
        <p {...stylex.props(styles.sectionTitle)}>
          {auth === "pass" ? (
            <CircleCheck
              aria-hidden
              size={15}
              {...stylex.props(styles.okIcon)}
            />
          ) : (
            <TriangleAlert
              aria-hidden
              size={15}
              {...stylex.props(
                auth === "fail" ? styles.warnIcon : styles.entryService
              )}
            />
          )}
          {AUTH_TEXT[auth ?? "unknown"]()}
        </p>
      </Popover.Section>
    </>
  );
};

/** Where "Show images" loads them from. */
export type ImagesRoute = "proxy" | "sender";

/**
 * What can be done with one email's remote images,
 * as one state instead of loose flags, so a combination the settings can't
 * produce (blocked and loaded at once…) can't be written either.
 */
export type ImagesAccess =
  /** The settings block them ("never", or no route the admin allows). */
  | { readonly kind: "never" }
  /** Static build: kept blocked (its choice, or not preloaded). */
  | { readonly kind: "blocked" }
  /** The proxy isn't configured: explain instead of offering the button. */
  | { readonly kind: "unavailable" }
  | {
      readonly kind: "offered";
      /**
       * Through the proxy, or straight from the sender after a warning
       * (mail/remote-images.ts).
       */
      readonly from: ImagesRoute;
      readonly loading: boolean;
      /** The settings load them without a click (always, trusted sender). */
      readonly auto: boolean;
      /** Loaded at once because the sender is trusted. */
      readonly senderTrusted: boolean;
      /** "Always show from this sender" (trusted-senders) is offered. */
      readonly offerTrust: boolean;
    };

export interface PrivacyBarProps {
  readonly result: RenderResult;
  readonly auth: AuthVerdict | null;
  readonly images: ImagesAccess;
  readonly onShowImages: () => void;
  readonly onTrustSender: () => void;
  /**
   * The user's preload quota is full (accepted risk H15):
   * this email may not have been preloaded, so showing its images may tell
   * the sender when it was opened.
   */
  readonly preloadFull?: PreloadFull | null;
}

const fullDate = (until: number): string =>
  new Intl.DateTimeFormat(getLocale(), {
    day: "numeric",
    month: "long",
  }).format(new Date(until));

/** The route the images take (or would), if any. */
const routeOf = (images: ImagesAccess): ImagesRoute | null => {
  if (images.kind === "offered") {
    return images.from;
  }
  // Only the proxy can turn out not to be configured.
  return images.kind === "unavailable" ? "proxy" : null;
};

/** What the report says was done with the remote images. */
const imagesBody = (images: ImagesAccess, loaded: boolean): string => {
  if (images.kind === "never") {
    return m.privacy_report_images_never();
  }
  if (routeOf(images) === "sender") {
    return loaded
      ? m.privacy_report_images_direct_loaded()
      : m.privacy_report_images_direct();
  }
  return loaded
    ? m.privacy_report_images_proxy_loaded()
    : m.privacy_report_images_body();
};

/** The images part of the bar's line. */
const imagesSummary = (images: ImagesAccess, loaded: boolean): string => {
  if (images.kind !== "offered" || !loaded) {
    return images.kind === "never"
      ? m.privacy_images_never()
      : m.privacy_images_protected();
  }
  const shown =
    images.from === "sender"
      ? m.privacy_images_from_sender()
      : m.privacy_images_shown();
  return images.senderTrusted
    ? `${shown} · ${m.privacy_images_trusted()}`
    : shown;
};

/** The line under the bar, if any. */
const imagesNotice = (state: {
  readonly images: ImagesAccess;
  readonly canShow: boolean;
  readonly loaded: boolean;
  readonly preloadFull: PreloadFull | null;
  readonly warning: boolean;
}): string | null => {
  const { images } = state;
  if (images.kind === "unavailable") {
    return m.images_unavailable();
  }
  if (images.kind !== "offered") {
    return null;
  }
  if (state.warning) {
    return m.privacy_direct_warning();
  }
  if (state.canShow && images.from === "proxy" && state.preloadFull !== null) {
    return state.preloadFull.until === null
      ? m.privacy_preload_full_unknown()
      : m.privacy_preload_full({ date: fullDate(state.preloadFull.until) });
  }
  // Loaded without a click (the user's setting): still say what it means.
  return state.loaded && images.from === "sender" && images.auto
    ? m.privacy_direct_loaded_notice()
    : null;
};

export const PrivacyBar = ({
  auth,
  images,
  onShowImages,
  onTrustSender,
  preloadFull = null,
  result,
}: PrivacyBarProps) => {
  const { report } = result;
  const blocked = report.trackersBlocked;
  const loaded = result.remoteImagesLoaded;
  // Straight from the sender: a first click only shows the warning.
  const [warned, setWarned] = useState(false);
  const offered = images.kind === "offered" ? images : null;
  const canShow = offered !== null && result.remoteImages.length > 0 && !loaded;
  const direct = offered?.from === "sender";
  let showLabel = m.privacy_show_images();
  if (direct) {
    showLabel = warned ? m.privacy_direct_confirm() : m.privacy_direct_load();
  }
  const reportButton = (
    <Popover.Root>
      <Popover.Trigger
        render={
          <Button iconEnd={ChevronDown} size="sm" variant="ghost">
            {m.privacy_report()}
          </Button>
        }
      />
      <Popover.Content variant="accent">
        <Popover.Header icon={<ShieldGlyph size={22} />}>
          <Popover.Title>{m.privacy_report_title()}</Popover.Title>
          <Popover.Description>{m.privacy_report_desc()}</Popover.Description>
        </Popover.Header>
        <Report
          auth={auth}
          images={imagesBody(images, loaded)}
          report={report}
        />
      </Popover.Content>
    </Popover.Root>
  );

  const imagesText = imagesSummary(images, loaded);
  const noticeText = imagesNotice({
    canShow,
    images,
    loaded,
    preloadFull,
    warning: canShow && warned,
  });
  const notice =
    noticeText === null ? null : (
      <p aria-live="polite" {...stylex.props(styles.notice)}>
        {noticeText}
      </p>
    );

  if (blocked === 0 && result.remoteImages.length === 0) {
    return (
      <div {...stylex.props(styles.quiet)}>
        <PrivacyShield size={16} />
        <span {...stylex.props(styles.text)}>
          {m.privacy_no_trackers()} · {m.privacy_links_checked()}
        </span>
        {reportButton}
      </div>
    );
  }

  return (
    <>
      <div {...stylex.props(styles.bar)}>
        <PrivacyShield pulse={blocked} size={18} />
        <span {...stylex.props(styles.text)}>
          <span {...stylex.props(styles.strong)}>
            {blocked > 0
              ? m.privacy_trackers({ count: blocked })
              : m.privacy_no_trackers()}
          </span>
          {result.remoteImages.length > 0 ? ` · ${imagesText}` : null}
        </span>
        <span {...stylex.props(styles.buttons)}>
          {canShow ? (
            <Button
              icon={Image}
              loading={offered?.loading ?? false}
              onClick={() => {
                if (direct && !warned) {
                  setWarned(true);
                  return;
                }
                onShowImages();
              }}
              size="sm"
            >
              {showLabel}
            </Button>
          ) : null}
          {canShow && offered !== null && offered.offerTrust ? (
            <Button
              disabled={offered.loading}
              onClick={() => {
                if (direct && !warned) {
                  setWarned(true);
                  return;
                }
                onTrustSender();
              }}
              size="sm"
              variant="ghost"
            >
              {m.privacy_trust_sender()}
            </Button>
          ) : null}
          {reportButton}
        </span>
      </div>
      {notice}
    </>
  );
};
