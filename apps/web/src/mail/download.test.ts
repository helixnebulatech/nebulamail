// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { analyzeAttachment } from "@nebulamail/mail-render";
import { describe, expect, it } from "vitest";

import {
  attachmentFileName,
  emlFileName,
  readPrefix,
  showControls,
} from "./download";

describe(attachmentFileName, () => {
  it.each([
    ["evil.exe. .", "evil.exe"],
    ["factura.pdf", "factura.pdf"],
    ["...", "(sin nombre)"],
    [" . ", "(sin nombre)"],
    ["", "(sin nombre)"],
  ])("%j is saved as %j", (name, saved) => {
    expect(
      attachmentFileName(analyzeAttachment({ name }), "(sin nombre)")
    ).toBe(saved);
  });
});

describe(emlFileName, () => {
  it.each([
    ["Factura de octubre", "Factura de octubre.eml"],
    ["", "message.eml"],
    ["   ...  ", "message.eml"],
    ["../../etc/passwd", "_.._etc_passwd.eml"],
    ['a/b\\c:d*e?f"g<h>i|j', "a_b_c_d_e_f_g_h_i_j.eml"],
    ["factura\u202Efdp.exe", "facturafdp.exe.eml"],
    ["zero\u200Bwidth", "zerowidth.eml"],
    ["line\r\nbreak\ttab", "line break tab.eml"],
    [".hidden", "hidden.eml"],
    ["ends with dots...", "ends with dots.eml"],
    ["CON", "_CON.eml"],
    ["lpt1", "_lpt1.eml"],
  ])("%j → %j", (subject, name) => {
    expect(emlFileName(subject)).toBe(name);
  });

  it("caps the length by characters, never splitting one", () => {
    const family = "\u{1F468}\u200D\u{1F469}\u200D\u{1F467}";
    const name = emlFileName(family.repeat(200));
    const graphemes = [...new Intl.Segmenter().segment(name)];
    expect(graphemes).toHaveLength(80 + ".eml".length);
    expect(name.startsWith(family.repeat(80))).toBeTruthy();
  });
});

describe(showControls, () => {
  it("makes bidi controls visible and leaves the rest", () => {
    expect(showControls("Subject: factura\u202Efdp.exe\r\n")).toBe(
      "Subject: factura[U+202E]fdp.exe\r\n"
    );
  });
});

const stream = (...chunks: string[]): ReadableStream<Uint8Array> =>
  new ReadableStream<Uint8Array>({
    start: (controller) => {
      for (const chunk of chunks) {
        controller.enqueue(new TextEncoder().encode(chunk));
      }
      controller.close();
    },
  });

describe(readPrefix, () => {
  it("reads everything under the limit", async () => {
    const { bytes, truncated } = await readPrefix(stream("abc", "def"), 10);
    expect([new TextDecoder().decode(bytes), truncated]).toStrictEqual([
      "abcdef",
      false,
    ]);
  });

  it("stops at the limit and cancels the rest", async () => {
    let pulled = 0;
    const body = new ReadableStream<Uint8Array>({
      pull: (controller) => {
        pulled += 1;
        controller.enqueue(new Uint8Array(4).fill(65));
      },
    });
    const { bytes, truncated } = await readPrefix(body, 10);
    expect([bytes.byteLength, truncated]).toStrictEqual([10, true]);
    expect(pulled).toBeLessThan(10);
  });

  it("handles a missing body", async () => {
    await expect(readPrefix(null, 10)).resolves.toStrictEqual({
      bytes: new Uint8Array(0),
      truncated: false,
    });
  });
});
