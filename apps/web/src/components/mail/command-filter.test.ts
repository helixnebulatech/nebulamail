// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { matchesQuery } from "./command-filter";

describe(matchesQuery, () => {
  const read = { label: "Marcar como leído" };
  const theme = { keywords: "theme tema dark", label: "Tema: Oscuro" };

  it("matches every word, in any order, ignoring case and accents", () => {
    expect(matchesQuery(read, "LEIDO marcar")).toBeTruthy();
    expect(matchesQuery(read, "marcar spam")).toBeFalsy();
  });

  it("also looks at the keywords", () => {
    expect(matchesQuery(theme, "dark")).toBeTruthy();
  });

  it("matches everything for an empty query", () => {
    expect(matchesQuery(read, "   ")).toBeTruthy();
  });
});
