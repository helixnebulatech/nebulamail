// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import {
  authVerdict,
  authservId,
  dkimPasses,
  normalizeDomain,
  parseAuthResults,
  shownVerdict,
} from "./auth-results";

const SERVER = "mx";

describe(parseAuthResults, () => {
  it("reads method=result pairs, ignoring comments, case and properties", () => {
    expect(
      parseAuthResults(
        "mx.nebulamail.test; DKIM=pass (signature verified) header.d=example.com; spf=softfail smtp.mailfrom=example.com; dmarc=none (p=none)"
      )
    ).toStrictEqual({ dkim: "pass", dmarc: "none", spf: "softfail" });
  });

  it("never takes the authserv-id for a result", () => {
    expect(parseAuthResults("dkim=pass.example; spf=fail")).toStrictEqual({
      spf: "fail",
    });
  });

  it("counts a method as passed if any instance passed", () => {
    expect(
      parseAuthResults("mx; dkim=fail header.d=a.com; dkim=pass header.d=b.com")
    ).toStrictEqual({ dkim: "pass" });
  });
});

/** Builds an address (keeps the test free of address literals). */
const addr = (local: string, domain: string): string => `${local}@${domain}`;

const FROM = [addr("ana", "example.com")];

describe(authVerdict, () => {
  it("passes on DMARC pass for the From domain", () => {
    expect(
      authVerdict(
        "mx; dkim=fail; spf=fail; dmarc=pass header.from=example.com",
        SERVER,
        FROM
      )
    ).toBe("pass");
  });

  it("doesn't pass on DKIM and SPF without DMARC (may be another domain)", () => {
    expect(
      authVerdict(
        "mx; dkim=pass header.d=evil.example; spf=pass smtp.mailfrom=evil.example",
        SERVER,
        FROM
      )
    ).toBeNull();
    expect(
      authVerdict("mx; dkim=pass; spf=pass; dmarc=none", SERVER, FROM)
    ).toBeNull();
  });

  it("doesn't pass on a DMARC pass for another domain or without header.from", () => {
    expect([
      authVerdict("mx; dmarc=pass header.from=evil.example", SERVER, FROM),
      authVerdict("mx; dmarc=pass", SERVER, FROM),
      authVerdict("mx; dmarc=pass header.from=", SERVER, FROM),
      authVerdict("mx; dmarc=pass header.from=ex@mple.com", SERVER, FROM),
    ]).toStrictEqual([null, null, null, null]);
  });

  it("compares the domains ignoring case, quotes and a final dot", () => {
    expect(
      authVerdict('mx; dmarc=pass header.from="Example.COM."', SERVER, [
        "Ana@EXAMPLE.com",
      ])
    ).toBe("pass");
  });

  it("compares international domains in ASCII (IDNA)", () => {
    expect([
      authVerdict("mx; dmarc=pass header.from=xn--bcher-kva.example", SERVER, [
        "ana@bücher.example",
      ]),
      authVerdict("mx; dmarc=pass header.from=bücher.example", SERVER, [
        addr("ana", "bücher.example"),
      ]),
    ]).toStrictEqual(["pass", "pass"]);
  });

  it("wants exactly the From domain, not a parent, child or suffix", () => {
    expect([
      authVerdict("mx; dmarc=pass header.from=example.com", SERVER, [
        addr("ana", "mail.example.com"),
      ]),
      authVerdict("mx; dmarc=pass header.from=mail.example.com", SERVER, FROM),
      authVerdict(
        "mx; dmarc=pass header.from=example.com.evil.example",
        SERVER,
        FROM
      ),
      authVerdict("mx; dmarc=pass header.from=xample.com", SERVER, FROM),
    ]).toStrictEqual([null, null, null, null]);
  });

  it("isn't fooled by header.from in a comment or in another method", () => {
    expect([
      authVerdict(
        "mx; dmarc=pass (header.from=example.com) header.from=evil.example",
        SERVER,
        FROM
      ),
      authVerdict(
        "mx; spf=pass header.from=example.com; dmarc=pass header.from=evil.example",
        SERVER,
        FROM
      ),
    ]).toStrictEqual([null, null]);
  });

  it("looks at every DMARC result; any fail wins", () => {
    expect([
      authVerdict(
        "mx; dmarc=pass header.from=evil.example; dmarc=pass header.from=example.com",
        SERVER,
        FROM
      ),
      authVerdict(
        "mx; dmarc=pass header.from=example.com; dmarc=fail header.from=example.com",
        SERVER,
        FROM
      ),
    ]).toStrictEqual(["pass", "fail"]);
  });

  it("doesn't pass without exactly one usable From address", () => {
    const aligned = "mx; dmarc=pass header.from=example.com";

    expect([
      authVerdict(aligned, SERVER, [
        addr("ana", "example.com"),
        addr("bob", "example.com"),
      ]),
      authVerdict(aligned, SERVER, []),
      authVerdict(aligned, SERVER, ["example.com"]),
      authVerdict("mx; dmarc=fail", SERVER, [
        addr("ana", "example.com"),
        addr("bob", "example.com"),
      ]),
    ]).toStrictEqual([null, null, null, "fail"]);
  });

  it("doesn't let a misaligned pass hide failures", () => {
    expect(
      authVerdict(
        "mx; dmarc=pass header.from=evil.example; dkim=fail; spf=fail",
        SERVER,
        FROM
      )
    ).toBe("fail");
  });

  it("fails on DMARC fail, or a failure with nothing passing", () => {
    expect(
      authVerdict("mx; dkim=pass; spf=pass; dmarc=fail", SERVER, FROM)
    ).toBe("fail");
    expect(authVerdict("mx; dkim=none; spf=fail", SERVER, FROM)).toBe("fail");
    expect(authVerdict("mx; dkim=permerror", SERVER, FROM)).toBe("fail");
  });

  it("gives no verdict when unsure", () => {
    expect(authVerdict(null, SERVER, FROM)).toBeNull();
    expect(authVerdict("", SERVER, FROM)).toBeNull();
    expect(authVerdict("mx; none", SERVER, FROM)).toBeNull();
    expect(authVerdict("mx; dkim=pass; spf=fail", SERVER, FROM)).toBeNull();
    expect(authVerdict("mx; spf=neutral; dkim=none", SERVER, FROM)).toBeNull();
  });

  // RFC 8601 §2.2: a pvalue may be a quoted-string or have an RFC 5322
  // quoted local-part, which may hold ";", "(", ")" and "=".
  it("isn't fooled by a forged result inside a quoted local-part", () => {
    const bank = [addr("alerts", "bank.example")];

    expect(
      authVerdict(
        'mx; spf=pass smtp.mailfrom="x;dmarc=pass header.from=bank.example;"@evil.example; dmarc=none header.from=bank.example',
        SERVER,
        bank
      )
    ).toBeNull();
  });

  it("doesn't let a ( in a quoted-string hide a later fail", () => {
    expect(
      authVerdict(
        'mx; spf=pass smtp.mailfrom="(x"@evil.example; dmarc=fail header.from=bank.example',
        SERVER,
        [addr("alerts", "bank.example")]
      )
    ).toBe("fail");
  });

  it("reads quoted-pairs in quoted-strings and comments", () => {
    expect([
      authVerdict(
        String.raw`mx; spf=pass smtp.mailfrom="a\"; dmarc=pass header.from=example.com"@evil.example; dmarc=none`,
        SERVER,
        FROM
      ),
      authVerdict(
        String.raw`mx; spf=pass (a \) ; dmarc=pass header.from=example.com) smtp.mailfrom=evil.example`,
        SERVER,
        FROM
      ),
      authVerdict(
        "mx; dkim=pass (nested (comment; dmarc=fail) here) header.d=a.example; dmarc=pass header.from=example.com",
        SERVER,
        FROM
      ),
    ]).toStrictEqual([null, null, "pass"]);
  });

  it("gives no verdict for unclosed quotes or comments", () => {
    expect([
      authVerdict('mx; dmarc=pass header.from="example.com', SERVER, FROM),
      authVerdict("mx; dmarc=pass header.from=example.com (x", SERVER, FROM),
      authVerdict("mx; dmarc=pass ) header.from=example.com", SERVER, FROM),
    ]).toStrictEqual([null, null, null]);
  });

  it("is not fooled by results hidden in comments", () => {
    expect(authVerdict("mx; spf=fail (dmarc=pass)", SERVER, FROM)).toBe("fail");
  });
});

describe(dkimPasses, () => {
  it("isn't fooled by a forged dkim=pass inside a quoted local-part", () => {
    expect(
      dkimPasses(
        'mx; spf=pass smtp.mailfrom="a;dkim=pass header.d=list.example;"@evil.example',
        SERVER
      )
    ).toStrictEqual([]);
  });

  it("reads quoted property values whole", () => {
    expect(
      dkimPasses(
        'mx; dkim=pass header.d="List.Example" header.s=sel header.b="Ab/C+d"',
        SERVER
      )
    ).toStrictEqual([
      { domain: "list.example", selector: "sel", signature: "Ab/C+d" },
    ]);
  });
});

describe(normalizeDomain, () => {
  it("normalizes host names and rejects anything else", () => {
    expect([
      normalizeDomain(" Example.COM. "),
      normalizeDomain("bücher.example"),
      normalizeDomain("example.com.."),
      normalizeDomain(""),
      normalizeDomain("."),
      normalizeDomain("a b.com"),
      normalizeDomain("evil.example/x"),
      normalizeDomain("evil.example:25"),
      normalizeDomain("a%2e.com"),
      normalizeDomain("[::1]"),
    ]).toStrictEqual([
      "example.com",
      "xn--bcher-kva.example",
      null,
      null,
      null,
      null,
      null,
      null,
      null,
      null,
    ]);
  });
});

describe("trust", () => {
  it("reads the authserv-id", () => {
    expect([
      authservId("MX.Example.com; dkim=pass"),
      authservId("mx.example.com 1; spf=pass"),
      authservId("(comment) mx; dkim=pass"),
      authservId("; dkim=pass"),
      authservId('"mx"; dkim=pass'),
    ]).toStrictEqual(["mx.example.com", "mx.example.com", "mx", null, "mx"]);
  });

  it("gives no verdict for a header from another authserv-id", () => {
    // What a local sender could forge (Stalwart delivers it intact).
    const forged =
      "localhost; dkim=pass header.d=paypal.com; spf=pass; dmarc=pass header.from=paypal.com";
    const paypal = [addr("service", "paypal.com")];

    expect([
      authVerdict(forged, SERVER, paypal),
      authVerdict(
        "other.example; dmarc=pass header.from=paypal.com",
        SERVER,
        paypal
      ),
      authVerdict("MX; dmarc=pass header.from=paypal.com", "mx", paypal),
    ]).toStrictEqual([null, null, "pass"]);
  });
});

describe(shownVerdict, () => {
  it("shows the green badge only when the admin turned it on; failures always", () => {
    expect([
      shownVerdict("pass", false),
      shownVerdict("pass", true),
      shownVerdict("fail", false),
      shownVerdict(null, true),
    ]).toStrictEqual([null, "pass", "fail", null]);
  });
});
