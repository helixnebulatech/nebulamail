// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// @vitest-environment jsdom

// Scheduled send against a real Stalwart: the advertised
// limit, HOLDUNTIL accepted and pending, the list of scheduled messages,
// changing the time, cancelling (back to Drafts), the real 7-day limit
// learned from the refusal, and the message reaching the recipient at the
// new time. Takes ~3 minutes. Skipped unless these are set (values from
// docker/.env; the recipient's login only to check the arrival):
//
//   SCHEDULE_LIVE_URL=http://localhost:8080 \
//   SCHEDULE_LIVE_USER=alice SCHEDULE_LIVE_PASSWORD=… \
//   SCHEDULE_LIVE_TO=<bob's address> \
//   SCHEDULE_LIVE_TO_USER=bob SCHEDULE_LIVE_TO_PASSWORD=… \
//   bunx vitest run src/mail/schedule.live.test.ts

import { setTimeout as sleep } from "node:timers/promises";

import type { EmailId, FetchLike, JmapClient } from "@nebulamail/jmap-client";
import { JmapClient as Client } from "@nebulamail/jmap-client";
import { beforeAll, describe, expect, it, vi } from "vitest";

const env = {
  password: process.env.SCHEDULE_LIVE_PASSWORD,
  to: process.env.SCHEDULE_LIVE_TO,
  toPassword: process.env.SCHEDULE_LIVE_TO_PASSWORD,
  toUser: process.env.SCHEDULE_LIVE_TO_USER,
  url: process.env.SCHEDULE_LIVE_URL,
  user: process.env.SCHEDULE_LIVE_USER,
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

const connectAs = async (user: string, password: string) =>
  await Client.connect({
    fetch: basicFetch(user, password),
    getAccessToken: async () => await Promise.resolve("basic"),
    sessionUrl: `${env.url ?? ""}/.well-known/jmap`,
  });

const live: { client: JmapClient | null } = { client: null };

vi.mock(import("./client"), () => ({
  getClient: async () => {
    if (live.client === null) {
      throw new Error("not connected");
    }
    return await Promise.resolve(live.client);
  },
}));

const { listIdentities, rescheduleEmail, sendEmail, undoSend } =
  await import("./compose");
const { limitFor, listScheduled } = await import("./schedule");
const { SendFailedError } = await import("./send-failed");

const MINUTE = 60_000;
const DAY = 24 * 60 * MINUTE;

const connected = (): JmapClient => {
  if (live.client === null) {
    throw new Error("not connected");
  }
  return live.client;
};

/** Where an email is now and whether it's a draft again. */
const emailState = async (id: EmailId) => {
  const client = connected();
  const [mailboxes, emails] = await client.request((b) => [
    b.call("Mailbox/get", {
      accountId: client.primaryAccountId(),
      properties: ["role"],
    }),
    b.call("Email/get", {
      accountId: client.primaryAccountId(),
      ids: [id],
      properties: ["mailboxIds", "keywords"],
    }),
  ]);
  const [email] = emails.list;
  const roles = Object.keys(email?.mailboxIds ?? {}).map(
    (box) => mailboxes.list.find((item) => item.id === box)?.role ?? null
  );
  return { draft: email?.keywords?.$draft === true, roles };
};

const destroy = async (id: EmailId): Promise<void> => {
  const client = connected();
  await client.request((b) => [
    b.call("Email/set", {
      accountId: client.primaryAccountId(),
      destroy: [id],
    }),
  ]);
};

/** When the recipient got an email with this subject (polls until `until`). */
const arrival = async (subject: string, until: number): Promise<string> => {
  const bob = await connectAs(env.toUser ?? "", env.toPassword ?? "");
  const accountId = bob.primaryAccountId();
  for (;;) {
    // oxlint-disable-next-line no-await-in-loop -- polling, one at a time.
    const [, got] = await bob.request((b) => {
      const query = b.call("Email/query", { accountId, filter: { subject } });
      const get = b.call("Email/get", {
        "#ids": query.ref("/ids"),
        accountId,
        properties: ["receivedAt"],
      });
      return [query, get] as const;
    });
    const receivedAt = got.list[0]?.receivedAt;
    if (receivedAt !== undefined) {
      return receivedAt;
    }
    if (Date.now() > until) {
      throw new Error("The scheduled message never arrived");
    }
    // oxlint-disable-next-line no-await-in-loop -- polling, one at a time.
    await sleep(5000);
  }
};

const draftTo = async (subject: string) => {
  const [identity] = await listIdentities();
  if (identity === undefined) {
    throw new Error("no identity");
  }
  return {
    attachments: [],
    bcc: [],
    cc: [],
    html: "<p>Scheduled</p>",
    identity,
    subject,
    to: [{ email: env.to ?? "" }],
  };
};

/** Schedules a message for `at`; its sent email. */
const schedule = async (subject: string, at: Date) => {
  const outcome = await sendEmail(await draftTo(subject), { sendAt: at });
  if (outcome.kind === "unknown") {
    throw new Error("unknown outcome");
  }
  return outcome.sent;
};

const scheduledOf = async (emailId: EmailId) => {
  const list = await listScheduled();
  return list.filter((item) => item.emailId === emailId);
};

describe.skipIf(!configured)("live Stalwart: scheduled send", () => {
  // The steps of one message, in order (vitest runs them one by one).
  const subject = `Scheduled live ${Date.now()}`;
  const state: {
    sent: Awaited<ReturnType<typeof schedule>> | null;
    newTime: Date | null;
  } = { newTime: null, sent: null };
  const sentEmail = () => {
    if (state.sent === null) {
      throw new Error("not scheduled");
    }
    return state.sent;
  };

  beforeAll(async () => {
    live.client = await connectAs(env.user ?? "", env.password ?? "");
  });

  it("advertises FUTURERELEASE with 30 days", () => {
    expect(limitFor(connected())).toStrictEqual({
      latest: null,
      maxSeconds: 30 * 86_400,
    });
  });

  it("schedules: pending, out of Drafts, listed with its subject", async () => {
    state.sent = await schedule(subject, new Date(Date.now() + 10 * MINUTE));

    expect(state.sent.scheduled).toBeTruthy();
    await expect(emailState(state.sent.emailId)).resolves.toStrictEqual({
      draft: false,
      roles: ["sent"],
    });
    const [listed] = await scheduledOf(state.sent.emailId);

    expect(listed?.subject).toBe(subject);
    expect(listed?.recipients).toStrictEqual([env.to]);
  });

  it("changes the time: one pending submission, the new one", async () => {
    const [listed] = await scheduledOf(sentEmail().emailId);
    if (listed === undefined) {
      throw new Error("not listed");
    }
    // Two minutes from now: it must not leave at the first time.
    const newTime = new Date(Date.now() + 2 * MINUTE);
    newTime.setMilliseconds(0);
    state.newTime = newTime;
    await rescheduleEmail(listed, newTime);
    const moved = await scheduledOf(sentEmail().emailId);

    expect(moved).toHaveLength(1);
    expect(moved[0]?.submissionId).not.toBe(listed.submissionId);
    expect(Date.parse(moved[0]?.sendAt ?? "")).toBe(newTime.getTime());
  });

  it(
    "reaches the recipient at the new time, and leaves the list",
    { timeout: 6 * MINUTE },
    async () => {
      const at = state.newTime?.getTime() ?? 0;
      const receivedAt = await arrival(subject, Date.now() + 5 * MINUTE);
      const late = Date.parse(receivedAt) - at;

      // Stalwart writes receivedAt in whole seconds; its queue may take a
      // few seconds more.
      expect(late).toBeGreaterThanOrEqual(-1000);
      expect(late).toBeLessThan(MINUTE);
      await expect(scheduledOf(sentEmail().emailId)).resolves.toHaveLength(0);
    }
  );

  it("cancels: back to Drafts and out of the list", async () => {
    const sent = await schedule("Cancel live", new Date(Date.now() + DAY));
    await undoSend(sent);

    await expect(emailState(sent.emailId)).resolves.toStrictEqual({
      draft: true,
      roles: ["drafts"],
    });
    await expect(scheduledOf(sent.emailId)).resolves.toHaveLength(0);
    await destroy(sent.emailId);
  });

  it("learns the queue's real limit (7 days) from the refusal", async () => {
    const error = await sendEmail(await draftTo("Too far live"), {
      sendAt: new Date(Date.now() + 8 * DAY),
    }).catch((error_: unknown) => error_);

    expect(error).toBeInstanceOf(SendFailedError);
    const seconds = limitFor(connected())?.maxSeconds ?? 0;

    expect(Math.abs(seconds * 1000 - 7 * DAY)).toBeLessThan(MINUTE);
    if (error instanceof SendFailedError && error.draftId !== null) {
      await destroy(error.draftId);
    }
  });
});
