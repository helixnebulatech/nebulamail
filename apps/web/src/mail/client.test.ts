// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { afterEach, describe, expect, it, vi } from "vitest";

import { fetchWithLanguage } from "./client";

describe(fetchWithLanguage, () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("sends the interface language and keeps the other headers", async () => {
    const seen: Headers[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (_input: URL | string, init?: RequestInit) => {
        seen.push(new Headers(init?.headers));
        return await Promise.resolve(new Response("{}"));
      })
    );
    let language = "es";
    const send = fetchWithLanguage(() => language);
    await send("https://jmap.example/api", {
      headers: { authorization: "Bearer x", "accept-language": "en-US" },
      method: "POST",
    });
    language = "en";
    await send("https://jmap.example/api", { method: "GET" });
    expect(seen[0]?.get("accept-language")).toBe("es");
    expect(seen[0]?.get("authorization")).toBe("Bearer x");
    expect(seen[1]?.get("accept-language")).toBe("en");
  });
});
