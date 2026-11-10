// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Our removals after Proton's sanitizer. Proton keeps
// `base` and every form control; we do not: they enable CSS keyloggers and
// fake login forms (PortSwigger, "CSS: the bomb inside your inbox"). The
// rest of the list is defence in depth for elements DOMPurify already drops.

import type { ReportBuilder } from "./report";

/** Removed together with their content. */
export const REMOVED_ELEMENTS: ReadonlySet<string> = new Set([
  //
  "base",
  "meta",
  "link",
  "input",
  "select",
  "option",
  "optgroup",
  "datalist",
  "selectedcontent",
  "textarea",
  "keygen",
  // defence in depth (scripts, nested browsing contexts, plugins, foreign
  // content, inert templates whose content could be revived by re-parsing)
  "script",
  "noscript",
  "iframe",
  "frame",
  "frameset",
  "object",
  "embed",
  "applet",
  "param",
  "portal",
  "fencedframe",
  "template",
  "svg",
  "math",
  "audio",
  "video",
  "source",
  "track",
  "title",
  "xmp",
  "plaintext",
  "noembed",
  "noframes",
  "bgsound",
  "isindex",
  "menuitem",
  "slot",
]);

/**
 * Elements whose content never reaches the output: DOMPurify drops these
 * together with their content (its FORBID_CONTENTS list, for the ones it
 * removes), and we remove the rest of this list with content ourselves.
 * Other removed elements (object, embed, applet, frameset, unknown ones)
 * have their children kept by DOMPurify (KEEP_CONTENT). Used by the BFF
 * extractor to skip the same subtrees.
 */
export const DROPPED_WITH_CONTENT: ReadonlySet<string> = new Set([
  "script",
  "svg",
  "math",
  "iframe",
  "noscript",
  "noembed",
  "noframes",
  "plaintext",
  "xmp",
  "template",
  "title",
  "audio",
  "video",
  "input",
  "select",
  "option",
  "optgroup",
  "datalist",
  "selectedcontent",
  "textarea",
  "keygen",
  "base",
  "meta",
  "link",
  "source",
  "track",
]);

/**
 * Form containers whose children are ordinary content: the element goes,
 * its children stay (a "Pay now" button's text or a label's text is not
 * dangerous; the control itself is).
 */
export const UNWRAPPED_ELEMENTS: ReadonlySet<string> = new Set([
  "form",
  "button",
  "label",
  "output",
  "fieldset",
  "legend",
  "dialog",
]);

/**
 * Attributes removed everywhere. Proton already renamed the loading ones on
 * non-link elements to `proton-*`; the images step restores only what is
 * allowed. These would load, submit, navigate or hijack focus/keys.
 */
export const REMOVED_ATTRIBUTES: ReadonlySet<string> = new Set([
  "src",
  "srcset",
  "srcdoc",
  "data",
  "poster",
  "background",
  "action",
  "formaction",
  "formtarget",
  "form",
  "codebase",
  "archive",
  "classid",
  "lowsrc",
  "dynsrc",
  "manifest",
  "icon",
  "ping",
  "profile",
  "xlink:href",
  "target",
  "http-equiv",
  "autofocus",
  "accesskey",
  "contenteditable",
  "popover",
  "popovertarget",
  "popovertargetaction",
  "nonce",
  "integrity",
  "crossorigin",
  "referrerpolicy",
  "fetchpriority",
  "loading",
  "download",
  "is",
  "xmlns",
  "xmlns:xlink",
  "shadowroot",
  "shadowrootmode",
  "attributionsrc",
  "interestfor",
  "commandfor",
  "command",
]);

/** Attributes that keep their meaning on these elements only. */
const ELEMENT_ATTRIBUTES: Readonly<Record<string, ReadonlySet<string>>> = {
  a: new Set(["href"]),
  area: new Set(["href"]),
};

const unwrap = (element: Element): void => {
  element.replaceWith(...element.childNodes);
};

/** Strip removed elements, unwrap form containers, drop dangerous attributes. */
export const removeDangerousContent = (
  doc: Document,
  report: ReportBuilder
): void => {
  for (const element of doc.querySelectorAll("*")) {
    const name = element.localName;
    if (REMOVED_ELEMENTS.has(name)) {
      // Descendants of an already removed element are detached: skip them.
      if (element.isConnected) {
        element.remove();
        report.elementRemoved();
      }
      continue;
    }
    if (!element.isConnected) {
      continue;
    }
    // getAttributeNames() is a copy: attributes are removed while iterating.
    for (const rawName of element.getAttributeNames()) {
      const attrName = rawName.toLowerCase();
      const keep = ELEMENT_ATTRIBUTES[name]?.has(attrName) === true;
      if (
        !keep &&
        (REMOVED_ATTRIBUTES.has(attrName) ||
          attrName.startsWith("on") ||
          attrName === "href")
      ) {
        element.removeAttribute(rawName);
      }
    }
    if (UNWRAPPED_ELEMENTS.has(name)) {
      unwrap(element);
      report.elementRemoved();
    }
  }
};
