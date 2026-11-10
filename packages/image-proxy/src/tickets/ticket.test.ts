// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { fromBase64Url, toBase64Url } from "../base64";
import {
  claims,
  KEY_1,
  KEY_2,
  KEY_3,
  keys,
  NOW,
  ticket,
} from "../test-fixtures";
import {
  generateTicketKeyEntry,
  imageUrl,
  MAX_TICKET_LENGTH,
  MAX_TICKET_TTL_MS,
  parseTicketKeys,
  signTicket,
  TicketError,
  verifyTicket,
} from "./index";
import { signTicketUnchecked } from "./ticket";

const OTHER_CLAIMS = [
  { accountId: "acc2" },
  { emailId: "mail2" },
  { url: "https://images.example.com/b.png" },
  { expiresAt: NOW + 1 },
];

/** Replace one base64url character of a token segment. */
const flip = (char: string): string => (char === "A" ? "B" : "A");

interface Segments {
  readonly v: string;
  readonly kv: string;
  readonly payload: string;
  readonly mac: string;
}

const segments = (token: string): Segments => {
  const [v = "", kv = "", payload = "", mac = ""] = token.split(".");
  return { kv, mac, payload, v };
};

describe(parseTicketKeys, () => {
  it("first key signs, all verify", async () => {
    const k = await parseTicketKeys(`${KEY_2}, ${KEY_1}`);
    expect(k.current).toBe(2);
    expect([...k.keys.keys()]).toStrictEqual([2, 1]);
  });

  it.each([
    ["empty", ""],
    ["no version", "AAAA"],
    ["zero version", `0:${"A".repeat(43)}`],
    ["leading zero", `01:${"A".repeat(43)}`],
    ["short key", `1:${"A".repeat(42)}`],
    ["long key", `1:${"A".repeat(44)}`],
    ["padded", `1:${"A".repeat(43)}=`],
    ["standard base64", `1:${"+".repeat(43)}`],
    ["trailing comma", `${KEY_1},`],
    ["non-canonical tail", `1:${"A".repeat(42)}B`],
  ])("refuses %s", async (_name, value) => {
    await expect(parseTicketKeys(value)).rejects.toThrow(TicketError);
  });

  it("refuses duplicate versions", async () => {
    await expect(
      parseTicketKeys(`${KEY_1},1:${"B".repeat(42)}A`)
    ).rejects.toThrow(/twice/u);
  });

  it("never echoes key material in errors", async () => {
    const secret = `1:${"Z".repeat(42)}`;
    const message = await parseTicketKeys(secret).then(() => "", String);
    expect(message).toContain("entry 1");
    expect(message).not.toContain("ZZZZ");
  });

  it("generates valid entries", async () => {
    const entry = generateTicketKeyEntry(7);
    expect(entry).toMatch(/^7:[A-Za-z0-9_-]{43}$/u);
    const k = await parseTicketKeys(entry);
    expect(k.current).toBe(7);
  });
});

describe("sign and verify", () => {
  it("round-trips", async () => {
    const k = await keys();
    const token = await ticket(k);
    expect(token).toMatch(/^[A-Za-z0-9_.-]+$/u);
    await expect(verifyTicket(k, token, NOW)).resolves.toStrictEqual({
      claims: claims(),
      ok: true,
    });
  });

  it("is deterministic and binds every claim", async () => {
    const k = await keys();
    const base = await ticket(k);
    await expect(ticket(k)).resolves.toBe(base);
    const others = await Promise.all(
      OTHER_CLAIMS.map(async (other) => await ticket(k, other))
    );
    expect(new Set([base, ...others]).size).toBe(OTHER_CLAIMS.length + 1);
  });

  it("rejects a changed byte anywhere", async () => {
    const k = await keys();
    const token = await ticket(k);
    const tampered = Array.from(token, (char, i) => {
      const replacement = char === "." ? "x" : flip(char);
      return `${token.slice(0, i)}${replacement}${token.slice(i + 1)}`;
    });
    const results = await Promise.all(
      tampered.map(async (t) => await verifyTicket(k, t, NOW))
    );
    // Every position: never accepted.
    expect(results.filter((r) => r.ok)).toStrictEqual([]);
  });

  it("rejects a MAC moved onto other claims (bad-signature)", async () => {
    const k = await keys();
    const { v, kv, mac } = segments(await ticket(k));
    const results = await Promise.all(
      OTHER_CLAIMS.map(async (other) => {
        const { payload } = segments(await ticket(k, other));
        return await verifyTicket(k, `${v}.${kv}.${payload}.${mac}`, NOW);
      })
    );
    expect(results).toStrictEqual(
      OTHER_CLAIMS.map(() => ({ ok: false, reason: "bad-signature" }))
    );
  });

  it("has no ambiguity between id boundaries", async () => {
    const k = await keys();
    const a = segments(await ticket(k, { accountId: "ab", emailId: "c" }));
    const b = segments(await ticket(k, { accountId: "a", emailId: "bc" }));
    expect(a.mac).not.toBe(b.mac);
    await expect(
      verifyTicket(k, `${a.v}.${a.kv}.${b.payload}.${a.mac}`, NOW)
    ).resolves.toMatchObject({
      reason: "bad-signature",
    });
  });

  it("refuses ids that are not JMAP ids (separators cannot be smuggled)", async () => {
    const k = await keys();
    for (const bad of [
      { accountId: "a|b" },
      { emailId: "a.b" },
      { accountId: "" },
      { emailId: "x".repeat(256) },
    ]) {
      // oxlint-disable-next-line no-await-in-loop
      await expect(signTicket(k, claims(bad), NOW)).rejects.toThrow(
        TicketError
      );
    }
  });

  it("binds the key version", async () => {
    const k = await keys(`${KEY_2},${KEY_1}`);
    const { v, payload, mac } = segments(await ticket(k));
    await expect(
      verifyTicket(k, `${v}.1.${payload}.${mac}`, NOW)
    ).resolves.toMatchObject({
      reason: "bad-signature",
    });
  });

  it("unknown key version", async () => {
    const signer = await keys(KEY_3);
    const verifier = await keys(`${KEY_2},${KEY_1}`);
    await expect(
      verifyTicket(verifier, await ticket(signer), NOW)
    ).resolves.toStrictEqual({
      ok: false,
      reason: "unknown-key",
    });
  });

  it("a different key with the same version is a bad signature", async () => {
    const signer = await keys(`1:${"Q".repeat(42)}A`);
    await expect(
      verifyTicket(await keys(), await ticket(signer), NOW)
    ).resolves.toMatchObject({
      reason: "bad-signature",
    });
  });

  it("rotation: old tickets still verify, new ones use the new key", async () => {
    const before = await keys(KEY_1);
    const old = await ticket(before);
    const after = await keys(`${KEY_2},${KEY_1}`);
    await expect(verifyTicket(after, old, NOW)).resolves.toMatchObject({
      ok: true,
    });
    const fresh = await ticket(after);
    expect(segments(fresh).kv).toBe("2");
    await expect(verifyTicket(after, fresh, NOW)).resolves.toMatchObject({
      ok: true,
    });
    // After removing key 1, old tickets stop working.
    await expect(
      verifyTicket(await keys(KEY_2), old, NOW)
    ).resolves.toMatchObject({ reason: "unknown-key" });
  });

  it("expires", async () => {
    const k = await keys();
    const token = await ticket(k, { expiresAt: NOW + 1000 });
    await expect(verifyTicket(k, token, NOW + 999)).resolves.toMatchObject({
      ok: true,
    });
    await expect(verifyTicket(k, token, NOW + 1000)).resolves.toStrictEqual({
      ok: false,
      reason: "expired",
    });
  });

  it("caps the lifetime when signing", async () => {
    const k = await keys();
    await expect(
      signTicket(k, claims({ expiresAt: NOW + MAX_TICKET_TTL_MS }), NOW)
    ).resolves.toBeTypeOf("string");
    await expect(
      signTicket(k, claims({ expiresAt: NOW + MAX_TICKET_TTL_MS + 1 }), NOW)
    ).rejects.toThrow(TicketError);
    await expect(
      signTicket(k, claims({ expiresAt: NOW }), NOW)
    ).rejects.toThrow(TicketError);
  });

  it("refuses authentic tickets that live longer than signTicket allows", async () => {
    const k = await keys();
    const forged = await signTicketUnchecked(
      k,
      claims({ expiresAt: NOW + 24 * 60 * 60 * 1000 })
    );
    await expect(verifyTicket(k, forged, NOW)).resolves.toMatchObject({
      reason: "malformed",
    });
  });

  it("refuses non-http(s) and overlong URLs when signing", async () => {
    const k = await keys();
    const urls = [
      // oxlint-disable-next-line no-script-url -- hostile input that must be refused
      "javascript:alert(1)",
      "file:///etc/passwd",
      "/relative.png",
      "//host/a.png",
      `https://a.example/${"x".repeat(2048)}`,
    ];
    const outcomes = await Promise.all(
      urls.map(
        async (url) =>
          await signTicket(k, claims({ url }), NOW).then(
            () => "signed",
            (error: unknown) =>
              error instanceof TicketError ? "refused" : "other"
          )
      )
    );
    expect(outcomes).toStrictEqual(urls.map(() => "refused"));
  });

  it("refuses those URLs when verifying too", async () => {
    const k = await keys();
    const forged = await signTicketUnchecked(
      k,
      claims({ url: "file:///etc/passwd" })
    );
    await expect(verifyTicket(k, forged, NOW)).resolves.toMatchObject({
      reason: "malformed",
    });
  });

  it.each([
    ["empty", ""],
    ["garbage", "hello"],
    ["wrong version", "2.1.AAAA.AAAA"],
    ["leading zero key version", "1.01.AAAA.AAAA"],
    ["extra segment", "1.1.AAAA.AAAA.AAAA"],
    ["padding", "1.1.AAAA=.AAAA"],
    ["huge", `1.1.${"A".repeat(MAX_TICKET_LENGTH)}.AAAA`],
  ])("malformed: %s", async (_name, token) => {
    await expect(verifyTicket(await keys(), token, NOW)).resolves.toStrictEqual(
      { ok: false, reason: "malformed" }
    );
  });

  it("malformed: short MAC, non-JSON or non-canonical payload", async () => {
    const k = await keys();
    const { v, kv, payload, mac } = segments(await ticket(k));
    await expect(
      verifyTicket(k, `${v}.${kv}.${payload}.${mac.slice(0, 40)}`, NOW)
    ).resolves.toMatchObject({ reason: "malformed" });
    const notJson = toBase64Url(new TextEncoder().encode("not json"));
    await expect(
      verifyTicket(k, `${v}.${kv}.${notJson}.${mac}`, NOW)
    ).resolves.toMatchObject({ reason: "malformed" });
    const decoded = new TextDecoder().decode(
      fromBase64Url(payload) ?? new Uint8Array()
    );
    const spaced = toBase64Url(
      new TextEncoder().encode(decoded.replaceAll(",", ", "))
    );
    await expect(
      verifyTicket(k, `${v}.${kv}.${spaced}.${mac}`, NOW)
    ).resolves.toMatchObject({ reason: "malformed" });
  });

  it("rejects huge tokens before any crypto", async () => {
    const k = await keys();
    const huge = "1.1.".concat("A".repeat(10_000_000), ".AAAA");
    const started = performance.now();
    await expect(verifyTicket(k, huge, NOW)).resolves.toMatchObject({
      reason: "malformed",
    });
    expect(performance.now() - started).toBeLessThan(50);
  });
});

describe(imageUrl, () => {
  it("builds the cache-mode URL", async () => {
    const token = await ticket(await keys());
    expect(imageUrl("https://img.example.com", token)).toBe(
      `https://img.example.com/i/${token}`
    );
  });

  it.each([
    "https://img.example.com/",
    "https://img.example.com/x",
    "img.example.com",
  ])("refuses non-origin %s", async (origin) => {
    const token = await ticket(await keys());
    expect(() => imageUrl(origin, token)).toThrow(TicketError);
  });

  it("refuses non-ticket text", () => {
    expect(() => imageUrl("https://img.example.com", "../x")).toThrow(
      TicketError
    );
  });
});
