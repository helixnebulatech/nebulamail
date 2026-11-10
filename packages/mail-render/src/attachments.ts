// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Attachment warnings. DOM-free. These are
// warnings for the UI; HTML and SVG attachments are never previewed.

import { cleanDisplayText, hasBidiControls } from "./unicode";

export type AttachmentWarning =
  | "dangerous-extension"
  | "double-extension"
  | "mime-mismatch"
  | "bidi-in-name"
  | "no-preview";

/** Executable, script, shortcut, disk-image and active-document types. */
const DANGEROUS_EXTENSIONS = new Set([
  "ade",
  "adp",
  "app",
  "appref-ms",
  "application",
  "appx",
  "appxbundle",
  "asp",
  "aspx",
  "bat",
  "cab",
  "chm",
  "cmd",
  "com",
  "cpl",
  "dll",
  "dmg",
  "docm",
  "dotm",
  "exe",
  "gadget",
  "hlp",
  "hta",
  "htm",
  "html",
  "img",
  "inf",
  "ins",
  "iso",
  "isp",
  "jar",
  "jnlp",
  "js",
  "jse",
  "ksh",
  "library-ms",
  "lnk",
  "mht",
  "mhtml",
  "msc",
  "msh",
  "msi",
  "msix",
  "msp",
  "mst",
  "one",
  "pif",
  "pkg",
  "potm",
  "ppam",
  "ppsm",
  "pptm",
  "ps1",
  "ps1xml",
  "ps2",
  "psc1",
  "psm1",
  "py",
  "pyw",
  "reg",
  "scf",
  "scr",
  "sct",
  "settingcontent-ms",
  "sh",
  "shb",
  "shs",
  "shtml",
  "svg",
  "svgz",
  "sys",
  "url",
  "vb",
  "vbe",
  "vbs",
  "vhd",
  "vhdx",
  "website",
  "wsc",
  "wsf",
  "wsh",
  "xbap",
  "xht",
  "xhtml",
  "xlam",
  "xll",
  "xlsm",
  "xltm",
  "xnk",
]);

/** Extensions people trust; seeing one before a dangerous one is the classic trick. */
const DECOY_EXTENSIONS = new Set([
  "bmp",
  "csv",
  "doc",
  "docx",
  "gif",
  "jpeg",
  "jpg",
  "mp3",
  "mp4",
  "odp",
  "ods",
  "odt",
  "pdf",
  "png",
  "ppt",
  "pptx",
  "rtf",
  "txt",
  "wav",
  "xls",
  "xlsx",
  "zip",
]);

/** Never previewed, whatever their name says. */
const NO_PREVIEW_TYPES =
  /^(?:text\/html|application\/xhtml\+xml|image\/svg\+xml|text\/xml|application\/xml)\b/iu;

/**
 * Expected MIME types per extension (only common, unambiguous ones). A
 * `Map`, not an object: the extension comes from the sender, and
 * `x.constructor` or `x.__proto__` must not reach the prototype.
 */
const EXPECTED_MIME: ReadonlyMap<string, readonly string[]> = new Map([
  ["csv", ["text/csv", "text/plain", "application/vnd.ms-excel"]],
  ["doc", ["application/msword"]],
  [
    "docx",
    ["application/vnd.openxmlformats-officedocument.wordprocessingml.document"],
  ],
  ["gif", ["image/gif"]],
  ["jpeg", ["image/jpeg", "image/pjpeg"]],
  ["jpg", ["image/jpeg", "image/pjpeg"]],
  ["mp3", ["audio/mpeg", "audio/mp3"]],
  ["mp4", ["video/mp4", "audio/mp4"]],
  ["pdf", ["application/pdf"]],
  ["png", ["image/png"]],
  ["ppt", ["application/vnd.ms-powerpoint"]],
  [
    "pptx",
    [
      "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    ],
  ],
  ["txt", ["text/plain"]],
  ["webp", ["image/webp"]],
  ["xls", ["application/vnd.ms-excel"]],
  [
    "xlsx",
    ["application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"],
  ],
  ["zip", ["application/zip", "application/x-zip-compressed"]],
]);

/** Declared types that say nothing about the content. */
const GENERIC_MIME = new Set([
  "application/octet-stream",
  "application/unknown",
  "binary/octet-stream",
  "",
]);

export interface AttachmentInput {
  readonly name: string;
  /** Declared MIME type (JMAP `type`). */
  readonly type?: string | null;
}

export interface AttachmentAnalysis {
  /** Name to display (bidi controls and invisible characters removed). */
  readonly displayName: string;
  /**
   * Name to save the file as: `displayName` without trailing dots and
   * spaces, which Windows drops (`evil.exe.` is saved as `evil.exe`), so
   * the warnings are about the file the user really gets. May be "".
   */
  readonly fileName: string;
  /** Real final extension, lower case, or "" if none. */
  readonly extension: string;
  readonly warnings: readonly AttachmentWarning[];
}

const isWhitespace = (ch: string | undefined): boolean =>
  ch !== undefined && /^\s$/u.test(ch);

/** `name` without trailing dots and spaces, in linear time. */
const stripTrailingDotsAndSpaces = (name: string): string => {
  let end = name.length;
  while (end > 0 && (name[end - 1] === "." || isWhitespace(name[end - 1]))) {
    end -= 1;
  }
  return name.slice(0, end);
};

/**
 * The extension pushed out of view with spaces (`factura.pdf      .exe`):
 * three or more spaces right before the last dot. Linear (the regex
 * `/\s{3,}\.[^.]+$/` was quadratic on long runs of spaces).
 */
const hasPaddedExtension = (name: string): boolean => {
  const dot = name.lastIndexOf(".");
  if (dot === -1 || dot === name.length - 1) {
    return false;
  }
  let spaces = 0;
  for (let i = dot - 1; i >= 0 && spaces < 3 && isWhitespace(name[i]); i -= 1) {
    spaces += 1;
  }
  return spaces >= 3;
};

const extensionsOf = (name: string): string[] => {
  const parts = name.toLowerCase().split(".");
  return parts.length < 2 ? [] : parts.slice(1).map((part) => part.trim());
};

export const analyzeAttachment = (
  input: AttachmentInput
): AttachmentAnalysis => {
  const displayName = cleanDisplayText(input.name);
  const fileName = stripTrailingDotsAndSpaces(displayName);
  const extensions = extensionsOf(fileName);
  const extension = extensions.at(-1) ?? "";
  const warnings: AttachmentWarning[] = [];
  const type =
    (input.type ?? "").trim().toLowerCase().split(";")[0]?.trim() ?? "";

  if (hasBidiControls(input.name)) {
    warnings.push("bidi-in-name");
  }
  if (DANGEROUS_EXTENSIONS.has(extension)) {
    warnings.push("dangerous-extension");
  }
  const previous = extensions.at(-2);
  // "factura.pdf.exe", or the extension pushed out of view with spaces.
  const paddedExtension = hasPaddedExtension(fileName);
  if (
    (previous !== undefined &&
      DECOY_EXTENSIONS.has(previous) &&
      extension !== previous &&
      !DECOY_EXTENSIONS.has(extension)) ||
    paddedExtension
  ) {
    warnings.push("double-extension");
  }
  const expected = EXPECTED_MIME.get(extension);
  if (
    expected !== undefined &&
    !GENERIC_MIME.has(type) &&
    !expected.includes(type)
  ) {
    warnings.push("mime-mismatch");
  }
  if (
    NO_PREVIEW_TYPES.test(type) ||
    ["htm", "html", "svg", "svgz", "xht", "xhtml", "mht", "mhtml"].includes(
      extension
    )
  ) {
    warnings.push("no-preview");
  }
  return { displayName, extension, fileName, warnings };
};

/** Only these may be previewed (as `<img>` from a `blob:` URL). */
export const isPreviewableImage = (input: AttachmentInput): boolean => {
  const analysis = analyzeAttachment(input);
  const type = (input.type ?? "").toLowerCase();
  return (
    !analysis.warnings.includes("no-preview") &&
    !analysis.warnings.includes("mime-mismatch") &&
    /^image\/(?:png|jpeg|gif|webp|avif|bmp)$/u.test(type)
  );
};
