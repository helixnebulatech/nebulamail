// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { EMAIL_PUSH_PROPERTIES, emailPushConfig } from "./email-push";

describe(emailPushConfig, () => {
  it("asks for new unread inbox mail of the account", () => {
    expect(emailPushConfig("a", "inbox1")).toStrictEqual({
      a: {
        filter: { inMailbox: "inbox1", notKeyword: "$seen" },
        properties: [...EMAIL_PUSH_PROPERTIES],
        urgency: "normal",
      },
    });
  });

  it("never asks for the body", () => {
    for (const property of [
      "preview",
      "bodyValues",
      "textBody",
      "htmlBody",
      "attachments",
      "headers",
      "bodyStructure",
    ]) {
      expect(EMAIL_PUSH_PROPERTIES).not.toContain(property);
    }
  });
});
