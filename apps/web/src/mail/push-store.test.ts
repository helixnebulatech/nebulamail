// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import "fake-indexeddb/auto";
import { beforeEach, describe, expect, it } from "vitest";

import {
  claimNotice,
  forgetPushStore,
  mailPushOn,
  mailState,
  pruneNotices,
  pushAccountKey,
  seedMailState,
  setMailPushOn,
} from "./push-store";

const ALICE = pushAccountKey("https://mail.test", "alice", "c");
const BOB = pushAccountKey("https://mail.test", "bob", "c");

describe("new-mail push store", () => {
  beforeEach(async () => {
    await forgetPushStore();
  });

  it("is off until the user turns it on", async () => {
    await expect(mailPushOn()).resolves.toBeFalsy();
    await setMailPushOn(true);
    await expect(mailPushOn()).resolves.toBeTruthy();
    await setMailPushOn(false);
    await expect(mailPushOn()).resolves.toBeFalsy();
  });

  it("seeds a state only once, per account", async () => {
    await seedMailState(ALICE, "s1");
    await seedMailState(ALICE, "s2");
    await seedMailState(BOB, "b1");
    await expect(mailState(ALICE)).resolves.toBe("s1");
    await expect(mailState(BOB)).resolves.toBe("b1");
    await expect(
      mailState(pushAccountKey("https://other.test", "alice", "c"))
    ).resolves.toBeNull();
  });

  it("announces each email once, even when claimed at the same time", async () => {
    const claims = await Promise.all([
      claimNotice(ALICE, "e1"),
      claimNotice(ALICE, "e1"),
      claimNotice(ALICE, "e1"),
    ]);
    expect(claims.filter(Boolean)).toHaveLength(1);
    await expect(claimNotice(BOB, "e1")).resolves.toBeTruthy();
    await expect(claimNotice(ALICE, "e2")).resolves.toBeTruthy();
  });

  it("forgets old claims", async () => {
    const now = Date.now();
    await claimNotice(ALICE, "old", now - 3 * 24 * 60 * 60_000);
    await claimNotice(ALICE, "new", now);
    await pruneNotices(now);
    await expect(claimNotice(ALICE, "old", now)).resolves.toBeTruthy();
    await expect(claimNotice(ALICE, "new", now)).resolves.toBeFalsy();
  });

  it("logout forgets everything", async () => {
    await setMailPushOn(true);
    await seedMailState(ALICE, "s1");
    await claimNotice(ALICE, "e1");
    await forgetPushStore();
    await expect(mailPushOn()).resolves.toBeFalsy();
    await expect(mailState(ALICE)).resolves.toBeNull();
    await expect(claimNotice(ALICE, "e1")).resolves.toBeTruthy();
  });
});
