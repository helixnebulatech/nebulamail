// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { AccountId, BlobId } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import {
  applyBlobCopies,
  attachmentName,
  checkFiles,
  pastedFileName,
  startProgress,
} from "./attachments";

const part = (blobId: string, accountId?: AccountId) => ({
  blobId: BlobId.parse(blobId),
  name: `${blobId}.pdf`,
  size: 1,
  type: "application/pdf",
  ...(accountId === undefined ? {} : { accountId }),
});

describe(applyBlobCopies, () => {
  const primary = AccountId.parse("a");
  const shared = AccountId.parse("c");

  it("points other accounts' blobs at their copies in ours", () => {
    const result = applyBlobCopies(
      [part("own"), part("x", shared), { ...part("img", shared), cid: "i1" }],
      primary,
      new Map([[shared, { img: BlobId.parse("img2"), x: BlobId.parse("x2") }]])
    );
    expect(result.failed).toBe(0);
    expect(result.attachments).toStrictEqual([
      part("own"),
      { ...part("x"), blobId: "x2" },
      { ...part("img"), blobId: "img2", cid: "i1" },
    ]);
  });

  it("leaves out and counts what wasn't copied", () => {
    const result = applyBlobCopies(
      [part("x", shared), part("y", shared), part("z", primary)],
      primary,
      new Map([[shared, { x: BlobId.parse("x2") }]])
    );
    expect(result.failed).toBe(1);
    expect(result.attachments.map((item) => item.blobId)).toStrictEqual([
      "x2",
      "z",
    ]);
    expect(result.attachments.some((item) => "accountId" in item)).toBeFalsy();
  });
});

const file = (name: string, size: number, type = "text/plain"): File =>
  new File(["x".repeat(size)], name, { type });

describe(checkFiles, () => {
  const limits = { maxFile: 10, maxTotal: 15 };

  it("refuses files over maxSizeUpload and over the total, in order", () => {
    const result = checkFiles(
      [file("a", 5), file("big", 11), file("b", 6), file("c", 5), file("e", 0)],
      [{ size: 3 }],
      limits
    );
    expect(result.accepted.map((item) => item.name)).toStrictEqual(["a", "b"]);
    expect(result.rejected).toStrictEqual([
      { name: "big", reason: "too-large", size: 11 },
      { name: "c", reason: "total-too-large", size: 5 },
      { name: "e", reason: "empty", size: 0 },
    ]);
  });

  it("has no total limit when the server gives none", () => {
    const result = checkFiles([file("a", 10), file("b", 10)], [{ size: 10 }], {
      maxFile: 10,
      maxTotal: null,
    });
    expect(result.accepted).toHaveLength(2);
  });
});

describe(attachmentName, () => {
  it("removes bidi, invisible and control characters", () => {
    expect(attachmentName("factura\u202Efdp.exe")).toBe("facturafdp.exe");
    expect(attachmentName("a\u200Bb\u0000c.txt")).toBe("abc.txt");
    expect(attachmentName(" \u202E ")).toBe("attachment");
  });
});

describe(pastedFileName, () => {
  const now = new Date("2026-10-24T09:41:05Z");

  it("names clipboard images by date and keeps real names", () => {
    expect(pastedFileName(file("image.png", 1, "image/png"), now)).toBe(
      "image-2026-10-24T09-41-05.png"
    );
    expect(pastedFileName(file("", 1, "image/jpeg"), now)).toBe(
      "image-2026-10-24T09-41-05.jpeg"
    );
    expect(pastedFileName(file("plano.pdf", 1, "application/pdf"), now)).toBe(
      "plano.pdf"
    );
  });
});

describe(startProgress, () => {
  it("reports the fraction sent when the transport can", () => {
    const seen: (number | null)[] = [];
    const report = startProgress(true, 10, (fraction) => {
      seen.push(fraction);
    });
    report(2, 10);
    report(5, undefined);
    report(12, 10);
    expect(seen).toStrictEqual([0.2, 0.5, 1]);
  });

  it("goes indeterminate at once when it can't, and stays so", () => {
    const seen: (number | null)[] = [];
    const report = startProgress(false, 10, (fraction) => {
      seen.push(fraction);
    });
    report(10, 10);
    expect(seen).toStrictEqual([null]);
  });

  it("an empty file doesn't divide by zero", () => {
    const seen: (number | null)[] = [];
    startProgress(true, 0, (fraction) => {
      seen.push(fraction);
    })(0, undefined);
    expect(seen).toStrictEqual([0]);
  });
});
