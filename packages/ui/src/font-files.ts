// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The URL of the font file almost every page needs, for a
// `<link rel="preload" as="font">` in the document head: without it the
// browser finds the font only after downloading and reading fonts.css. The
// same file fonts.css points to, so the bundler emits it once, with one
// hashed URL.

import geistLatin from "@fontsource-variable/geist/files/geist-latin-wght-normal.woff2?url";

/** Geist, latin subset (ASCII and Western European), every weight. */
export const GEIST_LATIN_WOFF2: string = geistLatin;
