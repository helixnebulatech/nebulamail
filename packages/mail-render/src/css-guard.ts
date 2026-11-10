// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// CSS that the vendored Proton sanitizer would take too long on (see
// `cssCommentHazard`), found before it runs. The browser then shows the
// text part (`too-complex`); the server extracts no images (extract.ts).

import { cssCommentHazard } from "./css";

// An inert document has no window, so no `instanceof HTMLTemplateElement`.
const isTemplate = (element: Element): element is HTMLTemplateElement =>
  element.localName === "template" &&
  element.namespaceURI === "http://www.w3.org/1999/xhtml";

/**
 * Whether any `style` attribute or `<style>` of an inert document (also
 * inside `<template>` contents, which DOMPurify sanitizes too) has a
 * comment hazard. Reads the same text Proton's hook reads.
 */
export const findCssHazard = (doc: Document): boolean => {
  const roots: ParentNode[] = [doc];
  let root = roots.pop();
  while (root !== undefined) {
    for (const element of root.querySelectorAll("style, [style], template")) {
      const style = element.getAttribute("style");
      if (style !== null && cssCommentHazard(style)) {
        return true;
      }
      if (
        element.localName === "style" &&
        cssCommentHazard(element.textContent ?? "")
      ) {
        return true;
      }
      if (isTemplate(element)) {
        roots.push(element.content);
      }
    }
    root = roots.pop();
  }
  return false;
};
