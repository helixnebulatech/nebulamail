// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The folders' request started on the page's first load (router.tsx
// `hydrate`): the folders' query takes it once, for the same account and
// only while it is recent, and nothing reaches React Query's cache before
// the query runs (a hydration mismatch otherwise).

import { AccountId } from "@nebulamail/jmap-client";
import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { MailboxSummary } from "./api";

const api = vi.hoisted(() => ({
  listMailboxes: vi.fn<(account?: unknown) => Promise<MailboxSummary[]>>(),
}));

vi.mock(import("./api"), async (original) => ({
  ...(await original()),
  listMailboxes: api.listMailboxes,
}));

const answer = (name: string): MailboxSummary[] => [
  // Only what the test compares; the query passes it on as it is.
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- a partial summary is enough here.
  { name } as unknown as MailboxSummary,
];

const load = async () => await import("./queries");

describe("startMailboxesEarly", () => {
  beforeEach(() => {
    vi.resetModules();
    api.listMailboxes.mockReset();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("hands its answer to the first folders' query, once", async () => {
    const { mailboxesQuery, startMailboxesEarly } = await load();
    api.listMailboxes.mockResolvedValueOnce(answer("early"));
    api.listMailboxes.mockResolvedValue(answer("later"));
    const client = new QueryClient();

    startMailboxesEarly();
    // Nothing in the cache until the query itself runs.
    expect(client.getQueryData(mailboxesQuery().queryKey)).toBeUndefined();

    await expect(client.query(mailboxesQuery())).resolves.toStrictEqual(
      answer("early")
    );
    await expect(
      client.query({ ...mailboxesQuery(), staleTime: 0 })
    ).resolves.toStrictEqual(answer("later"));
    expect(api.listMailboxes).toHaveBeenCalledTimes(2);
  });

  it("isn't used for another account", async () => {
    const { mailboxesQuery, startMailboxesEarly } = await load();
    api.listMailboxes.mockResolvedValueOnce(answer("own"));
    api.listMailboxes.mockResolvedValue(answer("shared"));

    startMailboxesEarly();

    await expect(
      new QueryClient().query(mailboxesQuery(AccountId.parse("b")))
    ).resolves.toStrictEqual(answer("shared"));
  });

  it("isn't used once it is old", async () => {
    vi.useFakeTimers();
    const { mailboxesQuery, startMailboxesEarly } = await load();
    api.listMailboxes.mockResolvedValueOnce(answer("early"));
    api.listMailboxes.mockResolvedValue(answer("fresh"));

    startMailboxesEarly();
    vi.advanceTimersByTime(11_000);

    await expect(
      new QueryClient().query(mailboxesQuery())
    ).resolves.toStrictEqual(answer("fresh"));
  });
});
