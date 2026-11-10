// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { setTimeout as sleep } from "node:timers/promises";

import { describe, expect, it, vi } from "vitest";

import { scriptedJmap } from "../test/scripted-jmap";
import { OUR_SCRIPT_NAME } from "./filters/generate";
import { cleanScriptName, loadScripts, scriptNameProblem } from "./scripts";

/** Invisible characters, written as code points so they stay visible. */
const RLO = String.fromCodePoint(0x20_2e);
const LINE_SEPARATOR = String.fromCodePoint(0x20_28);
const PARAGRAPH_SEPARATOR = String.fromCodePoint(0x20_29);
const ZERO_WIDTH = String.fromCodePoint(0x20_0b);
const NUL = String.fromCodePoint(0);

describe("script names", () => {
  const others = [{ name: "Work" }, { name: "nebula-mail-filters" }];

  it("accepts a new, clean name", () => {
    expect(scriptNameProblem("Holidays", others)).toBeNull();
  });

  it("refuses empty, taken and reserved names", () => {
    expect(scriptNameProblem("  ", others)).toBe("empty");
    expect(scriptNameProblem("work", others)).toBe("taken");
    expect(scriptNameProblem("Vacation", others)).toBe("reserved");
    expect(scriptNameProblem("nebula-mail-filters", [])).toBe("reserved");
  });

  it("measures the length in bytes", () => {
    expect(scriptNameProblem("ñ".repeat(300), others)).toBe("tooLong");
    expect(scriptNameProblem("abcd", others, 3)).toBe("tooLong");
  });

  it("removes control, format and line separator characters", () => {
    expect(cleanScriptName(` A\nB${RLO}C${LINE_SEPARATOR}D${NUL} `)).toBe(
      "ABCD"
    );
    // Only invisible characters: nothing is left.
    expect(
      scriptNameProblem(`${ZERO_WIDTH}${PARAGRAPH_SEPARATOR}`, others)
    ).toBe("empty");
  });
});

const jmap = vi.hoisted(() => ({
  connect: null as null | (() => Promise<unknown>),
}));

vi.mock(import("../mail/client"), async () => {
  const { JmapClient } = await import("@nebulamail/jmap-client");
  return {
    getClient: async () => {
      const client = await jmap.connect?.();
      if (!(client instanceof JmapClient)) {
        throw new TypeError("no scripted client");
      }
      return client;
    },
    resetClient: vi.fn<() => void>(),
  };
});

describe(loadScripts, () => {
  it("downloads ours and another active script at once", async () => {
    const scripted = scriptedJmap({
      handlers: {
        "SieveScript/get": (_args, callId) => [
          [
            "SieveScript/get",
            {
              accountId: "a",
              list: [
                {
                  blobId: "b-ours",
                  id: "s1",
                  isActive: false,
                  name: OUR_SCRIPT_NAME,
                },
                { blobId: "b-other", id: "s2", isActive: true, name: "Mine" },
              ],
              notFound: [],
              state: "1",
            },
            callId,
          ],
        ],
      },
    });
    const client = await scripted.connect();
    jmap.connect = async () => await Promise.resolve(client);
    let open = 0;
    let most = 0;
    const texts: Record<string, string> = {
      "b-other": "keep;\r\n",
      "b-ours": "# edited by hand\r\nkeep;\r\n",
    };
    vi.spyOn(client, "download").mockImplementation(async ({ blobId }) => {
      open += 1;
      most = Math.max(most, open);
      await sleep(5);
      open -= 1;
      return new Response(texts[blobId] ?? "");
    });
    const state = await loadScripts();
    expect(most).toBe(2);
    expect(state.active?.name).toBe("Mine");
    expect(state.ours?.name).toBe(OUR_SCRIPT_NAME);
    expect(state.rawContent).toBe("keep;\r\n");
  });
});
