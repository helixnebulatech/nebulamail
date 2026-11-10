// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Signing in on the bridge's own page, and a JMAP client for the test
// itself with the signed-in page's access token (the one the app uses).

import { JmapClient } from "@nebulamail/jmap-client";
import type { Page } from "@playwright/test";
import { z } from "zod";

export const signIn = async (page: Page, user: string, password: string) => {
  await page.goto("/login");
  await page.getByRole("button", { exact: true, name: "Sign in" }).click();
  // The bridge's sign-in page (another origin).
  await page.getByRole("textbox", { name: "Username or email" }).fill(user);
  await page.getByRole("textbox", { name: "Password" }).fill(password);
  await page.getByRole("button", { exact: true, name: "Sign in" }).click();
};

const Grant = z.object({ accessToken: z.string(), sessionUrl: z.url() });

/** JMAP straight to the bridge, as the user signed in on `page`. */
export const jmapFor = async (page: Page): Promise<JmapClient> => {
  const grant = async (refresh: boolean) => {
    const body: unknown = await page.evaluate(async (forceRefresh) => {
      const response = await fetch(
        forceRefresh ? "/auth/token?refresh=1" : "/auth/token",
        { credentials: "same-origin", method: "POST" }
      );
      if (!response.ok) {
        throw new Error(`Token request failed: HTTP ${response.status}`);
      }
      const json: unknown = await response.json();
      return json;
    }, refresh);
    return Grant.parse(body);
  };
  const { sessionUrl } = await grant(false);
  return await JmapClient.connect({
    getAccessToken: async ({ forceRefresh }) => {
      const next = await grant(forceRefresh);
      return next.accessToken;
    },
    sessionUrl,
  });
};
