// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The body of a message being written: the editor's HTML and the plain-text
// alternative sent with it (multipart/alternative). The text is generated
// from the HTML so both say the same: paragraphs → lines, lists → "- " and
// "1. ", quotes → "> ", links → "text <url>", signature after "-- ".

import {
  normalizeCid,
  normalizeContentId,
  parseInertHtml,
  sanitizeComposeHtml,
  textToComposeHtml,
} from "@nebulamail/mail-render";

export { textToComposeHtml } from "@nebulamail/mail-render";

/** RFC 3676 §4.3: "-- " (with the space) starts a signature. */
const SIGNATURE_SEPARATOR = "-- ";

/** A horizontal rule in the text version (not "-- ", the signature's). */
const HORIZONTAL_RULE = "----------";

const BLOCKS = new Set([
  "p",
  "div",
  "blockquote",
  "ul",
  "ol",
  "li",
  "pre",
  "h1",
  "h2",
  "h3",
  "h4",
  "h5",
  "h6",
  "hr",
]);

const isElement = (node: Node): node is Element => node.nodeType === 1;

const isBlock = (node: Node): node is Element =>
  isElement(node) && BLOCKS.has(node.localName);

/** Text of inline content; `<br>` becomes a new line. */
const inlineText = (node: Node): string => {
  if (!isElement(node)) {
    return node.nodeType === 3 ? (node.textContent ?? "") : "";
  }
  if (node.localName === "br") {
    return "\n";
  }
  const inner = [...node.childNodes].map(inlineText).join("");
  if (node.localName === "a") {
    const href = node.getAttribute("href") ?? "";
    const target = href.replace(/^mailto:/iu, "");
    return href === "" || target === inner.trim()
      ? inner
      : `${inner} <${target}>`;
  }
  return inner;
};

const quoteLine = (line: string): string => {
  if (line === "") {
    return ">";
  }
  return line.startsWith(">") ? `>${line}` : `> ${line}`;
};

const blockLines = (parent: Node, out: string[]): void => {
  let pending = "";
  const flush = (): void => {
    if (pending.trim() !== "") {
      out.push(...pending.split("\n"));
    }
    pending = "";
  };
  for (const child of parent.childNodes) {
    if (!isBlock(child)) {
      pending += inlineText(child);
      continue;
    }
    flush();
    const name = child.localName;
    if (name === "blockquote") {
      const inner: string[] = [];
      blockLines(child, inner);
      out.push(...inner.map(quoteLine));
    } else if (name === "ul" || name === "ol") {
      let number = 1;
      for (const item of child.children) {
        const inner: string[] = [];
        blockLines(item, inner);
        const marker = name === "ul" ? "- " : `${number}. `;
        const indent = " ".repeat(marker.length);
        out.push(
          ...inner.map((line, index) =>
            index === 0 ? `${marker}${line}` : `${indent}${line}`
          )
        );
        number += 1;
      }
    } else if (name === "hr") {
      out.push(HORIZONTAL_RULE);
    } else if (name === "div" || name === "li") {
      if (child.matches("[data-signature]")) {
        out.push(SIGNATURE_SEPARATOR);
      }
      blockLines(child, out);
    } else {
      out.push(...inlineText(child).split("\n"));
    }
  }
  flush();
};

/** The plain-text alternative of the editor's HTML. Needs a DOM. */
export const htmlToText = (html: string): string => {
  const doc = parseInertHtml(`<body>${html}</body>`);
  const lines: string[] = [];
  blockLines(doc.body, lines);
  const text = lines
    .map((line) =>
      line === SIGNATURE_SEPARATOR
        ? line
        : line.replaceAll("\u00A0", " ").trimEnd()
    )
    .join("\n")
    .replace(/^\n+/u, "")
    .trimEnd();
  // Text ends with a line break, like any text file; an empty body is "".
  return text === "" ? "" : `${text}\n`;
};

/** Pasted or dropped HTML, before it reaches the editor. */
export const sanitizePastedHtml = (html: string): string =>
  sanitizeComposeHtml(html);

/**
 * The editor's starting HTML: the body (HTML, else text), the identity's
 * signature and the quoted original. A restored message (reopened draft)
 * already has its signature. All parts are sanitized HTML.
 */
export const initialBody = (
  init: {
    readonly html?: string;
    readonly text?: string;
    readonly quoteHtml?: string;
    readonly restored?: boolean;
  },
  signature: string | null
): string => {
  const body = init.html ?? textToComposeHtml(init.text ?? "");
  if (init.restored === true) {
    return body;
  }
  const block =
    signature === null ? "" : `<div data-signature>${signature}</div>`;
  const quote = init.quoteHtml ?? "";
  // The editor keeps an empty paragraph after the last block (to type
  // after a quote); adding it now keeps "opened" from counting as an edit.
  const end = block === "" && quote === "" ? "" : "<p></p>";
  return `${body}${block}${quote}${end}`;
};

/**
 * An email's body for the editor: its sanitized HTML, else its text.
 * `inlineCids`: Content-IDs of its inline parts that go along with the new
 * message; their `cid:` images stay (every other image goes).
 */
export const editableBody = (
  html: string | null,
  text: string | null,
  inlineCids: readonly string[] = []
): string =>
  html !== null && html.trim() !== ""
    ? sanitizeComposeHtml(html, { inlineCids })
    : textToComposeHtml(text ?? "");

/** Content-IDs the editor's HTML still shows (normalized, see normalizeCid). */
export const shownCids = (html: string): ReadonlySet<string> => {
  const doc = parseInertHtml(`<body>${html}</body>`);
  return new Set(
    [...doc.body.querySelectorAll("img[src]")].flatMap((image) => {
      const src = image.getAttribute("src") ?? "";
      return /^cid:/iu.test(src) ? [normalizeCid(src.slice(4))] : [];
    })
  );
};

/**
 * What the message carries: every attachment, but inline images only while
 * the body still shows them (the user may have deleted one from the quote).
 */
export const attachmentsInUse = <T extends { readonly cid?: string }>(
  attachments: readonly T[],
  html: string
): T[] => {
  if (attachments.every((attachment) => attachment.cid === undefined)) {
    return [...attachments];
  }
  const shown = shownCids(html);
  return attachments.filter(
    (attachment) =>
      attachment.cid === undefined ||
      shown.has(normalizeContentId(attachment.cid))
  );
};

// --- Plain-text mode ------------------------------------------

/** The line that starts a signature in text (RFC 3676), as typed. */
const SIGNATURE_LINE = "-- ";

/**
 * The identity's signature as text: its `textSignature`, else its HTML
 * signature converted. Without either, null.
 */
export const plainSignature = (identity: {
  readonly signature: string | null;
  readonly textSignature?: string | null;
}): string | null => {
  const text = identity.textSignature?.trim() ?? "";
  if (text !== "") {
    return identity.textSignature?.replaceAll("\r\n", "\n").trimEnd() ?? null;
  }
  if (identity.signature === null) {
    return null;
  }
  const converted = htmlToText(identity.signature).trimEnd();
  return converted === "" ? null : converted;
};

const isQuoted = (line: string): boolean => line.startsWith(">");

/**
 * Where the quoted message at the end of a text starts: its "… wrote:"
 * line, else its first quoted line. Null when the text doesn't end with
 * a quote (a reply written between the quoted lines has no single one).
 */
const quoteStart = (lines: readonly string[]): number | null => {
  const first = lines.findIndex(isQuoted);
  if (first === -1) {
    return null;
  }
  if (
    lines.slice(first).some((line) => line.trim() !== "" && !isQuoted(line))
  ) {
    return null;
  }
  const previous = lines[first - 1];
  return previous !== undefined && previous.trimEnd().endsWith(":")
    ? first - 1
    : first;
};

/**
 * Swaps the signature of a plain-text body when the sender changes: the
 * old one (after "-- ") is replaced or removed; with none before, the new
 * one goes before the quote (or at the end). A signature the user edited
 * is left alone.
 */
export const swapTextSignature = (
  text: string,
  previous: string | null,
  next: string | null
): string => {
  if (previous !== null) {
    const old = `${SIGNATURE_LINE}\n${previous}`;
    const at = text.indexOf(old);
    if (at === -1) {
      return text;
    }
    if (next !== null) {
      return `${text.slice(0, at)}${SIGNATURE_LINE}\n${next}${text.slice(at + old.length)}`;
    }
    // Also the blank line that separated it.
    const start = text.slice(0, at).endsWith("\n\n") ? at - 1 : at;
    const end = text.slice(at + old.length).startsWith("\n")
      ? at + old.length + 1
      : at + old.length;
    return `${text.slice(0, start)}${text.slice(end)}`;
  }
  if (next === null) {
    return text;
  }
  const lines = text.split("\n");
  const quote = quoteStart(lines);
  const block = [SIGNATURE_LINE, ...next.split("\n")];
  if (quote === null) {
    const body = text.trimEnd();
    return `${body}\n\n${block.join("\n")}`;
  }
  return [...lines.slice(0, quote), ...block, "", ...lines.slice(quote)].join(
    "\n"
  );
};

/**
 * The editor's HTML as plain text, for switching to plain-text mode: the
 * signature becomes the identity's text signature (when given), the rest
 * is `htmlToText` (formatting and inline images are lost).
 */
export const htmlToPlainText = (
  html: string,
  signature: string | null
): string => {
  const doc = parseInertHtml(`<body>${html}</body>`);
  const block = doc.body.querySelector("[data-signature]");
  if (block !== null && signature !== null) {
    // Built node by node: no HTML parsing (Trusted Types).
    block.replaceChildren(
      ...signature.split("\n").map((line) => {
        const paragraph = doc.createElement("p");
        paragraph.textContent = line;
        return paragraph;
      })
    );
  }
  return htmlToText(doc.body.innerHTML).replace(/\n$/u, "");
};

/** Text lines → escaped paragraphs, "> " lines → nested blockquotes. */
const linesToHtml = (lines: readonly string[]): string => {
  let html = "";
  for (let index = 0; index < lines.length;) {
    const line = lines[index] ?? "";
    if (isQuoted(line)) {
      const quoted: string[] = [];
      while (isQuoted(lines[index] ?? "")) {
        quoted.push((lines[index] ?? "").replace(/^> ?/u, ""));
        index += 1;
      }
      html += `<blockquote>${linesToHtml(quoted)}</blockquote>`;
    } else {
      html += textToComposeHtml(line);
      index += 1;
    }
  }
  return html;
};

/**
 * A plain-text body back to editor HTML, for switching to HTML mode
 * (nothing is lost): paragraphs per line, "> " lines as quotes, the part
 * after "-- " as the signature block (the identity's HTML signature, when
 * given) and a quote at the end as the quote block.
 */
export const plainTextToHtml = (
  text: string,
  htmlSignature: string | null
): string => {
  const lines = text.replaceAll("\r\n", "\n").split("\n");
  const quote = quoteStart(lines);
  const end = quote ?? lines.length;
  const separator = lines.slice(0, end).indexOf(SIGNATURE_LINE);
  const bodyEnd = separator === -1 ? end : separator;
  const body = lines.slice(0, bodyEnd);
  // Blank lines between the body and the signature or quote stay out.
  while (body.length > 0 && body.at(-1)?.trim() === "") {
    body.pop();
  }
  let html = body.length === 0 ? "<p></p>" : linesToHtml(body);
  if (separator !== -1) {
    const signature = lines.slice(separator + 1, end);
    while (signature.length > 0 && signature.at(-1)?.trim() === "") {
      signature.pop();
    }
    html += `<div data-signature>${htmlSignature ?? linesToHtml(signature)}</div>`;
  }
  if (quote !== null) {
    html += `<div data-quote>${linesToHtml(lines.slice(quote))}</div><p></p>`;
  }
  return html;
};

/**
 * The starting text of a plain-text message: the body (text, else the HTML
 * as text), the identity's text signature after "-- ", and the quoted
 * original with "> ". A restored message is used as it is.
 */
export const initialPlainText = (
  init: {
    readonly html?: string;
    readonly text?: string;
    readonly quoteHtml?: string;
    readonly restored?: boolean;
  },
  signature: string | null
): string => {
  const body =
    init.text ?? (init.html === undefined ? "" : htmlToText(init.html));
  if (init.restored === true) {
    return body.replace(/\n$/u, "");
  }
  const quote =
    init.quoteHtml === undefined ? "" : htmlToText(init.quoteHtml).trimEnd();
  const parts = [
    body.trimEnd(),
    ...(signature === null ? [] : [`${SIGNATURE_LINE}\n${signature}`]),
    ...(quote === "" ? [] : [quote]),
  ];
  // An empty first part leaves blank lines to type in above the rest.
  return parts.length === 1 ? (parts[0] ?? "") : parts.join("\n\n");
};
