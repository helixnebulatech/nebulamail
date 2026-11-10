// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { describe, expect, it } from "vitest";

import { MailboxId } from "./ids";
import { keywordPatch, mailboxPatch } from "./patch";
import {
  encodeTemplateValue,
  escapePointerSegment,
  expandTemplate,
  templateVariables,
} from "./url";

const DOWNLOAD =
  "https://mail.example.com/jmap/download/{accountId}/{blobId}/{name}?accept={type}";

describe(expandTemplate, () => {
  it("expands the Stalwart download template", () => {
    expect(
      expandTemplate(DOWNLOAD, {
        accountId: "d",
        blobId: "Gabc_-1",
        name: "Factura marzo.pdf",
        type: "application/pdf",
      })
    ).toBe(
      "https://mail.example.com/jmap/download/d/Gabc_-1/Factura%20marzo.pdf?accept=application%2Fpdf"
    );
  });

  it("encodes path separators, queries and reserved characters", () => {
    expect(
      expandTemplate(DOWNLOAD, {
        accountId: "d",
        blobId: "b",
        name: "../../x?y=1&z#w",
        type: "text/plain; charset=utf-8",
      })
    ).toBe(
      "https://mail.example.com/jmap/download/d/b/..%2F..%2Fx%3Fy%3D1%26z%23w?accept=text%2Fplain%3B%20charset%3Dutf-8"
    );
  });

  it("leaves undefined variables empty", () => {
    expect(expandTemplate("/u/{accountId}/{missing}", { accountId: "a" })).toBe(
      "/u/a/"
    );
  });
});

describe(encodeTemplateValue, () => {
  it.each([
    ["it's (a) *test*!", "it%27s%20%28a%29%20%2Atest%2A%21"],
    ["año €", "a%C3%B1o%20%E2%82%AC"],
    ["~-._", "~-._"],
    ["\uD800x", "%EF%BF%BDx"],
    ["*", "%2A"],
  ])("encodes %j as %s", (input, expected) => {
    expect(encodeTemplateValue(input)).toBe(expected);
  });
});

describe(templateVariables, () => {
  it("lists level-1 variables", () => {
    expect(templateVariables(DOWNLOAD)).toStrictEqual([
      "accountId",
      "blobId",
      "name",
      "type",
    ]);
  });

  it.each(["/{+path}", "/{a,b}", "/{a", "/a}", "/{}", "/{{a}}"])(
    "rejects %s",
    (template) => {
      expect(templateVariables(template)).toBeUndefined();
    }
  );
});

describe("patch helpers", () => {
  it("escapes JSON Pointer segments", () => {
    expect(escapePointerSegment("a/b~c")).toBe("a~1b~0c");
    expect(keywordPatch({ "$x/y": true, $seen: false })).toStrictEqual({
      "keywords/$seen": null,
      "keywords/$x~1y": true,
    });
  });

  it("adds and removes mailboxes", () => {
    expect(
      mailboxPatch({
        add: [MailboxId.parse("b")],
        remove: [MailboxId.parse("a")],
      })
    ).toStrictEqual({ "mailboxIds/a": null, "mailboxIds/b": true });
  });
});
