// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import {
  EMPTY_DOC,
  mergeDocs,
  parseDoc,
  sameDoc,
  serializeDoc,
  valuesOf,
  withValue,
} from "./model";
import { readSyncMessage, syncMessage } from "./remote";

describe("synced preferences document", () => {
  it("round-trips and keeps options it doesn't know", () => {
    const doc = withValue(EMPTY_DOC, "theme", "dark", 5);
    const text = serializeDoc({
      values: { ...doc.values, futureOption: { at: 3, value: [1, 2] } },
    });
    const back = parseDoc(text);
    expect(back?.values.futureOption).toStrictEqual({ at: 3, value: [1, 2] });
    expect(valuesOf(back ?? EMPTY_DOC)).toStrictEqual({ theme: "dark" });
  });

  it("drops bad values and refuses other files", () => {
    expect(parseDoc("not json")).toBeNull();
    expect(parseDoc('{"values":{}}')).toBeNull();
    const doc = parseDoc(
      JSON.stringify({
        format: "nebula-mail-preferences",
        values: {
          __proto__: { at: 1, value: 1 },
          density: { at: -1, value: "compact" },
          pane: { at: 1, value: "side" },
          theme: { at: 1, value: "neon" },
        },
      })
    );
    expect(doc?.values).toStrictEqual({ pane: { at: 1, value: "side" } });
  });

  it("keeps the newest change of each option", () => {
    const local = withValue(
      withValue(EMPTY_DOC, "theme", "dark", 20),
      "density",
      "compact",
      0
    );
    const remote = withValue(
      withValue(EMPTY_DOC, "theme", "light", 10),
      "density",
      "comfortable",
      15
    );
    const merged = mergeDocs(local, remote);
    expect(valuesOf(merged)).toStrictEqual({
      density: "comfortable",
      theme: "dark",
    });
    // On a tie, the server's copy wins.
    const tie = mergeDocs(
      withValue(EMPTY_DOC, "pane", "full", 0),
      withValue(EMPTY_DOC, "pane", "side", 0)
    );
    expect(valuesOf(tie).pane).toBe("side");
  });

  it("compares documents regardless of key order", () => {
    const a = withValue(
      withValue(EMPTY_DOC, "theme", "dark", 1),
      "pane",
      "full",
      2
    );
    const b = withValue(
      withValue(EMPTY_DOC, "pane", "full", 2),
      "theme",
      "dark",
      1
    );
    expect(sameDoc(a, b)).toBeTruthy();
    expect(sameDoc(a, withValue(b, "pane", "side", 3))).toBeFalsy();
  });

  it("stores the paused script", () => {
    const doc = withValue(
      EMPTY_DOC,
      "vacationPausedScript",
      { id: "s1", name: "Ñandú «filtros»" },
      1
    );
    expect(valuesOf(parseDoc(serializeDoc(doc)) ?? EMPTY_DOC)).toStrictEqual({
      vacationPausedScript: { id: "s1", name: "Ñandú «filtros»" },
    });
  });
});

describe("mail fallback message", () => {
  it("carries the JSON (any text) in base64 and reads it back", () => {
    const doc = withValue(
      EMPTY_DOC,
      "vacationPausedScript",
      { id: "x", name: "Größe 🌌" },
      7
    );
    const message = syncMessage(doc, new Date("2026-10-26T10:00:00Z"));
    expect(message).toContain("Content-Transfer-Encoding: base64\r\n");
    expect(message).toContain("Date: Mon, 26 Oct 2026 10:00:00 +0000\r\n");
    // No raw non-ASCII in the message.
    expect(/^[ -~\r\n]*$/u.test(message)).toBeTruthy();
    expect(readSyncMessage(message)).toStrictEqual(doc);
    expect(readSyncMessage("Subject: x\r\n\r\n!!!")).toBeNull();
  });
});
