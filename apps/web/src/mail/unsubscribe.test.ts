// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { dkimPasses } from "./auth-results";
import type { UnsubscribeHeaders } from "./unsubscribe";
import {
  isAligned,
  parseDkimSignature,
  unsubscribeOptions,
} from "./unsubscribe";

const SERVER = "mail.nebulamail.test";
const SIGNATURE = [
  "v=1; a=rsa-sha256; c=relaxed/relaxed; d=shop.example; s=sel1;",
  " h=From:To:Subject:List-Unsubscribe:List-Unsubscribe-Post;",
  " bh=abc=; b=AbCdEf12 34567890=",
].join("\r\n");

const BASE: UnsubscribeHeaders = {
  authResults: `${SERVER}; dkim=pass header.d=shop.example header.s=sel1 header.b=AbCdEf12; spf=pass`,
  dkimSignatures: [SIGNATURE],
  from: ["news@news.shop.example"],
  post: "List-Unsubscribe=One-Click",
  serverHostname: SERVER,
  urls: ["mailto:leave@shop.example?subject=bye", "https://shop.example/u/1"],
};

describe(dkimPasses, () => {
  it("reads domain, selector and signature prefix of passing DKIM results", () => {
    expect(
      dkimPasses(
        `${SERVER}; dkim=fail header.d=x.example; DKIM=pass (ok) header.d=Shop.Example header.s=sel1 header.b=AbCd; dmarc=pass`,
        SERVER
      )
    ).toStrictEqual([
      { domain: "shop.example", selector: "sel1", signature: "AbCd" },
    ]);
  });

  it("trusts only our server's header", () => {
    expect(
      dkimPasses("evil.example; dkim=pass header.d=shop.example", SERVER)
    ).toStrictEqual([]);
  });
});

describe(parseDkimSignature, () => {
  it("unfolds the tags", () => {
    expect(parseDkimSignature(SIGNATURE)).toStrictEqual({
      domain: "shop.example",
      headers: [
        "from",
        "to",
        "subject",
        "list-unsubscribe",
        "list-unsubscribe-post",
      ],
      selector: "sel1",
      signature: "AbCdEf1234567890=",
    });
  });
});

describe(isAligned, () => {
  it.each([
    ["shop.example", "shop.example", true],
    ["shop.example", "news.shop.example", true],
    ["news.shop.example", "shop.example", false],
    ["hop.example", "shop.example", false],
    ["example", "shop.example", false],
    [".example", "shop.example", false],
  ])("d=%s for %s → %s", (d, domain, aligned) => {
    expect(isAligned(d, domain)).toBe(aligned);
  });
});

describe(unsubscribeOptions, () => {
  it("allows one-click with an aligned DKIM pass that signs both headers", () => {
    expect(unsubscribeOptions(BASE)).toStrictEqual({
      mailto: "mailto:leave@shop.example?subject=bye",
      oneClick: "https://shop.example/u/1",
      web: "https://shop.example/u/1",
    });
  });

  it("aligns with the From domain when the URL is on an ESP", () => {
    expect(
      unsubscribeOptions({ ...BASE, urls: ["https://esp.example/u/1"] })
        .oneClick
    ).toBe("https://esp.example/u/1");
  });

  it.each([
    ["no List-Unsubscribe-Post", { post: null }],
    ["another Post value", { post: "List-Unsubscribe=Yes" }],
    ["an http URL", { urls: ["http://shop.example/u/1"] }],
    [
      "DKIM from another domain",
      {
        authResults: `${SERVER}; dkim=pass header.d=other.example`,
        dkimSignatures: [
          SIGNATURE.replace("d=shop.example", "d=other.example"),
        ],
      },
    ],
    [
      "DKIM fail",
      { authResults: `${SERVER}; dkim=fail header.d=shop.example` },
    ],
    [
      "a header from someone else",
      { authResults: "mx.evil.example; dkim=pass header.d=shop.example" },
    ],
    [
      "a signature that doesn't cover List-Unsubscribe-Post",
      {
        dkimSignatures: [SIGNATURE.replace(":List-Unsubscribe-Post", "")],
      },
    ],
    [
      "an extra unverified copy that covers less",
      {
        authResults: `${SERVER}; dkim=pass header.d=shop.example`,
        dkimSignatures: [
          SIGNATURE,
          SIGNATURE.replace(":List-Unsubscribe-Post", ""),
        ],
      },
    ],
    ["no DKIM-Signature", { dkimSignatures: [] }],
  ] as const)("falls back without one-click: %s", (_name, change) => {
    const options = unsubscribeOptions({ ...BASE, ...change });
    expect(options.oneClick).toBeNull();
  });

  it("matches the signature by b= prefix when the server wrote it", () => {
    const other = SIGNATURE.replace("b=AbCdEf12", "b=ZZZZ").replace(
      ":List-Unsubscribe-Post",
      ""
    );
    expect(
      unsubscribeOptions({ ...BASE, dkimSignatures: [SIGNATURE, other] })
        .oneClick
    ).toBe("https://shop.example/u/1");
  });

  it("offers mailto and web links, ignoring other schemes", () => {
    expect(
      unsubscribeOptions({
        ...BASE,
        post: null,
        urls: ["data:text/html,x", "ftp://x.example/", "nope"],
      })
    ).toStrictEqual({ mailto: null, oneClick: null, web: null });
  });
});
