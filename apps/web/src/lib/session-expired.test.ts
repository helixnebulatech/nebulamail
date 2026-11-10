// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { QueryClient } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";

import { SessionExpiredError } from "../session/types";
import {
  expiredLoginUrl,
  leaveForExpiredLogin,
  resetLeaving,
  routerPath,
  watchSession,
} from "./session-expired";

const page = (pathname: string, search = "") => ({
  assign: vi.fn<(url: string) => void>(),
  pathname,
  search,
});

describe(routerPath, () => {
  it("drops the static build's base", () => {
    expect(
      routerPath(page("/nebulamail/calendar", "?d=1"), "/nebulamail/")
    ).toBe("/calendar?d=1");
    expect(routerPath(page("/nebulamail", ""), "/nebulamail/")).toBe("/");
    expect(routerPath(page("/nebulamailx/a", ""), "/nebulamail/")).toBe(
      "/nebulamailx/a"
    );
  });

  it("keeps the path with the root base", () => {
    expect(routerPath(page("/tasks", "?x=1"), "/")).toBe("/tasks?x=1");
  });
});

describe(leaveForExpiredLogin, () => {
  afterEach(() => {
    resetLeaving();
  });

  it("goes to the base's login with the router's path", () => {
    const location = page("/nebulamail/calendar");
    leaveForExpiredLogin(location, "/nebulamail/");
    expect(location.assign).toHaveBeenCalledWith(
      "/nebulamail/login?error=expired&returnTo=%2Fcalendar"
    );
    expect(expiredLoginUrl("/tasks", "/")).toBe(
      "/login?error=expired&returnTo=%2Ftasks"
    );
  });

  it("stays on the login, and leaves only once", () => {
    const login = page("/nebulamail/login", "?error=expired");
    leaveForExpiredLogin(login, "/nebulamail/");
    expect(login.assign).not.toHaveBeenCalled();
    const location = page("/mail/inbox");
    leaveForExpiredLogin(location, "/");
    leaveForExpiredLogin(location, "/");
    expect(location.assign).toHaveBeenCalledOnce();
  });
});

describe(watchSession, () => {
  it("reacts to a query that fails with SessionExpiredError", async () => {
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const onExpired = vi.fn<() => void>();
    const stop = watchSession(queryClient, onExpired);
    await queryClient
      .query({
        queryFn: () => {
          throw new SessionExpiredError("gone");
        },
        queryKey: ["x"],
      })
      .catch(() => null);
    expect(onExpired).toHaveBeenCalledWith();
    stop();
  });

  it("reacts to a mutation that fails with SessionExpiredError", async () => {
    const queryClient = new QueryClient();
    const onExpired = vi.fn<() => void>();
    const stop = watchSession(queryClient, onExpired);
    await queryClient
      .getMutationCache()
      .build(queryClient, {
        mutationFn: () => {
          throw new SessionExpiredError("gone");
        },
      })
      .execute(undefined)
      .catch(() => null);
    expect(onExpired).toHaveBeenCalledWith();
    stop();
  });

  it("ignores other errors", async () => {
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const onExpired = vi.fn<() => void>();
    const stop = watchSession(queryClient, onExpired);
    await queryClient
      .query({
        queryFn: () => {
          throw new Error("network");
        },
        queryKey: ["y"],
      })
      .catch(() => null);
    expect(onExpired).not.toHaveBeenCalled();
    stop();
  });
});
