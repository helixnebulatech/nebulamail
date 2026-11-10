// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// @vitest-environment jsdom
// oxlint-disable no-script-url -- the tests check that javascript: links are dropped

import { Editor } from "@tiptap/core";
import { describe, expect, it, onTestFinished } from "vitest";

import {
  applySignature,
  clearFormatting,
  composeEditorOptions,
  editorHtml,
  fillInlineImage,
  insertInlineImage,
  setDirection,
  setFont,
  setFontSize,
  textStyleAt,
} from "./editor-schema";
import type { EditorCallbacks } from "./editor-schema";

// jsdom has no ClipboardEvent; ProseMirror's pasteHTML only needs an Event.
if (!("ClipboardEvent" in globalThis)) {
  Object.defineProperty(globalThis, "ClipboardEvent", {
    configurable: true,
    value: class extends Event {
      readonly clipboardData = null;
    },
  });
}

const make = (
  content: string,
  callbacks: Partial<EditorCallbacks> = {}
): { editor: Editor; changes: string[]; pasted: string[] } => {
  const changes: string[] = [];
  const pasted: string[] = [];
  const all: EditorCallbacks = {
    onChange: (html) => {
      changes.push(html);
    },
    sanitize: (html) => {
      pasted.push(html);
      return "<p>clean</p>";
    },
    ...callbacks,
  };
  const editor = new Editor({
    ...composeEditorOptions({ current: all }, { content }),
    element: document.createElement("div"),
  });
  onTestFinished(() => {
    editor.destroy();
  });
  return { changes, editor, pasted };
};

describe(composeEditorOptions, () => {
  it("never injects a <style> (it would have no CSP nonce)", () => {
    make("<p>x</p>");
    expect(document.querySelectorAll("style")).toHaveLength(0);
  });

  it("keeps formatting, lists, links and our blocks; drops the rest", () => {
    const { editor } = make(
      `<p><b>a</b> <i>b</i> <u>c</u> <s>k</s> <a href="https://x.com/">d</a></p><ul><li><p>e</p></li></ul><blockquote><p>f</p></blockquote><div data-signature><p>g</p></div><div data-quote><p>h</p></div><h1>i</h1><h4>l</h4><hr><img src="https://x/y.png"><pre><code>j</code></pre>`
    );
    expect(editor.getHTML()).toBe(
      `<p><strong>a</strong> <em>b</em> <u>c</u> <s>k</s> <a rel="noopener noreferrer" href="https://x.com/">d</a></p><ul><li><p>e</p></li></ul><blockquote><p>f</p></blockquote><div data-signature=""><p>g</p></div><div data-quote=""><p>h</p></div><h1>i</h1><p>l</p><hr><pre><code>j</code></pre>`
    );
  });

  it("keeps text colour, highlight and alignment as inline style", () => {
    const { editor } = make(
      `<p style="text-align: center"><span style="color: #C5221F">a</span> <mark data-color="#FFF3A3">b</mark></p><h2 style="text-align: right">c</h2>`
    );
    // jsdom writes colours back as rgb() (browsers keep what was set); the
    // compose sanitizer turns rgb() into hex again.
    expect(editor.getHTML()).toBe(
      `<p style="text-align: center;"><span style="color: rgb(197, 34, 31);">a</span> <mark data-color="#FFF3A3" style="background-color: rgb(255, 243, 163); color: inherit;">b</mark></p><h2 style="text-align: right;">c</h2>`
    );
  });

  it("keeps only our fonts and sizes, written canonically", () => {
    const { editor } = make(
      `<p><span style="font-family: georgia, serif; font-size: 18px">a</span> <span style="font-family: Comic Sans MS; font-size: 13px">b</span> <span style="font-family: url(https://x/f.woff)">c</span> <span style="font-family: &quot;Times New Roman&quot;, serif">d</span></p>`
    );
    // jsdom writes the quotes of "Times New Roman" back (a browser keeps
    // the attribute as set); both name the same font.
    expect(editorHtml(editor).replaceAll("&quot;", "")).toBe(
      `<p><span style="font-family: Georgia, serif; font-size: 18px;">a</span> <span>b</span> <span>c</span> <span style="font-family: Times New Roman, serif;">d</span></p>`
    );
  });

  it("keeps dir on paragraphs and headings, only ltr or rtl", () => {
    const { editor } = make(
      `<p dir="rtl">a</p><h2 dir="LTR">b</h2><p dir="auto">c</p><p dir="x">d</p>`
    );
    expect(editorHtml(editor)).toBe(
      `<p dir="rtl">a</p><h2 dir="ltr">b</h2><p>c</p><p>d</p>`
    );
  });

  it("sets and unsets font, size and direction from the toolbar", () => {
    const { editor } = make("<p>hola</p>");
    editor.commands.selectAll();
    setFont(editor, "verdana");
    setFontSize(editor, "24px");
    setDirection(editor, "rtl");
    expect(textStyleAt(editor)).toStrictEqual({
      dir: "rtl",
      font: "verdana",
      size: "24px",
    });
    expect(editorHtml(editor)).toBe(
      `<p dir="rtl"><span style="font-family: Verdana, sans-serif; font-size: 24px;">hola</span></p>`
    );
    setFont(editor, null);
    setFontSize(editor, null);
    setDirection(editor, null);
    expect(editorHtml(editor)).toBe("<p>hola</p>");
    expect(textStyleAt(editor)).toStrictEqual({
      dir: null,
      font: null,
      size: null,
    });
  });

  it("inserts an uploaded inline image as cid: only", () => {
    const { editor } = make("<p>ab</p>");
    editor.commands.setTextSelection(2);
    expect(
      insertInlineImage(editor, { alt: "foto.png", cid: "x1@nebula" })
    ).toBeTruthy();
    expect(
      insertInlineImage(editor, { alt: "", cid: 'x" onerror="a' })
    ).toBeFalsy();
    expect(editorHtml(editor)).toBe(
      `<p>a<img alt="foto.png" src="cid:x1@nebula">b</p>`
    );
  });

  it("clears marks, colours, headings and alignment", () => {
    const { editor } = make(
      `<h2 style="text-align: center"><strong><span style="color: #C5221F">a</span></strong> <mark data-color="#FFF3A3">b</mark></h2>`
    );
    editor.commands.selectAll();
    clearFormatting(editor);
    // The empty paragraph is TipTap's trailing node.
    expect(editor.getHTML()).toBe("<p>a b</p><p></p>");
  });

  it("drops links that aren't http(s) or mailto", () => {
    const { editor } = make(
      `<p><a href="javascript:alert(1)">a</a> <a href="data:text/html,x">b</a> <a href="mailto:ana@x.com">c</a></p>`
    );
    expect(editor.getHTML()).toBe(
      `<p>a b <a rel="noopener noreferrer" href="mailto:ana@x.com">c</a></p>`
    );
  });

  it("passes pasted HTML through the sanitizer", () => {
    const { editor, pasted } = make("<p></p>");
    editor.view.pasteHTML(`<p>dirty<img src="https://t.example/p.gif"></p>`);
    expect(pasted).toStrictEqual([
      `<p>dirty<img src="https://t.example/p.gif"></p>`,
    ]);
    expect(editor.getHTML()).toContain("clean");
    expect(editor.getHTML()).not.toContain("dirty");
  });

  it("hands pasted files over, unless the clipboard also has text", () => {
    const received: string[] = [];
    const { editor } = make("<p></p>", {
      onFiles: (files) => {
        received.push(...files.map((file) => file.name));
      },
    });
    const paste = (text: string): boolean => {
      const event = new ClipboardEvent("paste");
      Object.defineProperty(event, "clipboardData", {
        value: {
          files: [new File(["x"], "shot.png", { type: "image/png" })],
          getData: (type: string) => (type === "text/plain" ? text : ""),
        },
      });
      return (
        editor.view.someProp("handlePaste", (handle) =>
          handle(editor.view, event, editor.state.doc.slice(0, 0))
        ) ?? false
      );
    };
    expect(paste("")).toBeTruthy();
    expect(paste("Texto copiado de Word")).toBeFalsy();
    expect(received).toStrictEqual(["shot.png"]);
  });

  it("keeps cid: images only, and never puts their URL in the page", () => {
    const { editor } = make(
      `<p>a<img src="cid:logo@x" alt="Logo" width="1"><img src="https://t.example/p.gif"><img src="data:image/png;base64,AAAA">b</p>`,
      {
        inlineImage: (cid) =>
          cid === "logo@x" ? "data:image/png;base64,iVBORw0KGgo=" : undefined,
      }
    );
    expect(editorHtml(editor)).toBe(
      `<p>a<img alt="Logo" src="cid:logo@x">b</p>`
    );
    const view = editor.view.dom.querySelector(".nm-inline-image");
    expect(view?.getAttribute("role")).toBe("img");
    expect(view?.querySelector("img")?.getAttribute("src")).toBe(
      "data:image/png;base64,iVBORw0KGgo="
    );
    // Nothing in the editor's DOM points at cid: or a remote URL.
    expect(editor.view.dom.innerHTML).not.toMatch(/src="(?:cid|https?):/u);
  });

  it("shows a placeholder until the data: copy arrives (never SVG)", () => {
    const { editor } = make(`<p><img src="cid:a@x" alt="Chart"></p>`, {
      inlineImage: () => "data:image/svg+xml;base64,PHN2Zz4=",
    });
    const view = editor.view.dom.querySelector<HTMLElement>(".nm-inline-image");
    expect(view?.querySelector("img")).toBeNull();
    expect(view?.textContent).toBe("Chart");
    if (view !== null && view !== undefined) {
      fillInlineImage(view, "data:image/gif;base64,R0lGODlh");
    }
    expect(view?.querySelector("img")?.getAttribute("alt")).toBe("Chart");
  });

  it("reports changes as HTML", () => {
    const { changes, editor } = make("<p></p>");
    editor.commands.insertContent("hola");
    expect(changes.at(-1)).toBe("<p>hola</p>");
  });
});

describe(applySignature, () => {
  it("adds the signature before the quote, swaps it and removes it", () => {
    const { editor } = make(`<p>hi</p><div data-quote><p>q</p></div>`);
    applySignature(editor, "<p>Ana</p>");
    expect(editor.getHTML()).toBe(
      `<p>hi</p><div data-signature=""><p>Ana</p></div><div data-quote=""><p>q</p></div><p></p>`
    );
    applySignature(editor, "<p>Bob</p>");
    expect(editor.getHTML()).toBe(
      `<p>hi</p><div data-signature=""><p>Bob</p></div><div data-quote=""><p>q</p></div><p></p>`
    );
    applySignature(editor, null);
    expect(editor.getHTML()).toBe(
      `<p>hi</p><div data-quote=""><p>q</p></div><p></p>`
    );
  });

  it("adds it at the end without a quote, before the trailing paragraph", () => {
    const { editor } = make("<p>hi</p>");
    applySignature(editor, "<p>Ana</p>");
    expect(editor.getHTML()).toBe(
      `<p>hi</p><div data-signature=""><p>Ana</p></div><p></p>`
    );
    applySignature(editor, null);
    applySignature(editor, "<p>Bob</p>");
    expect(editor.getHTML()).toBe(
      `<p>hi</p><div data-signature=""><p>Bob</p></div><p></p>`
    );
  });
});
