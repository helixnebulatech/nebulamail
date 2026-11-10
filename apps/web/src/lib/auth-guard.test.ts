// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { isRedirect } from "@tanstack/react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { markAuthChecked, requireAuth, resetAuthCheck } from "./auth-guard";

const here = { href: "/mail/inbox" };

const signedIn = (authenticated: boolean) =>
  vi.fn<() => Promise<{ readonly authenticated: boolean }>>(
    async () => await Promise.resolve({ authenticated })
  );

describe(requireAuth, () => {
  beforeEach(() => {
    vi.stubGlobal("document", {});
  });

  afterEach(() => {
    resetAuthCheck();
    vi.unstubAllGlobals();
  });

  it("asks once per page, for every section", async () => {
    const check = signedIn(true);
    await expect(requireAuth(here, check)).resolves.toStrictEqual({
      authChecked: true,
    });
    await requireAuth({ href: "/calendar" }, check);
    await requireAuth({ href: "/settings" }, check);
    expect(check).toHaveBeenCalledOnce();
  });

  it("shares one request between concurrent checks (preloads)", async () => {
    const check = signedIn(true);
    await Promise.all([
      requireAuth(here, check),
      requireAuth({ href: "/tasks" }, check),
    ]);
    expect(check).toHaveBeenCalledOnce();
  });

  it("sends a signed-out visitor to the login, and asks again", async () => {
    const check = signedIn(false);
    const error = await requireAuth(here, check).catch(
      (error_: unknown) => error_
    );
    expect(isRedirect(error)).toBeTruthy();
    await requireAuth(here, check).catch(() => null);
    expect(check).toHaveBeenCalledTimes(2);
  });

  it("trusts the server render's check", async () => {
    const check = signedIn(false);
    markAuthChecked();
    await requireAuth(here, check);
    expect(check).not.toHaveBeenCalled();
  });

  it("checks every request on the server", async () => {
    vi.stubGlobal("document", undefined);
    const check = signedIn(true);
    markAuthChecked();
    await requireAuth(here, check);
    await requireAuth(here, check);
    expect(check).toHaveBeenCalledTimes(2);
  });
});
