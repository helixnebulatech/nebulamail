// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The page side of the static build's session, with a fake SharedWorker.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { ProviderMetadata } from "./oauth";

const ORIGIN = "https://mail.example";

vi.mock(import("./oauth"), async (importOriginal) => ({
  ...(await importOriginal()),
  fetchMetadata: vi.fn<() => Promise<ProviderMetadata>>().mockResolvedValue({
    authorization_endpoint: `${ORIGIN}/authorize`,
    issuer: ORIGIN,
    token_endpoint: `${ORIGIN}/token`,
  }),
}));

/** The worker's side: answers requests and can send events to the page. */
const fake = {
  onMessage: undefined as ((event: { data: unknown }) => void) | undefined,
  emit(data: unknown): void {
    fake.onMessage?.({ data });
  },
};

const ignore = (): void => {};

class FakeSharedWorker {
  readonly port = {
    addEventListener: (
      _type: string,
      listener: (event: { data: unknown }) => void
    ): void => {
      fake.onMessage = listener;
    },
    postMessage: (message: { id: number; op: string }): void => {
      const value =
        message.op === "grant"
          ? { accessToken: "A", expiresAt: Date.now() + 3_600_000 }
          : null;
      if (message.op === "logout") {
        // Like the worker: the event goes to every tab before the reply.
        queueMicrotask(() => {
          fake.emit({ event: "loggedOut" });
          fake.emit({ id: message.id, ok: true, value });
        });
        return;
      }
      queueMicrotask(() => {
        fake.emit({ id: message.id, ok: true, value });
      });
    },
    start: (): void => {},
  };

  readonly addEventListener = ignore;
}

const assign = vi.fn<(url: string) => void>();

describe("session in the browser", () => {
  beforeEach(() => {
    vi.resetModules();
    assign.mockReset();
    fake.onMessage = undefined;
    vi.stubGlobal("SharedWorker", FakeSharedWorker);
    vi.stubGlobal("document", {});
    vi.stubGlobal("location", {
      assign,
      origin: ORIGIN,
      pathname: "/mail/inbox",
      reload: vi.fn<() => void>(),
      search: "",
    });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("leaves for the login when another tab logs out", async () => {
    const { getGrant } = await import("./browser");
    await getGrant();
    fake.emit({ event: "loggedOut" });
    expect(assign).toHaveBeenCalledWith(
      "/login?error=expired&returnTo=%2Fmail%2Finbox"
    );
  });

  it("doesn't leave on its own logout", async () => {
    const { getGrant, logout } = await import("./browser");
    await getGrant();
    await logout();
    expect(assign).not.toHaveBeenCalled();
  });

  it("does nothing on a page that never showed mail", async () => {
    const { getAuthState } = await import("./browser");
    await getAuthState();
    fake.emit({ event: "loggedOut" });
    expect(assign).not.toHaveBeenCalled();
  });
});
