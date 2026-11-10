// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import {
  SECURITY_TXT_EXPIRES,
  securityTxt,
  securityTxtResponse,
} from "./security-txt";

const DAY = 24 * 60 * 60 * 1000;

const fields = (text: string): [string, string][] =>
  text
    .split("\n")
    .filter((line) => line !== "" && !line.startsWith("#"))
    .map((line) => {
      const colon = line.indexOf(": ");
      return [line.slice(0, colon), line.slice(colon + 2)];
    });

describe("security.txt (RFC 9116)", () => {
  it("has the required fields, https contacts and the instance's Canonical", () => {
    expect(fields(securityTxt("https://mail.example.com"))).toStrictEqual([
      ["Contact", "mailto:security@helixnebula.tech"],
      [
        "Contact",
        "https://github.com/helixnebulatech/nebulamail/security/advisories/new",
      ],
      ["Expires", SECURITY_TXT_EXPIRES],
      ["Preferred-Languages", "es, en"],
      ["Canonical", "https://mail.example.com/.well-known/security.txt"],
      ["Canonical", "https://helixnebula.tech/.well-known/security.txt"],
      [
        "Policy",
        "https://github.com/helixnebulatech/nebulamail/blob/main/SECURITY.md",
      ],
    ]);
  });

  it("lists the project's Canonical once on its own domain", () => {
    const canonical = fields(securityTxt("https://helixnebula.tech")).filter(
      ([name]) => name === "Canonical"
    );
    expect(canonical).toHaveLength(1);
  });

  // Fails 30 days before Expires: check the contacts and move the date (at
  // most a year ahead, RFC 9116 §2.5.5).
  it("Expires is in the future, at most a year ahead", () => {
    const expires = Date.parse(SECURITY_TXT_EXPIRES);
    expect(expires - Date.now()).toBeGreaterThan(30 * DAY);
    expect(expires - Date.now()).toBeLessThanOrEqual(366 * DAY);
  });

  it("is served as UTF-8 plain text, cacheable", () => {
    const response = securityTxtResponse("https://mail.example.com");
    expect(Object.fromEntries(response.headers)).toStrictEqual({
      "cache-control": "public, max-age=86400",
      "content-type": "text/plain; charset=utf-8",
    });
  });
});
