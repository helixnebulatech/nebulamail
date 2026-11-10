// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Explicit tests for every blocked and allowed range. We do not rely on
// ipaddr.js being right: each range is probed at its first, last and a middle
// address, and the public neighbours just outside are checked to stay allowed.

import { describe, expect, it } from "vitest";

import { checkAddress } from "./address";

/** Addresses whose verdict is not `expected`; tests assert this is empty. */
const wrong = (ips: readonly string[], expected: boolean): string[] =>
  ips.filter((ip) => checkAddress(ip).ok !== expected);

// [range, first, middle, last, public neighbours just outside]
const V4_RANGES: readonly (readonly [
  string,
  string,
  string,
  string,
  readonly string[],
])[] = [
  ["0.0.0.0/8", "0.0.0.0", "0.12.34.56", "0.255.255.255", ["1.0.0.0"]],
  [
    "10.0.0.0/8",
    "10.0.0.0",
    "10.20.30.40",
    "10.255.255.255",
    ["9.255.255.255", "11.0.0.0"],
  ],
  [
    "100.64.0.0/10",
    "100.64.0.0",
    "100.100.1.1",
    "100.127.255.255",
    ["100.63.255.255", "100.128.0.0"],
  ],
  [
    "127.0.0.0/8",
    "127.0.0.0",
    "127.0.0.1",
    "127.255.255.255",
    ["126.255.255.255", "128.0.0.0"],
  ],
  [
    "169.254.0.0/16",
    "169.254.0.0",
    "169.254.169.254",
    "169.254.255.255",
    ["169.253.255.255", "169.255.0.0"],
  ],
  [
    "172.16.0.0/12",
    "172.16.0.0",
    "172.20.1.1",
    "172.31.255.255",
    ["172.15.255.255", "172.32.0.0"],
  ],
  [
    "192.0.0.0/24",
    "192.0.0.0",
    "192.0.0.170",
    "192.0.0.255",
    ["191.255.255.255", "192.0.1.0"],
  ],
  [
    "192.0.2.0/24",
    "192.0.2.0",
    "192.0.2.1",
    "192.0.2.255",
    ["192.0.1.255", "192.0.3.0"],
  ],
  [
    "192.88.99.0/24",
    "192.88.99.0",
    "192.88.99.1",
    "192.88.99.255",
    ["192.88.98.255", "192.88.100.0"],
  ],
  [
    "192.168.0.0/16",
    "192.168.0.0",
    "192.168.1.1",
    "192.168.255.255",
    ["192.167.255.255", "192.169.0.0"],
  ],
  [
    "198.18.0.0/15",
    "198.18.0.0",
    "198.19.1.1",
    "198.19.255.255",
    ["198.17.255.255", "198.20.0.0"],
  ],
  [
    "198.51.100.0/24",
    "198.51.100.0",
    "198.51.100.7",
    "198.51.100.255",
    ["198.51.99.255", "198.51.101.0"],
  ],
  [
    "203.0.113.0/24",
    "203.0.113.0",
    "203.0.113.5",
    "203.0.113.255",
    ["203.0.112.255", "203.0.114.0"],
  ],
  [
    "224.0.0.0/4",
    "224.0.0.0",
    "239.1.2.3",
    "239.255.255.255",
    ["223.255.255.255"],
  ],
  ["240.0.0.0/4", "240.0.0.0", "250.1.2.3", "255.255.255.254", []],
  [
    "255.255.255.255/32",
    "255.255.255.255",
    "255.255.255.255",
    "255.255.255.255",
    [],
  ],
];

const V6_RANGES: readonly (readonly [
  string,
  readonly string[],
  readonly string[],
])[] = [
  ["::/128", ["::"], []],
  ["::1/128", ["::1"], []],
  [
    "fc00::/7",
    ["fc00::", "fd12:3456::1", "fdff:ffff:ffff:ffff:ffff:ffff:ffff:ffff"],
    [],
  ],
  [
    "fe80::/10",
    ["fe80::", "fe80::1", "febf:ffff:ffff:ffff:ffff:ffff:ffff:ffff"],
    [],
  ],
  [
    "fec0::/10",
    ["fec0::", "fec0::1", "feff:ffff:ffff:ffff:ffff:ffff:ffff:ffff"],
    [],
  ],
  [
    "ff00::/8",
    ["ff00::", "ff02::1", "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff"],
    [],
  ],
  [
    "2001:db8::/32",
    ["2001:db8::", "2001:db8:1::1", "2001:db8:ffff:ffff:ffff:ffff:ffff:ffff"],
    ["2001:db7:ffff:ffff:ffff:ffff:ffff:ffff", "2001:db9::"],
  ],
  ["100::/64", ["100::", "100::1", "100::ffff:ffff:ffff:ffff"], []],
  [
    "2001::/32 (teredo)",
    [
      "2001::",
      "2001:0:4136:e378:8000:63bf:3fff:fdd2",
      "2001:0:ffff:ffff:ffff:ffff:ffff:ffff",
    ],
    [],
  ],
  [
    "2001::/23 (IETF)",
    ["2001:2::1", "2001:10::1", "2001:1ff:ffff:ffff:ffff:ffff:ffff:ffff"],
    ["2001:200::1"],
  ],
  [
    "3fff::/20 (documentation)",
    ["3fff::1", "3fff:fff:ffff:ffff:ffff:ffff:ffff:ffff"],
    ["3fff:1000::1", "3ffe:ffff::1"],
  ],
  ["::/96 (IPv4-compatible)", ["::127.0.0.1", "::8.8.8.8", "::ffff:ffff"], []],
  ["::ffff:0:0:0/96 (IPv4-translated)", ["::ffff:0:8.8.8.8"], []],
  [
    "64:ff9b:1::/48 (NAT64 local)",
    ["64:ff9b:1::808:808", "64:ff9b:1::a00:1"],
    [],
  ],
  [
    "outside 2000::/3",
    ["1::1", "4000::1", "5f00::1", "e000::1", "64:ff9a::1"],
    [],
  ],
];

describe("checkAddress: IPv4 ranges", () => {
  it.each(V4_RANGES)("%s", (_range, first, middle, last, outside) => {
    expect(wrong([first, middle, last], false)).toStrictEqual([]);
    expect(wrong(outside, true)).toStrictEqual([]);
  });

  it("allows ordinary public addresses", () => {
    for (const ip of [
      "1.1.1.1",
      "8.8.8.8",
      "93.184.215.14",
      "223.255.255.255",
    ]) {
      expect(checkAddress(ip)).toStrictEqual({
        address: ip,
        family: 4,
        ok: true,
      });
    }
  });
});

describe("checkAddress: IPv6 ranges", () => {
  it.each(V6_RANGES)("%s", (_range, inside, outside) => {
    expect(wrong(inside, false)).toStrictEqual([]);
    expect(wrong(outside, true)).toStrictEqual([]);
  });

  it("allows global unicast", () => {
    for (const ip of [
      "2606:4700:4700::1111",
      "2a00:1450:4001:80b::200e",
      "2001:4860:4860::8888",
    ]) {
      expect(checkAddress(ip)).toMatchObject({ family: 6, ok: true });
    }
  });

  it("returns canonical text", () => {
    expect(
      checkAddress("2606:4700:4700:0000:0000:0000:0000:1111")
    ).toMatchObject({
      address: "2606:4700:4700::1111",
    });
  });
});

describe("checkAddress: embedded IPv4", () => {
  it("::ffff:0:0/96 is judged by the IPv4", () => {
    expect(checkAddress("::ffff:127.0.0.1")).toStrictEqual({
      ok: false,
      range: "ipv4-mapped:loopback",
    });
    expect(checkAddress("::ffff:a00:1")).toStrictEqual({
      ok: false,
      range: "ipv4-mapped:private",
    });
    expect(checkAddress("::ffff:8.8.8.8")).toMatchObject({ ok: true });
  });

  it("64:ff9b::/96 (NAT64) is judged by the IPv4", () => {
    expect(checkAddress("64:ff9b::7f00:1")).toStrictEqual({
      ok: false,
      range: "nat64:loopback",
    });
    expect(checkAddress("64:ff9b::169.254.169.254")).toStrictEqual({
      ok: false,
      range: "nat64:link-local",
    });
    expect(checkAddress("64:ff9b::808:808")).toMatchObject({ ok: true });
  });

  it("2002::/16 (6to4) is judged by the IPv4 in bits 16–47", () => {
    expect(checkAddress("2002:7f00:1::")).toStrictEqual({
      ok: false,
      range: "6to4:loopback",
    });
    expect(checkAddress("2002:c0a8:101::1")).toStrictEqual({
      ok: false,
      range: "6to4:private",
    });
    expect(checkAddress("2002:808:808::1")).toMatchObject({ ok: true });
  });

  it("IPv4-compatible ::a.b.c.d is blocked even with a public IPv4 (ipaddr.js says unicast)", () => {
    expect(checkAddress("::8.8.8.8")).toStrictEqual({
      ok: false,
      range: "ipv4-compatible",
    });
  });
});

describe("checkAddress: input forms", () => {
  it.each([
    "",
    "localhost",
    "0x7f.1",
    "0177.0.0.1",
    "127.1",
    "2130706433",
    "127.0.0.01",
    "[::1]",
    "fe80::1%eth0",
    "2606:4700::1%1",
    " 8.8.8.8",
    "8.8.8.8 ",
    "256.1.1.1",
    "::ffff:999.0.0.1",
  ])("rejects non-canonical %j as invalid", (text) => {
    expect(checkAddress(text)).toStrictEqual({ ok: false, range: "invalid" });
  });
});

// Property-style tests with a seeded PRNG: deterministic, no extra dependency.
// Park–Miller "minimal standard" generator: plain arithmetic, exact in doubles.
const prng = (seed: number) => {
  let state = seed;
  const step = () => {
    state = (state * 48_271) % 2_147_483_647;
    return state;
  };
  /** A uniformly distributed 32-bit unsigned integer. */
  return () => (step() % 65_536) * 65_536 + (step() % 65_536);
};

const v4FromInt = (n: number) =>
  [2 ** 24, 2 ** 16, 2 ** 8, 1]
    .map((unit) => Math.floor(n / unit) % 256)
    .join(".");

const hex16 = (n: number) => (n % 65_536).toString(16);

describe("checkAddress: properties", () => {
  const random = prng(24_301);
  const samples = Array.from({ length: 3000 }, () => v4FromInt(random()));

  it("embedding an IPv4 never changes the verdict (mapped, NAT64, 6to4)", () => {
    const mismatches: string[] = [];
    for (const ip of samples) {
      const verdict = checkAddress(ip).ok;
      const [a = 0, b = 0, c = 0, d = 0] = ip.split(".").map(Number);
      const hi = hex16(a * 256 + b);
      const lo = hex16(c * 256 + d);
      for (const embedded of [
        `::ffff:${ip}`,
        `64:ff9b::${hi}:${lo}`,
        `2002:${hi}:${lo}::1`,
      ]) {
        if (checkAddress(embedded).ok !== verdict) {
          mismatches.push(embedded);
        }
      }
    }
    expect(mismatches).toStrictEqual([]);
  });

  it("IPv4-compatible and NAT64 local-use are always blocked", () => {
    const compatible = samples.map((ip) => `::${ip}`);
    const nat64Local = samples.map((ip) => `64:ff9b:1::${ip}`);
    expect(wrong([...compatible, ...nat64Local], false)).toStrictEqual([]);
  });

  it("random addresses inside each blocked IPv4 range are blocked", () => {
    const ranges: readonly (readonly [number, number])[] = [
      [0x00_00_00_00, 8],
      [0x0a_00_00_00, 8],
      [0x64_40_00_00, 10],
      [0x7f_00_00_00, 8],
      [0xa9_fe_00_00, 16],
      [0xac_10_00_00, 12],
      [0xc0_00_00_00, 24],
      [0xc0_00_02_00, 24],
      [0xc0_58_63_00, 24],
      [0xc0_a8_00_00, 16],
      [0xc6_12_00_00, 15],
      [0xc6_33_64_00, 24],
      [0xcb_00_71_00, 24],
      [0xe0_00_00_00, 4],
      [0xf0_00_00_00, 4],
    ];
    const inside = ranges.flatMap(([base, bits]) =>
      Array.from({ length: 200 }, () =>
        v4FromInt(base + (random() % 2 ** (32 - bits)))
      )
    );
    expect(wrong(inside, false)).toStrictEqual([]);
  });

  it("random IPv6 outside 2000::/3 is always blocked", () => {
    const outside: string[] = [];
    while (outside.length < 2000) {
      const first = random() % 65_536;
      // Skip 2000::/3 and 0x64 (NAT64 prefixes are tested separately).
      if ((first < 0x20_00 || first > 0x3f_ff) && first !== 0x64) {
        outside.push(
          `${hex16(first)}:${hex16(random())}:${hex16(random())}::${hex16(random())}`
        );
      }
    }
    expect(wrong(outside, false)).toStrictEqual([]);
  });
});
