// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// @vitest-environment jsdom

import { describe, expect, it } from "vitest";

import {
  attachmentsInUse,
  editableBody,
  htmlToPlainText,
  htmlToText,
  initialBody,
  initialPlainText,
  plainSignature,
  plainTextToHtml,
  swapTextSignature,
} from "./body";

describe(htmlToText, () => {
  it("turns paragraphs into lines and keeps empty ones", () => {
    expect(htmlToText("<p>Hola</p><p></p><p>Adiós <b>ya</b></p>")).toBe(
      "Hola\n\nAdiós ya\n"
    );
  });

  it("writes lists, nested quotes and links", () => {
    expect(
      htmlToText(
        `<ul><li><p>uno</p></li><li><p>dos</p><p>más</p></li></ul><ol><li><p>a</p></li><li><p>b</p></li></ol><blockquote><p>q</p><blockquote><p>qq</p></blockquote></blockquote><p><a href="https://x.com/">aquí</a> <a href="mailto:ana@x.com">ana@x.com</a> <a href="https://y.com/">https://y.com/</a></p>`
      )
    ).toBe(
      "- uno\n- dos\n  más\n1. a\n2. b\n> q\n>> qq\naquí <https://x.com/> ana@x.com https://y.com/\n"
    );
  });

  it("puts the signature after the RFC 3676 separator and quotes replies", () => {
    expect(
      htmlToText(
        `<p>Hola</p><div data-signature><p>Ana</p></div><div data-quote><p>El lunes, Bob escribió:</p><blockquote><p>¿Vienes?</p><p></p></blockquote></div>`
      )
    ).toBe("Hola\n-- \nAna\nEl lunes, Bob escribió:\n> ¿Vienes?\n>\n");
  });

  it("writes headings and code as lines, and a rule as dashes", () => {
    expect(
      htmlToText(
        `<h1>Plan</h1><p style="text-align: center"><span style="color: #C5221F">rojo</span></p><hr><pre><code>npm i\nnpm t</code></pre>`
      )
    ).toBe("Plan\nrojo\n----------\nnpm i\nnpm t\n");
  });

  it("is empty for an empty editor", () => {
    expect(htmlToText("<p></p>")).toBe("");
  });

  it("keeps line breaks and doesn't interpret text as HTML", () => {
    expect(htmlToText("<p>a<br>&lt;b&gt;</p>")).toBe("a\n<b>\n");
  });
});

describe(initialBody, () => {
  it("adds the signature between the body and the quote", () => {
    expect(
      initialBody(
        { quoteHtml: "<div data-quote><p>q</p></div>", text: "" },
        "<p>Ana</p>"
      )
    ).toBe(
      "<p></p><div data-signature><p>Ana</p></div><div data-quote><p>q</p></div><p></p>"
    );
  });

  it("leaves restored drafts as they are and works without signature", () => {
    expect(initialBody({ html: "<p>x</p>", restored: true }, "<p>A</p>")).toBe(
      "<p>x</p>"
    );
    expect(initialBody({ text: "hola <3" }, null)).toBe("<p>hola &lt;3</p>");
  });
});

describe(editableBody, () => {
  it("uses the sanitized HTML, else the text", () => {
    expect(
      editableBody(`<p>x<img src="https://t.example/p.gif"></p>`, "x")
    ).toBe("<p>x</p>");
    expect(editableBody(null, "a\nb")).toBe("<p>a</p><p>b</p>");
    expect(editableBody(" ", null)).toBe("<p></p>");
  });

  it("keeps the cid: images of the parts that travel along", () => {
    expect(
      editableBody(
        `<p><img src="cid:a@x" alt="A"><img src="cid:b@x"><img src="https://t.example/c.png"></p>`,
        null,
        ["a@x"]
      )
    ).toBe(`<p><img src="cid:a@x" alt="A"></p>`);
  });
});

describe(attachmentsInUse, () => {
  interface Part {
    readonly blobId: string;
    readonly cid?: string;
  }
  const file: Part = { blobId: "F" };
  const shown: Part = { blobId: "S", cid: "Shown@x" };
  const deleted: Part = { blobId: "D", cid: "deleted@x" };

  it("keeps files and the inline images the body still shows", () => {
    expect(
      attachmentsInUse(
        [file, shown, deleted],
        `<p>hi</p><div data-quote><p><img alt="" src="cid:shown@X"></p></div>`
      )
    ).toStrictEqual([file, shown]);
  });

  it("leaves a list without inline images alone", () => {
    expect(attachmentsInUse([file], "<p></p>")).toStrictEqual([file]);
  });
});

describe(plainSignature, () => {
  it("uses textSignature, else the HTML one as text", () => {
    expect([
      plainSignature({
        signature: "<p>HTML</p>",
        textSignature: "Ana\r\nCEO\n",
      }),
      plainSignature({ signature: "<p><b>Ana</b></p><p>CEO</p>" }),
      plainSignature({ signature: null, textSignature: "  " }),
      plainSignature({ signature: null }),
    ]).toStrictEqual(["Ana\nCEO", "Ana\nCEO", null, null]);
  });
});

describe(initialPlainText, () => {
  it("puts the text signature and the quote after room to type", () => {
    expect(
      initialPlainText(
        {
          quoteHtml: `<div data-quote><p>Ana wrote:</p><blockquote><p>Hola</p><p></p><p>Adiós</p></blockquote></div>`,
        },
        "Bob"
      )
    ).toBe("\n\n-- \nBob\n\nAna wrote:\n> Hola\n>\n> Adiós");
  });

  it("starts from the given text, and keeps restored drafts as they are", () => {
    expect(initialPlainText({ text: "Hola\n" }, null)).toBe("Hola");
    expect(
      initialPlainText({ text: "x\n-- \nBob\n", restored: true }, "Ana")
    ).toBe("x\n-- \nBob");
    expect(initialPlainText({ html: "<p><b>Hola</b></p>" }, null)).toBe("Hola");
  });
});

describe(htmlToPlainText, () => {
  it("drops formatting and uses the text signature", () => {
    expect(
      htmlToPlainText(
        `<p><b>Hola</b> <span style="font-family: Georgia, serif">mundo</span> <img src="cid:a@b" alt="x"></p><div data-signature><p><i>Ana</i> (HTML)</p></div><div data-quote><p>Bob wrote:</p><blockquote><p>q</p></blockquote></div>`,
        "Ana"
      )
    ).toBe("Hola mundo\n-- \nAna\nBob wrote:\n> q");
  });
});

describe(plainTextToHtml, () => {
  it("loses nothing: lines, quotes, signature and quote blocks", () => {
    const html = plainTextToHtml(
      "Hola <b>\n\n-- \nAna\n\nBob wrote:\n> q\n> > qq",
      "<p><b>Ana</b></p>"
    );
    expect(html).toBe(
      `<p>Hola &lt;b&gt;</p><div data-signature><p><b>Ana</b></p></div><div data-quote><p>Bob wrote:</p><blockquote><p>q</p><blockquote><p>qq</p></blockquote></blockquote></div><p></p>`
    );
    expect(htmlToText(html)).toBe(
      "Hola <b>\n-- \nAna\nBob wrote:\n> q\n>> qq\n"
    );
  });

  it("keeps the typed signature without an HTML one, and an interleaved reply as quotes", () => {
    expect(plainTextToHtml("a\n-- \nAna\n", null)).toBe(
      `<p>a</p><div data-signature><p>Ana</p></div>`
    );
    expect(plainTextToHtml("> q\nmy answer\n> q2", null)).toBe(
      `<blockquote><p>q</p></blockquote><p>my answer</p><blockquote><p>q2</p></blockquote>`
    );
  });
});

describe(swapTextSignature, () => {
  it("replaces, removes or adds the signature before the quote", () => {
    const text = "Hola\n\n-- \nAna\n\nBob wrote:\n> q";
    expect(swapTextSignature(text, "Ana", "Eva\nCEO")).toBe(
      "Hola\n\n-- \nEva\nCEO\n\nBob wrote:\n> q"
    );
    expect(swapTextSignature(text, "Ana", null)).toBe(
      "Hola\n\nBob wrote:\n> q"
    );
    expect(swapTextSignature("Hola\n\nBob wrote:\n> q", null, "Eva")).toBe(
      "Hola\n\n-- \nEva\n\nBob wrote:\n> q"
    );
    expect(swapTextSignature("Hola\n", null, "Eva")).toBe("Hola\n\n-- \nEva");
  });

  it("leaves a signature the user edited alone", () => {
    expect(swapTextSignature("x\n-- \nAna (edited)", "Ana Pérez", "Eva")).toBe(
      "x\n-- \nAna (edited)"
    );
  });
});
