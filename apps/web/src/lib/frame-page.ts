// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The page an email is moved into when its images load straight from the
// sender (mail-render `fillFramePage`, mail/remote-images.ts
// `DIRECT_FRAME_PAGE`). Both builds serve the same one: a file in the
// static zip (static/plugin.ts), a route with our server
// (routes/email-frame[.]html.ts). Only the CSP mail-render
// gives such an email (images from `https:`), no scripts; the app fills it.

import {
  buildContentSecurityPolicy,
  imageSourcesFor,
} from "@nebulamail/mail-render/document";

/** Its file name, under the app's base path. */
export const FRAME_PAGE = "email-frame.html";

/** The policy of the page: exactly that of an email with direct images. */
export const FRAME_PAGE_CSP = buildContentSecurityPolicy(
  imageSourcesFor({ kind: "direct" })
);

const escapeAttribute = (value: string): string =>
  value.replaceAll("&", "&amp;").replaceAll('"', "&quot;");

export const framePage = (): string =>
  [
    "<!DOCTYPE html>",
    '<html><head><meta charset="utf-8">',
    `<meta http-equiv="Content-Security-Policy" content="${escapeAttribute(
      FRAME_PAGE_CSP
    )}">`,
    '<meta name="referrer" content="no-referrer">',
    '<meta http-equiv="x-dns-prefetch-control" content="off">',
    "</head><body></body></html>",
    "",
  ].join("");
