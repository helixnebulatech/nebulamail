// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Plain-text view:
// the text is inserted as Text nodes and links are created as elements,
// never by concatenating HTML.

import { cleanLink } from "./links";
import type { LinkContext } from "./links";

// Links are found per whitespace-separated token with anchored, bounded
// patterns, so the cost is linear in the text length (an unanchored
// e-mail regex is quadratic on input like "a+a+a+…").
const LEADING = /^[(<[{"'\u00AB\u201C\u2018]+/u;
const TRAILING_CHARS = new Set(".,;:!?)]}>'\"\u00BB\u201D\u2019");
const URL_START = /^(?:https?:\/\/|www\.)/iu;
const EMAIL = /^[\w.+-]{1,64}@[\w-]{1,63}(?:\.[\w-]{1,63}){1,10}$/u;
const MAX_LINK_LENGTH = 4096;
/**
 * Links made per text: past this, the rest stays plain text. Each link is
 * an element (and goes through tidy-url), so a 2 MB text of short URLs
 * would otherwise make hundreds of thousands of them.
 */
export const MAX_TEXT_LINKS = 2000;

interface Segment {
  readonly text: string;
  readonly href?: string;
}

const toHref = (raw: string): string | null => {
  if (!URL_START.test(raw)) {
    return EMAIL.test(raw) ? `mailto:${raw}` : null;
  }
  const withScheme = raw.toLowerCase().startsWith("www.")
    ? `https://${raw}`
    : raw;
  try {
    const url = new URL(withScheme);
    return (url.protocol === "http:" || url.protocol === "https:") &&
      url.hostname !== ""
      ? url.href
      : null;
  } catch {
    return null;
  }
};

/** Splits text into plain and link segments. DOM-free, linear time. */
export const linkifySegments = (text: string): Segment[] => {
  const segments: Segment[] = [];
  let plain = "";
  let links = 0;
  for (const token of text.split(/(?<space>\s+)/u)) {
    if (links >= MAX_TEXT_LINKS) {
      plain += token;
      continue;
    }
    const lead = LEADING.exec(token)?.[0] ?? "";
    const rest = token.slice(lead.length);
    // A backwards loop: `/[.,…]+$/` would be quadratic on "....x".
    let end = rest.length;
    while (end > 0 && TRAILING_CHARS.has(rest[end - 1] ?? "")) {
      end -= 1;
    }
    const core = rest.slice(0, end);
    const trail = rest.slice(end);
    const href =
      core === "" || core.length > MAX_LINK_LENGTH ? null : toHref(core);
    if (href === null) {
      plain += token;
    } else {
      plain += lead;
      if (plain !== "") {
        segments.push({ text: plain });
      }
      segments.push({ href, text: core });
      links += 1;
      plain = trail;
    }
  }
  if (plain !== "") {
    segments.push({ text: plain });
  }
  return segments;
};

/**
 * Appends the plain text to `container` (in `container.ownerDocument`) as a
 * `<div class="wm-plain-text">` of Text nodes and `<a>` elements.
 */
export const appendPlainText = (
  container: Element,
  text: string,
  ctx: LinkContext
): void => {
  const doc = container.ownerDocument;
  const block = doc.createElement("div");
  block.setAttribute("class", "wm-plain-text");
  for (const segment of linkifySegments(text)) {
    if (segment.href === undefined) {
      block.append(doc.createTextNode(segment.text));
    } else {
      let { href } = segment;
      if (ctx.cleanLinks && href.startsWith("http")) {
        const cleaned = cleanLink(href);
        if (cleaned !== href) {
          ctx.report.linkCleaned(href, cleaned);
          href = cleaned;
        }
      }
      const link = doc.createElement("a");
      link.setAttribute("href", href);
      link.setAttribute("rel", "noopener noreferrer");
      link.textContent = segment.text;
      block.append(link);
    }
  }
  container.append(block);
};
