// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Several Sieve scripts against a real Stalwart: create,
// rename, activate, deactivate and delete (also the active one), with the
// functions the settings page uses. Skipped unless these are set (values
// from docker/.env), like settings/filters/live.test.ts:
//
//   SIEVE_LIVE_URL=http://localhost:8080 \
//   SIEVE_LIVE_USER=bob@nebulamail.test SIEVE_LIVE_PASSWORD=… bun run test
//
// It leaves the account as it found it (the active script comes back).

import type {
  FetchLike,
  JmapClient,
  SieveScriptId,
} from "@nebulamail/jmap-client";
import {
  JmapClient as Client,
  SieveScriptId as SieveScriptIdSchema,
} from "@nebulamail/jmap-client";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";

const env = {
  password: process.env.SIEVE_LIVE_PASSWORD,
  url: process.env.SIEVE_LIVE_URL,
  user: process.env.SIEVE_LIVE_USER,
};
const configured = Object.values(env).every(
  (value) => value !== undefined && value !== ""
);

const basicFetch =
  (user: string, password: string): FetchLike =>
  async (input, init) => {
    const headers = new Headers(init?.headers);
    headers.set("Authorization", `Basic ${btoa(`${user}:${password}`)}`);
    return await fetch(input, { ...init, headers });
  };

const live: { client: JmapClient | null } = { client: null };

vi.mock(import("../mail/client"), () => ({
  getClient: async () => {
    if (live.client === null) {
      throw new Error("not connected");
    }
    return await Promise.resolve(live.client);
  },
}));

const scripts = await import("./scripts");

describe.skipIf(!configured)("live Stalwart: several scripts", () => {
  const token = `nebula-live-${Date.now().toString(36)}`;
  let previouslyActive: SieveScriptId | null = null;

  beforeAll(async () => {
    live.client = await Client.connect({
      fetch: basicFetch(env.user ?? "", env.password ?? ""),
      getAccessToken: async () => await Promise.resolve("basic"),
      sessionUrl: `${env.url ?? ""}/.well-known/jmap`,
    });
    const state = await scripts.loadScripts();
    previouslyActive = state.active?.id ?? null;
  });

  afterAll(async () => {
    if (live.client === null) {
      return;
    }
    const state = await scripts.loadScripts();
    for (const script of state.scripts) {
      if (script.name.startsWith(token)) {
        // oxlint-disable-next-line no-await-in-loop -- cleanup, one by one.
        await scripts.deleteScript(script);
      }
    }
    if (previouslyActive !== null) {
      await scripts.activateScript(previouslyActive);
    }
  });

  it("creates, renames, activates and deletes scripts", async () => {
    await scripts.saveScript({
      activate: false,
      content: scripts.NEW_SCRIPT_TEMPLATE,
      id: null,
      name: `${token} a`,
    });
    let state = await scripts.loadScripts();
    const created = state.scripts.find((item) => item.name === `${token} a`);
    expect(created?.isActive).toBeFalsy();
    if (created === undefined) {
      return;
    }
    // Saved with CRLF (`withCrlf`, RFC 5228 §8.1); Stalwart 1.1.0 gives
    // those bytes back, so compare without the kind of line break.
    const text = await scripts.readScriptText(created);
    expect(text.replaceAll("\r\n", "\n")).toBe(scripts.NEW_SCRIPT_TEMPLATE);

    await scripts.renameScript(created.id, `${token} b`);
    await scripts.activateScript(created.id);
    state = await scripts.loadScripts();
    expect(state.active).toMatchObject({ id: created.id, name: `${token} b` });
  });

  it("switches every script off, and deletes the active one", async () => {
    const before = await scripts.loadScripts();
    const created = before.scripts.find((item) => item.name === `${token} b`);
    expect(created?.isActive).toBeTruthy();
    // Only "switch everything off", without other changes.
    await scripts.activateScript(null);
    const off = await scripts.loadScripts();
    expect(off.active).toBeNull();
    // The active one is switched off and deleted in one request.
    await scripts.activateScript(created?.id ?? null);
    await scripts.deleteScript({
      id: created?.id ?? SieveScriptIdSchema.parse("missing"),
      isActive: true,
    });
    const state = await scripts.loadScripts();
    expect(state.scripts.some((item) => item.id === created?.id)).toBeFalsy();
    expect(state.active).toBeNull();
  });
});
