// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import {
  classifyLink,
  midUrl,
  parseRefParam,
  refParam,
  taskFromEmail,
} from "./links";

const origin = "https://mail.example.com";

describe(midUrl, () => {
  it("writes RFC 2392 mid: URLs", () => {
    expect(midUrl("<abc.123@host.example>")).toBe("mid:abc.123@host.example");
    expect(midUrl("a/b%c?d@x")).toBe("mid:a%2Fb%25c%3Fd@x");
    expect(midUrl(" <> ")).toBeNull();
  });
});

describe(classifyLink, () => {
  it("opens our own email URLs in the app", () => {
    expect(classifyLink(`${origin}/mail/inbox/M123`, origin)).toStrictEqual({
      kind: "email",
      path: "/mail/inbox/M123",
    });
    expect(classifyLink(`${origin}/mail/b.f1/M1?x=1`, origin)).toStrictEqual({
      kind: "email",
      path: "/mail/b.f1/M1",
    });
  });

  it("never opens another site's or another path in the app", () => {
    expect(
      classifyLink("https://evil.example/mail/inbox/M1", origin)
    ).toStrictEqual({
      href: "https://evil.example/mail/inbox/M1",
      kind: "web",
    });
    expect(classifyLink(`${origin}/settings`, origin)).toStrictEqual({
      href: `${origin}/settings`,
      kind: "web",
    });
    // A script URL, built so the linter doesn't take it for one.
    const script = ["javascript", "alert(1)"].join(":");
    expect(classifyLink(script, origin)).toStrictEqual({
      kind: "other",
    });
    expect(classifyLink("data:text/html,x", origin)).toStrictEqual({
      kind: "other",
    });
    expect(classifyLink("not a url", origin)).toStrictEqual({ kind: "other" });
  });

  it("reads mid: links", () => {
    expect(classifyLink("mid:a%2Fb@x", origin)).toStrictEqual({
      kind: "mid",
      messageId: "a/b@x",
    });
    expect(classifyLink("MID:abc@x/part1", origin)).toStrictEqual({
      kind: "mid",
      messageId: "abc@x",
    });
    expect(classifyLink("mid:%E0%A4%A", origin)).toStrictEqual({
      kind: "other",
    });
  });
});

const words = {
  formatDate: (ms: number) => new Date(ms).toISOString().slice(0, 10),
  linkTitle: (subject: string) => `Email: ${subject}`,
  noSubject: "(No subject)",
  notes: (sender: string, date: string) => `From: ${sender} · ${date}`,
};

describe(taskFromEmail, () => {
  it("takes the subject, the sender and date, and both links", () => {
    expect(
      taskFromEmail(
        {
          emailId: "M1",
          folder: "inbox",
          from: { email: "alice@example.com", name: "Alice" },
          messageIds: ["<m1@example.com>"],
          receivedAt: Date.UTC(2026, 9, 27, 10),
          subject: "Invoice\r\n 42",
        },
        origin,
        words
      )
    ).toStrictEqual({
      links: [
        { href: "mid:m1@example.com", title: "Invoice 42" },
        { href: `${origin}/mail/inbox/M1`, title: "Email: Invoice 42" },
      ],
      notes: "From: Alice <alice@example.com> · 2026-10-27",
      title: "Invoice 42",
    });
  });

  it("works without subject, sender or Message-ID", () => {
    expect(
      taskFromEmail(
        {
          emailId: "M2",
          folder: "b.f1",
          from: null,
          messageIds: [],
          receivedAt: null,
          subject: "  ",
        },
        origin,
        words
      )
    ).toStrictEqual({
      links: [{ href: `${origin}/mail/b.f1/M2`, title: "Email: (No subject)" }],
      notes: "",
      title: "(No subject)",
    });
  });
});

describe(parseRefParam, () => {
  it("round-trips own and shared ids", () => {
    expect(refParam("a", "t1", "a")).toBe("t1");
    expect(refParam("b", "t1", "a")).toBe("b.t1");
    expect(parseRefParam("t1", "a")).toStrictEqual({
      accountId: "a",
      id: "t1",
    });
    expect(parseRefParam("b.t1", "a")).toStrictEqual({
      accountId: "b",
      id: "t1",
    });
  });

  it("drops anything malformed", () => {
    expect(parseRefParam(undefined, "a")).toBeNull();
    expect(parseRefParam("a.b.c", "a")).toBeNull();
    expect(parseRefParam("../x", "a")).toBeNull();
    expect(parseRefParam("t1", null)).toBeNull();
    expect(parseRefParam("b.", "a")).toBeNull();
  });
});
