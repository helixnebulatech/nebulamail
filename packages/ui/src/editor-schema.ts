// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// What the compose editor can contain and do (TipTap extensions), apart
// from its React view so it can be tested without a browser.
//
// The schema is the second filter after the sanitizer:
// paragraphs, headings (1–3), bold/italic/underline/strike, inline code
// and code blocks, links (http, https, mailto), lists, blockquotes,
// horizontal rules, line breaks, text colour and highlight (inline
// `style`, which mail clients keep), font and size (only the system fonts
// and px sizes of `@nebulamail/mail-render/compose-style`), alignment and
// direction (`dir`, ltr/rtl) of paragraphs and headings, and our
// signature and quote blocks. Whatever else is pasted becomes
// text. The only image is `<img src="cid:…">` (a quoted email's inline
// picture that travels as an inline part); it is never loaded from its
// URL: the editor shows the `data:` copy the app gives (`inlineImage`), or
// a placeholder.

import {
  composeFontOf,
  fontById,
  isComposeFontId,
  isComposeFontSize,
  isTextDirection,
  safeFontSize,
} from "@nebulamail/mail-render/compose-style";
import type {
  ComposeFontId,
  ComposeFontSize,
  TextDirection,
} from "@nebulamail/mail-render/compose-style";
import {
  Extension,
  Node,
  getStyleProperty,
  mergeAttributes,
} from "@tiptap/core";
import type { AnyExtension, Editor, EditorOptions } from "@tiptap/core";
import { Highlight } from "@tiptap/extension-highlight";
import { TextAlign } from "@tiptap/extension-text-align";
import { Color, TextStyle } from "@tiptap/extension-text-style";
import { Placeholder } from "@tiptap/extensions";
import { DOMSerializer } from "@tiptap/pm/model";
import type { Node as ProseMirrorNode, Schema } from "@tiptap/pm/model";
import { StarterKit } from "@tiptap/starter-kit";

import { isAllowedLink, normalizeLink } from "./editor-links";

/** The signature, a block of its own so it can be swapped. */
const Signature = Node.create({
  content: "block+",
  defining: true,
  group: "block",
  name: "signature",
  parseHTML: () => [{ tag: "div[data-signature]" }],
  renderHTML: ({ HTMLAttributes }) => [
    "div",
    mergeAttributes(HTMLAttributes, { "data-signature": "" }),
    0,
  ],
});

/** The quoted message of a reply or forward (header + blockquote). */
const Quote = Node.create({
  content: "block+",
  defining: true,
  group: "block",
  name: "quote",
  parseHTML: () => [{ tag: "div[data-quote]" }],
  renderHTML: ({ HTMLAttributes }) => [
    "div",
    mergeAttributes(HTMLAttributes, { "data-quote": "" }),
    0,
  ],
});

/**
 * Font and size (on TipTap's `textStyle` span) and the direction of
 * paragraphs and headings. Only our values are read or written; anything
 * else pasted is dropped (the sanitizer already rewrote it canonically).
 */
const MailTextStyle = Extension.create({
  addGlobalAttributes: () => [
    {
      attributes: {
        fontFamily: {
          default: null,
          parseHTML: (element) =>
            composeFontOf(getStyleProperty(element, "font-family") ?? ""),
          renderHTML: (attributes) =>
            isComposeFontId(attributes.fontFamily)
              ? {
                  style: `font-family: ${fontById(attributes.fontFamily).stack}`,
                }
              : {},
        },
        fontSize: {
          default: null,
          parseHTML: (element) =>
            safeFontSize(getStyleProperty(element, "font-size") ?? ""),
          renderHTML: (attributes) =>
            isComposeFontSize(attributes.fontSize)
              ? { style: `font-size: ${attributes.fontSize}` }
              : {},
        },
      },
      types: ["textStyle"],
    },
    {
      attributes: {
        dir: {
          default: null,
          parseHTML: (element) => {
            const dir = element.getAttribute("dir")?.trim().toLowerCase();
            return isTextDirection(dir) ? dir : null;
          },
          renderHTML: (attributes) =>
            isTextDirection(attributes.dir) ? { dir: attributes.dir } : {},
        },
      },
      types: ["paragraph", "heading"],
    },
  ],
  name: "mailTextStyle",
});

/** Sets the font of the selection (null: back to the default). */
export const setFont = (editor: Editor, font: ComposeFontId | null): void => {
  const chain = editor.chain().focus().setMark("textStyle", {
    fontFamily: font,
  });
  (font === null ? chain.removeEmptyTextStyle() : chain).run();
};

/** Sets the size of the selection (null: back to the default). */
export const setFontSize = (
  editor: Editor,
  size: ComposeFontSize | null
): void => {
  const chain = editor.chain().focus().setMark("textStyle", {
    fontSize: size,
  });
  (size === null ? chain.removeEmptyTextStyle() : chain).run();
};

/** Sets the direction of the paragraphs and headings in the selection. */
export const setDirection = (
  editor: Editor,
  dir: TextDirection | null
): void => {
  editor
    .chain()
    .focus()
    .updateAttributes("paragraph", { dir })
    .updateAttributes("heading", { dir })
    .run();
};

/** The font, size and direction at the cursor (null: the default). */
export const textStyleAt = (
  editor: Editor
): {
  readonly font: ComposeFontId | null;
  readonly size: ComposeFontSize | null;
  readonly dir: TextDirection | null;
} => {
  const style = editor.getAttributes("textStyle");
  const block = editor.isActive("heading")
    ? editor.getAttributes("heading")
    : editor.getAttributes("paragraph");
  return {
    dir: isTextDirection(block.dir) ? block.dir : null,
    font: isComposeFontId(style.fontFamily) ? style.fontFamily : null,
    size: isComposeFontSize(style.fontSize) ? style.fontSize : null,
  };
};

const CID_SRC = /^cid:[^\s"'<>]+$/iu;
/** What the editor may show for an inline image: raster `data:` only. */
const SHOWABLE =
  /^data:image\/(?:png|jpeg|gif|webp|avif|bmp|x-icon|vnd\.microsoft\.icon);base64,[\w+/=]+$/u;

/** The Content-ID of a `cid:` src (as written, without the scheme). */
const cidOf = (src: string): string => src.slice("cid:".length);

/** Marks inline image views so the app's late `data:` copies can fill them. */
export const INLINE_IMAGE_ATTRIBUTE = "data-nm-cid";

/** Shows `url` in an inline image's view, or its placeholder. */
export const fillInlineImage = (
  view: HTMLElement,
  url: string | undefined
): void => {
  const showable = url !== undefined && SHOWABLE.test(url);
  const current = view.querySelector("img");
  if (showable && current?.getAttribute("src") !== url) {
    const image = view.ownerDocument.createElement("img");
    image.alt = view.getAttribute("aria-label") ?? "";
    image.src = url;
    image.draggable = false;
    view.replaceChildren(image);
    view.dataset.loaded = "";
  }
};

/**
 * An inline image of the quoted email (`cid:` only, see above). An atom:
 * it can be deleted, not edited. Its view never uses the `cid:` URL.
 */
const createInlineImage = (callbacks: { readonly current: EditorCallbacks }) =>
  Node.create({
    addAttributes: () => ({
      alt: { default: "" },
      src: { default: null },
    }),
    addNodeView:
      () =>
      ({ node }) => {
        const src = String(node.attrs.src ?? "");
        const alt = String(node.attrs.alt ?? "");
        const view = document.createElement("span");
        view.className = "nm-inline-image";
        view.contentEditable = "false";
        view.setAttribute(INLINE_IMAGE_ATTRIBUTE, cidOf(src));
        view.setAttribute("role", "img");
        view.setAttribute("aria-label", alt);
        // The placeholder until (or unless) a data: copy is available.
        view.textContent = alt === "" ? "🖼" : alt;
        fillInlineImage(view, callbacks.current.inlineImage?.(cidOf(src)));
        return { dom: view, ignoreMutation: () => true };
      },
    atom: true,
    draggable: false,
    group: "inline",
    inline: true,
    name: "inlineImage",
    parseHTML: () => [
      {
        getAttrs: (element) => {
          const src = element.getAttribute("src") ?? "";
          return CID_SRC.test(src)
            ? { alt: element.getAttribute("alt") ?? "", src }
            : false;
        },
        tag: "img[src]",
      },
    ],
    renderHTML: ({ node }) => {
      const src = String(node.attrs.src ?? "");
      const alt = String(node.attrs.alt ?? "");
      // parseHTML only lets cid: in; this keeps it that way.
      return CID_SRC.test(src) ? ["img", { alt, src }] : ["span", {}, alt];
    },
  });

/** An inline image the app uploaded, to show at the cursor. */
export interface InsertedImage {
  /** Its Content-ID (no `cid:`), as the message's inline part carries it. */
  readonly cid: string;
  readonly alt: string;
}

/** Puts an inline (`cid:`) image at the cursor. False if the id is unusable. */
export const insertInlineImage = (
  editor: Editor,
  { alt, cid }: InsertedImage
): boolean => {
  const src = `cid:${cid}`;
  if (!CID_SRC.test(src)) {
    return false;
  }
  return editor
    .chain()
    .focus()
    .insertContent({ attrs: { alt, src }, type: "inlineImage" })
    .run();
};

/**
 * The HTML of one version of the document (`editorHtml`). Documents never
 * change once made, so it can run later, even after the editor is gone.
 */
export const docHtml = (schema: Schema, doc: ProseMirrorNode): string => {
  const inert = document.implementation.createHTMLDocument("");
  const fragment = DOMSerializer.fromSchema(schema).serializeFragment(
    doc.content,
    { document: inert }
  );
  const container = inert.createElement("div");
  container.append(fragment);
  return container.innerHTML;
};

/**
 * The editor's HTML, like `editor.getHTML()` but built in a document
 * without a window: there an `<img src="cid:…">` is never loaded (in the
 * page's document the browser would try, and the CSP would refuse it).
 */
export const editorHtml = (editor: Editor): string =>
  docHtml(editor.schema, editor.state.doc);

/** Heading levels the editor offers (and keeps when pasted). */
export const HEADING_LEVELS = [1, 2, 3] as const;
export type HeadingLevel = (typeof HEADING_LEVELS)[number];

/** Paragraph and heading alignments. */
export const ALIGNMENTS = ["left", "center", "right", "justify"] as const;
export type Alignment = (typeof ALIGNMENTS)[number];

/** Back to plain paragraphs: no marks, colours, headings or alignment. */
export const clearFormatting = (editor: Editor): void => {
  editor.chain().focus().unsetAllMarks().clearNodes().unsetTextAlign().run();
};

/**
 * The end of what the user wrote: before the empty paragraph TipTap keeps
 * at the end (TrailingNode), if there is more than that.
 */
const endOfText = (editor: Editor): number => {
  const { doc } = editor.state;
  const last = doc.lastChild;
  const trailing =
    last !== null &&
    doc.childCount > 1 &&
    last.type.name === "paragraph" &&
    last.content.size === 0;
  return doc.content.size - (trailing ? last.nodeSize : 0);
};

/** Replaces, adds (before the quote) or removes the signature block. */
export const applySignature = (editor: Editor, html: string | null): void => {
  let signature: { from: number; to: number } | null = null;
  let quoteAt: number | null = null;
  // Top-level blocks only (returning false skips their children).
  editor.state.doc.descendants((node, offset) => {
    if (node.type.name === "signature" && signature === null) {
      signature = { from: offset, to: offset + node.nodeSize };
    } else if (node.type.name === "quote" && quoteAt === null) {
      quoteAt = offset;
    }
    return false;
  });
  const content = html === null ? null : `<div data-signature>${html}</div>`;
  if (signature !== null) {
    if (content === null) {
      editor.commands.deleteRange(signature);
    } else {
      editor.commands.insertContentAt(signature, content);
    }
  } else if (content !== null) {
    editor.commands.insertContentAt(quoteAt ?? endOfText(editor), content);
  }
};

/**
 * A box with the latest value, read when the editor calls back (never
 * during render). React refs can't be handed to code that runs during
 * render, so the component keeps one of these in state instead.
 */
export class Latest<T> {
  #value: T;

  constructor(value: T) {
    this.#value = value;
  }

  get current(): T {
    return this.#value;
  }

  set(value: T): void {
    this.#value = value;
  }
}

/** Functions the editor calls; read at call time, so they can change. */
export interface EditorCallbacks {
  /** The editor's HTML after every change (serialized each time). */
  readonly onChange?: ((html: string) => void) | undefined;
  /**
   * Instead of `onChange`: after every change, a function that returns the
   * HTML of that version when called. Serializing a long document on every
   * keystroke is the costly part; this leaves it to whoever needs the HTML.
   */
  readonly onEdit?: ((read: () => string) => void) | undefined;
  readonly sanitize: (html: string) => string;
  readonly onFiles?: ((files: readonly File[]) => void) | undefined;
  readonly onSubmit?: (() => void) | undefined;
  readonly openLink?: (() => void) | undefined;
  /** A `data:` copy of the inline image with this Content-ID, if loaded. */
  readonly inlineImage?: ((cid: string) => string | undefined) | undefined;
}

/**
 * Editor options for the compose editor. `callbacks.current` holds the current
 * functions (React props change; TipTap keeps what it was created with).
 */
export const composeEditorOptions = (
  callbacks: { readonly current: EditorCallbacks },
  {
    content,
    placeholder = "",
  }: { readonly content: string; readonly placeholder?: string }
): Partial<EditorOptions> & { extensions: AnyExtension[] } => ({
  content,
  editorProps: {
    handlePaste: (_view, event) => {
      const data = event.clipboardData;
      const files = [...(data?.files ?? [])];
      const { onFiles } = callbacks.current;
      // Word and others put a picture of the copied text next to it: text
      // wins, files only count when there is no text (a screenshot…).
      const hasText = (data?.getData("text/plain") ?? "").trim() !== "";
      if (files.length > 0 && !hasText && onFiles !== undefined) {
        onFiles(files);
        return true;
      }
      return false;
    },
    // Pasted and dropped HTML (ProseMirror runs this for both).
    transformPastedHTML: (html) => callbacks.current.sanitize(html),
  },
  extensions: [
    StarterKit.configure({
      heading: { levels: [...HEADING_LEVELS] },
      link: {
        autolink: true,
        defaultProtocol: "https",
        HTMLAttributes: { rel: "noopener noreferrer", target: null },
        isAllowedUri: (url) => isAllowedLink(url),
        linkOnPaste: true,
        openOnClick: false,
        shouldAutoLink: (url) => normalizeLink(url) !== null,
      },
    }),
    TextStyle,
    Color,
    MailTextStyle,
    Highlight.configure({ multicolor: true }),
    TextAlign.configure({
      alignments: [...ALIGNMENTS],
      types: ["heading", "paragraph"],
    }),
    Placeholder.configure({ placeholder }),
    Signature,
    Quote,
    createInlineImage(callbacks),
    Extension.create({
      addKeyboardShortcuts: () => ({
        "Mod-Enter": () => {
          callbacks.current.onSubmit?.();
          return true;
        },
        "Mod-\\": ({ editor }) => {
          clearFormatting(editor);
          return true;
        },
        "Mod-k": () => {
          callbacks.current.openLink?.();
          return true;
        },
      }),
      name: "nebulaShortcuts",
    }),
  ],
  // The <style> TipTap would add has no CSP nonce: editor.css instead.
  injectCSS: false,
  onUpdate: ({ editor }) => {
    const { onChange, onEdit } = callbacks.current;
    if (onEdit !== undefined) {
      const { schema } = editor;
      const { doc } = editor.state;
      onEdit(() => docHtml(schema, doc));
      return;
    }
    onChange?.(editorHtml(editor));
  },
});
