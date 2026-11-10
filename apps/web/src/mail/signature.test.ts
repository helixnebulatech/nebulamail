// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// @vitest-environment jsdom

import { describe, expect, it } from "vitest";

import { initialBody } from "./body";
import { signatureHtml } from "./compose";

describe(signatureHtml, () => {
  it("sanitizes the HTML signature (no images, no scripts)", () => {
    // What Stalwart returns as-is from Identity/get (checked live).
    expect(
      signatureHtml(
        `<p><b>Alice</b> · <a href="https://nebulamail.test">Nebula Mail</a></p><img src="https://tracker.example/sig.gif"><script>alert(1)</script>`,
        "Alice · Nebula Mail"
      )
    ).toBe(
      `<p><b>Alice</b> · <a href="https://nebulamail.test/">Nebula Mail</a></p>`
    );
  });

  it("falls back to the text signature, escaped", () => {
    expect(signatureHtml("", "Ana <ana@x.com>\nVentas")).toBe(
      "<p>Ana &lt;ana@x.com&gt;</p><p>Ventas</p>"
    );
  });

  it("is null when the identity has no signature", () => {
    expect(signatureHtml("", " ")).toBeNull();
    expect(signatureHtml(undefined, undefined)).toBeNull();
  });

  it("goes between the new text and the quote of a reply", () => {
    const signature = signatureHtml("<p>Ana</p>", "");
    expect(
      initialBody({ quoteHtml: "<div data-quote><p>q</p></div>" }, signature)
    ).toBe(
      "<p></p><div data-signature><p>Ana</p></div><div data-quote><p>q</p></div><p></p>"
    );
  });
});
