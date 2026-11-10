// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import type { ReceiptContext, ReceiptRequest } from "./read-receipts";
import {
  readReceiptRequest,
  receiptAction,
  returnPathAddress,
  safeToSendAlone,
} from "./read-receipts";

const request = (patch: Partial<ReceiptRequest> = {}): ReceiptRequest => ({
  answered: false,
  returnPath: "ana@x.com",
  to: ["ana@x.com"],
  ...patch,
});

const context = (patch: Partial<ReceiptContext> = {}): ReceiptContext => ({
  inJunk: false,
  mode: "ask",
  ours: false,
  ownAccount: true,
  supported: true,
  ...patch,
});

describe(receiptAction, () => {
  it("asks by default", () => {
    expect(receiptAction(request(), context())).toBe("ask");
  });

  it("does nothing without a request, once answered, or with 'never'", () => {
    expect(receiptAction(null, context())).toBe("none");
    expect(receiptAction(request({ answered: true }), context())).toBe("none");
    expect(receiptAction(request(), context({ mode: "never" }))).toBe("none");
    expect(
      receiptAction(request({ answered: true }), context({ mode: "always" }))
    ).toBe("none");
  });

  it("does nothing without MDN, in shared folders or for our own mail", () => {
    expect(receiptAction(request(), context({ supported: false }))).toBe(
      "none"
    );
    expect(receiptAction(request(), context({ ownAccount: false }))).toBe(
      "none"
    );
    expect(receiptAction(request(), context({ ours: true }))).toBe("none");
  });

  it("sends on its own with 'always' only when it is safe (RFC 8098 §2.1)", () => {
    const always = context({ mode: "always" });
    expect(receiptAction(request(), always)).toBe("send");
    // Junk: never automatically.
    expect(
      receiptAction(request(), context({ inJunk: true, mode: "always" }))
    ).toBe("ask");
    // Return-Path differs, or is missing.
    expect(
      receiptAction(request({ returnPath: "bounces@list.x.com" }), always)
    ).toBe("ask");
    expect(receiptAction(request({ returnPath: null }), always)).toBe("ask");
    // More than one address.
    expect(
      receiptAction(request({ to: ["ana@x.com", "eve@y.com"] }), always)
    ).toBe("ask");
  });

  it("ignores requests for too many addresses", () => {
    const to = [
      "a@x.com",
      "b@x.com",
      "c@x.com",
      "d@x.com",
      "e@x.com",
      "f@x.com",
    ];
    expect(receiptAction(request({ to }), context())).toBe("none");
  });
});

describe(safeToSendAlone, () => {
  it("compares addresses without case", () => {
    expect(safeToSendAlone(request({ returnPath: "ANA@X.com" }))).toBeTruthy();
  });
});

describe(returnPathAddress, () => {
  it("reads the address inside angle brackets", () => {
    expect(returnPathAddress("<ana@x.com>")).toBe("ana@x.com");
    expect(returnPathAddress(" <ana@x.com> (comment)")).toBe("ana@x.com");
    expect(returnPathAddress("ana@x.com")).toBe("ana@x.com");
  });

  it("rejects the null path and nonsense", () => {
    expect(returnPathAddress("<>")).toBeNull();
    expect(returnPathAddress("not an address")).toBeNull();
    expect(returnPathAddress(null)).toBeNull();
  });
});

describe(readReceiptRequest, () => {
  it("reads the headers and $mdnsent", () => {
    expect(
      readReceiptRequest(
        {
          "header:Disposition-Notification-To:asAddresses": [
            { email: "ana@x.com", name: "Ana" },
            { email: "" },
          ],
          "header:Return-Path": " <ana@x.com>",
        },
        { $mdnsent: true }
      )
    ).toStrictEqual({
      answered: true,
      returnPath: "ana@x.com",
      to: ["ana@x.com"],
    });
  });

  it("is null without the header (or with a malformed one)", () => {
    expect(readReceiptRequest({}, {})).toBeNull();
    expect(
      readReceiptRequest(
        { "header:Disposition-Notification-To:asAddresses": "x" },
        {}
      )
    ).toBeNull();
  });
});
