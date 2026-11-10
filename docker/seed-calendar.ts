// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Calendar test data:
//   bun run dev:seed-calendar           # events and invitations for alice
//   bun run dev:seed-calendar --smtp    # also an outside invitation by SMTP
//   bun run dev:seed-calendar --remove  # delete what this script made
//
// For alice@nebulamail.test, in the current week (Europe/Madrid):
// - her own events: a weekly meeting, a dentist appointment, an all-day
//   trip, a lunch with a link, a meeting with an HTML description (with a
//   remote image and a script, to see the sanitizer) in New York time;
// - bob → alice invitations sent by Stalwart itself (iTIP over email): one
//   to answer, one that bob then cancels; and one of alice's that bob
//   accepts (a REPLY in her inbox);
// - an invitation from outside (carol@partner.example) with an .ics file,
//   imported into her inbox: Stalwart doesn't add it to the calendar, so
//   the card offers Accept / Maybe / Decline. With --smtp the same comes in
//   through SMTP (127.0.0.1:2525; Stalwart takes a minute or two).
//
// Every event has a UID starting with "nebula-seed-", so running it again
// only adds what is missing.

import { existsSync, readFileSync } from "node:fs";
import { connect } from "node:net";
import path from "node:path";
import { setTimeout as sleep } from "node:timers/promises";

import { z } from "zod";

const ROOT = path.join(import.meta.dirname, "..");
const STALWART_URL = "http://127.0.0.1:8080";
const DOMAIN = "nebulamail.test";
const ZONE = "Europe/Madrid";
const PREFIX = "nebula-seed-";
const OUTSIDE_UID = `${PREFIX}partner-kickoff@partner.example`;
const OUTSIDE_MESSAGE_ID = `${PREFIX}partner-kickoff@partner.example`;
const USING = [
  "urn:ietf:params:jmap:core",
  "urn:ietf:params:jmap:mail",
  "urn:ietf:params:jmap:submission",
  "urn:ietf:params:jmap:calendars",
];

const args = new Set(process.argv.slice(2));

const passwords = (() => {
  const file = path.join(ROOT, "docker/.env");
  const lines = existsSync(file) ? readFileSync(file, "utf-8").split("\n") : [];
  const read = (key: string): string => {
    const line = lines.find((entry) => entry.startsWith(`${key}=`));
    if (line === undefined) {
      throw new Error(`No ${key} in docker/.env: run bun run dev:setup`);
    }
    return line.slice(key.length + 1);
  };
  return {
    alice: read("TEST_ALICE_PASSWORD"),
    bob: read("TEST_BOB_PASSWORD"),
  };
})();

type User = keyof typeof passwords;

const Responses = z.object({
  methodResponses: z.array(
    z.tuple([z.string(), z.record(z.string(), z.unknown()), z.string()])
  ),
});

type Call = readonly [string, Record<string, unknown>];

const authorization = (user: User): string =>
  `Basic ${btoa(`${user}@${DOMAIN}:${passwords[user]}`)}`;

/** Runs JMAP calls as a user; fails on method errors and rejected objects. */
const jmap = async (
  user: User,
  calls: readonly Call[]
): Promise<Record<string, unknown>[]> => {
  const response = await fetch(`${STALWART_URL}/jmap/`, {
    body: JSON.stringify({
      methodCalls: calls.map(([name, callArgs], index) => [
        name,
        callArgs,
        String(index),
      ]),
      using: USING,
    }),
    headers: {
      authorization: authorization(user),
      "content-type": "application/json",
    },
    method: "POST",
  });
  if (!response.ok) {
    throw new Error(`JMAP HTTP ${response.status}: ${await response.text()}`);
  }
  const { methodResponses } = Responses.parse(await response.json());
  return methodResponses.map(([name, result]) => {
    if (name === "error") {
      throw new Error(`JMAP error: ${JSON.stringify(result)}`);
    }
    for (const key of ["notCreated", "notUpdated", "notDestroyed"]) {
      const failed = result[key];
      if (
        typeof failed === "object" &&
        failed !== null &&
        Object.keys(failed).length > 0
      ) {
        throw new Error(`${name} ${key}: ${JSON.stringify(failed)}`);
      }
    }
    return result;
  });
};

const one = <T>(items: readonly T[]): T => {
  const [item] = items;
  if (item === undefined) {
    throw new Error("Empty JMAP response");
  }
  return item;
};

const Ids = z.object({ ids: z.array(z.string()) });
const Session = z.object({
  primaryAccounts: z.object({
    "urn:ietf:params:jmap:calendars": z.string(),
    "urn:ietf:params:jmap:mail": z.string(),
  }),
});

interface Account {
  readonly mail: string;
  readonly calendars: string;
  readonly calendar: string;
}

const account = async (user: User): Promise<Account> => {
  const response = await fetch(`${STALWART_URL}/jmap/session`, {
    headers: { authorization: authorization(user) },
  });
  const { primaryAccounts } = Session.parse(await response.json());
  const calendars = primaryAccounts["urn:ietf:params:jmap:calendars"];
  const [result] = await jmap(user, [
    ["Calendar/get", { accountId: calendars }],
  ]);
  const { list } = z
    .object({
      list: z.array(z.object({ id: z.string(), isDefault: z.boolean() })),
    })
    .parse(result);
  const calendar = list.find((item) => item.isDefault) ?? one(list);
  return {
    calendar: calendar.id,
    calendars,
    mail: primaryAccounts["urn:ietf:params:jmap:mail"],
  };
};

// --- Dates (the current week, Monday first) ---------------------------------

const pad = (value: number): string => String(value).padStart(2, "0");

const monday = (() => {
  const now = new Date();
  const day = (now.getUTCDay() + 6) % 7;
  return new Date(
    Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate() - day)
  );
})();

/** "YYYY-MM-DD" of the current week's Monday + `days`. */
const dayOf = (days: number): string => {
  const date = new Date(monday.getTime() + days * 86_400_000);
  return `${date.getUTCFullYear()}-${pad(date.getUTCMonth() + 1)}-${pad(date.getUTCDate())}`;
};

const at = (days: number, time: string): string => `${dayOf(days)}T${time}:00`;

/** A day at least two days from now, on `weekday` (0 = Monday) of this or next week. */
const ahead = (weekday: number): number => {
  const today = Math.floor((Date.now() - monday.getTime()) / 86_400_000);
  return weekday >= today + 2 ? weekday : weekday + 7;
};

// --- Events -----------------------------------------------------------------

const participant = (
  user: string,
  name: string,
  organizer: boolean,
  status = organizer ? "accepted" : "needs-action"
) => ({
  "@type": "Participant",
  calendarAddress: `mailto:${user}@${DOMAIN}`,
  name,
  participationStatus: status,
  roles: organizer ? { attendee: true, owner: true } : { attendee: true },
  ...(organizer ? {} : { expectReply: true }),
});

const ownEvents = (): Record<string, Record<string, unknown>> => ({
  [`${PREFIX}weekly-sync`]: {
    alerts: {
      a1: {
        "@type": "Alert",
        action: "display",
        trigger: {
          "@type": "OffsetTrigger",
          offset: "-PT10M",
          relativeTo: "start",
        },
      },
    },
    duration: "PT45M",
    locations: { l1: { "@type": "Location", name: "Sala Andrómeda" } },
    recurrenceRule: {
      "@type": "RecurrenceRule",
      count: 12,
      frequency: "weekly",
    },
    start: at(0, "10:00"),
    timeZone: ZONE,
    title: "Weekly team sync",
  },
  [`${PREFIX}dentist`]: {
    alerts: {
      a1: {
        "@type": "Alert",
        action: "display",
        trigger: {
          "@type": "OffsetTrigger",
          offset: "-PT30M",
          relativeTo: "start",
        },
      },
    },
    duration: "PT1H",
    locations: {
      l1: { "@type": "Location", name: "Clínica Dental Sonrisa, C/ Mayor 4" },
    },
    start: at(2, "17:00"),
    timeZone: ZONE,
    title: "Dentist",
  },
  [`${PREFIX}conference`]: {
    duration: "P2D",
    showWithoutTime: true,
    start: at(10, "00:00"),
    title: "Conference trip ✈️",
  },
  [`${PREFIX}lunch`]: {
    description:
      "Table booked for two.\nMenu: https://restaurant.example/menu\nCall if late: +34 600 000 000",
    duration: "PT1H30M",
    locations: { l1: { "@type": "Location", name: "Casa Lola" } },
    start: at(4, "13:30"),
    timeZone: ZONE,
    title: "Lunch with Carla",
  },
  [`${PREFIX}design-review`]: {
    description:
      '<p>Agenda:</p><ol><li>New <b>Helix</b> icons</li><li>Calendar views</li></ol><p><img src="https://tracker.example/pixel.gif" width="1" height="1"><script>alert(1)</script><a href="https://docs.example/review">Review notes</a></p>',
    descriptionContentType: "text/html",
    duration: "PT1H",
    start: at(1, "09:30"),
    timeZone: "America/New_York",
    title: "Design review (New York)",
    virtualLocations: {
      v1: {
        "@type": "VirtualLocation",
        name: "Video call",
        uri: "https://meet.example/helix-review",
      },
    },
  },
});

const findUid = async (
  user: User,
  accountId: string,
  uid: string
): Promise<{ id: string; status: string | null }[]> => {
  const [, got] = await jmap(user, [
    ["CalendarEvent/query", { accountId, filter: { uid } }],
    [
      "CalendarEvent/get",
      {
        "#ids": { name: "CalendarEvent/query", path: "/ids", resultOf: "0" },
        accountId,
        properties: ["uid", "status"],
      },
    ],
  ]);
  return z
    .object({
      list: z.array(
        z.object({
          id: z.string(),
          status: z.string().nullish(),
          uid: z.string(),
        })
      ),
    })
    .parse(got)
    .list.filter((event) => event.uid === uid)
    .map((event) => ({ id: event.id, status: event.status ?? null }));
};

/** Creates the event unless one with its UID exists. Returns whether it did. */
const ensureEvent = async (
  user: User,
  owner: Account,
  uid: string,
  event: Record<string, unknown>,
  schedule = false
): Promise<boolean> => {
  const existing = await findUid(user, owner.calendars, uid);
  if (existing.length > 0) {
    return false;
  }
  await jmap(user, [
    [
      "CalendarEvent/set",
      {
        accountId: owner.calendars,
        create: {
          e: { ...event, calendarIds: { [owner.calendar]: true }, uid },
        },
        sendSchedulingMessages: schedule,
      },
    ],
  ]);
  return true;
};

// --- The outside invitation (.ics by email) --------------------------------

const outsideIcs = (): string =>
  [
    "BEGIN:VCALENDAR",
    "VERSION:2.0",
    "PRODID:-//Partner Example//Calendar//EN",
    "METHOD:REQUEST",
    "BEGIN:VEVENT",
    `UID:${OUTSIDE_UID}`,
    "DTSTAMP:20261024T080000Z",
    "SEQUENCE:0",
    `DTSTART;TZID=America/New_York:${dayOf(ahead(3)).replaceAll("-", "")}T090000`,
    "DURATION:PT1H",
    "SUMMARY:Partner kickoff",
    "LOCATION:Zoom",
    "DESCRIPTION:Kickoff with the partner team. Agenda: https://partner.example/kickoff",
    "ORGANIZER;CN=Carol Vidal:mailto:carol@partner.example",
    "ATTENDEE;ROLE=CHAIR;PARTSTAT=ACCEPTED;CN=Carol Vidal:mailto:carol@partner.example",
    `ATTENDEE;PARTSTAT=NEEDS-ACTION;RSVP=TRUE;CN=Alice:mailto:alice@${DOMAIN}`,
    "ATTENDEE;PARTSTAT=NEEDS-ACTION;RSVP=TRUE;CN=Dan:mailto:dan@partner.example",
    "END:VEVENT",
    "END:VCALENDAR",
    "",
  ].join("\r\n");

const outsideMessage = (): string => {
  const ics = outsideIcs();
  const base64 = Buffer.from(ics, "utf-8")
    .toString("base64")
    .replaceAll(/.{76}/gu, "$&\r\n");
  return [
    "From: Carol Vidal <carol@partner.example>",
    `To: Alice <alice@${DOMAIN}>`,
    "Subject: Invitation: Partner kickoff",
    `Date: ${new Date().toUTCString().replace("GMT", "+0000")}`,
    `Message-ID: <${OUTSIDE_MESSAGE_ID}>`,
    "MIME-Version: 1.0",
    'Content-Type: multipart/mixed; boundary="mix"',
    "",
    "--mix",
    'Content-Type: multipart/alternative; boundary="alt"',
    "",
    "--alt",
    "Content-Type: text/plain; charset=utf-8",
    "",
    "Carol Vidal invited you to Partner kickoff.",
    "--alt",
    "Content-Type: text/calendar; charset=utf-8; method=REQUEST",
    "Content-Transfer-Encoding: base64",
    "",
    base64,
    "--alt--",
    "--mix",
    'Content-Type: application/ics; name="invite.ics"',
    'Content-Disposition: attachment; filename="invite.ics"',
    "Content-Transfer-Encoding: base64",
    "",
    base64,
    "--mix--",
    "",
  ].join("\r\n");
};

const hasOutsideInvitation = async (alice: Account): Promise<boolean> => {
  const [found] = await jmap("alice", [
    [
      "Email/query",
      {
        accountId: alice.mail,
        // Stalwart finds nothing with a Message-ID header filter.
        filter: { from: "carol@partner.example", subject: "Partner kickoff" },
      },
    ],
  ]);
  return Ids.parse(found).ids.length > 0;
};

const importOutside = async (alice: Account): Promise<void> => {
  const upload = await fetch(`${STALWART_URL}/jmap/upload/${alice.mail}/`, {
    body: outsideMessage(),
    headers: {
      authorization: authorization("alice"),
      "content-type": "message/rfc822",
    },
    method: "POST",
  });
  const { blobId } = z
    .object({ blobId: z.string() })
    .parse(await upload.json());
  const [boxes] = await jmap("alice", [
    ["Mailbox/query", { accountId: alice.mail, filter: { role: "inbox" } }],
  ]);
  await jmap("alice", [
    [
      "Email/import",
      {
        accountId: alice.mail,
        emails: {
          m: { blobId, mailboxIds: { [one(Ids.parse(boxes).ids)]: true } },
        },
      },
    ],
  ]);
};

/** A minimal SMTP client for the local test port (no TLS, no auth). */
const sendSmtp = async (
  from: string,
  to: string,
  data: string
): Promise<void> => {
  const socket = connect({ host: "127.0.0.1", port: 2525 });
  socket.setEncoding("utf-8");
  let buffer = "";
  const waitFor = async (code: string): Promise<void> => {
    for (let tries = 0; tries < 600; tries += 1) {
      const lines = buffer.split("\r\n");
      const done = lines.find((line) => /^\d{3} /u.test(line));
      if (done !== undefined) {
        buffer = "";
        if (!done.startsWith(code)) {
          throw new Error(`SMTP: ${done}`);
        }
        return;
      }
      // oxlint-disable-next-line no-await-in-loop -- polling the socket buffer.
      await sleep(100);
    }
    throw new Error("SMTP: no answer");
  };
  socket.on("data", (chunk: string) => {
    buffer += chunk;
  });
  const send = async (line: string, code: string): Promise<void> => {
    socket.write(`${line}\r\n`);
    await waitFor(code);
  };
  await waitFor("220");
  await send("EHLO partner.example", "250");
  await send(`MAIL FROM:<${from}>`, "250");
  await send(`RCPT TO:<${to}>`, "250");
  await send("DATA", "354");
  const body = data.replaceAll(/^\./gmu, "..");
  await send(`${body}\r\n.`, "250");
  await send("QUIT", "221");
  socket.end();
};

// --- Main -------------------------------------------------------------------

const SCHEDULED = {
  budget: `${PREFIX}budget-review`,
  offsite: `${PREFIX}offsite`,
  planning: `${PREFIX}planning`,
} as const;

const removeAll = async (alice: Account, bob: Account): Promise<void> => {
  for (const [user, owner] of [
    ["alice", alice],
    ["bob", bob],
  ] as const) {
    const uids = [
      ...Object.keys(ownEvents()),
      ...Object.values(SCHEDULED),
      OUTSIDE_UID,
    ];
    // oxlint-disable-next-line no-await-in-loop -- one user at a time.
    const found = await Promise.all(
      uids.map(async (uid) => await findUid(user, owner.calendars, uid))
    );
    const ids = found.flat().map((event) => event.id);
    if (ids.length > 0) {
      // oxlint-disable-next-line no-await-in-loop -- one per user.
      await jmap(user, [
        ["CalendarEvent/set", { accountId: owner.calendars, destroy: ids }],
      ]);
    }
    const subjects = [
      "Budget review",
      "Quarterly offsite",
      "Planning with bob",
      "Partner kickoff",
    ];
    // oxlint-disable-next-line no-await-in-loop -- one user at a time.
    const [query, got] = await jmap(user, [
      [
        "Email/query",
        {
          accountId: owner.mail,
          filter: {
            operator: "OR",
            conditions: subjects.map((subject) => ({ subject })),
          },
          limit: 200,
        },
      ],
      [
        "Email/get",
        {
          "#ids": { name: "Email/query", path: "/ids", resultOf: "0" },
          accountId: owner.mail,
          properties: ["subject"],
        },
      ],
    ]);
    const emails = z
      .object({
        list: z.array(
          z.object({ id: z.string(), subject: z.string().nullish() })
        ),
      })
      .parse(got)
      .list.filter((email) =>
        subjects.some((subject) => email.subject?.includes(subject) === true)
      )
      .map((email) => email.id);
    if (Ids.parse(query).ids.length > 0 && emails.length > 0) {
      // oxlint-disable-next-line no-await-in-loop -- one per user.
      await jmap(user, [
        ["Email/set", { accountId: owner.mail, destroy: emails }],
      ]);
    }
    console.log(
      `${user}: removed ${ids.length} events and ${emails.length} emails`
    );
  }
};

const main = async (): Promise<void> => {
  const alice = await account("alice");
  const bob = await account("bob");
  if (args.has("--remove")) {
    await removeAll(alice, bob);
    return;
  }
  let created = 0;
  for (const [uid, event] of Object.entries(ownEvents())) {
    // oxlint-disable-next-line no-await-in-loop -- a few events, in order.
    created += Number(await ensureEvent("alice", alice, uid, event));
  }
  console.log(`alice: ${created} new events of her own`);

  // bob → alice: Stalwart emails the invitation and, as bob is a user of
  // this server, also puts it in alice's calendar (unanswered).
  if (
    await ensureEvent(
      "bob",
      bob,
      SCHEDULED.budget,
      {
        duration: "PT1H",
        locations: { l1: { "@type": "Location", name: "Room 2" } },
        participants: {
          a: participant("alice", "Alice", false),
          o: participant("bob", "Bob", true),
        },
        start: at(ahead(1), "11:00"),
        timeZone: ZONE,
        title: "Budget review",
      },
      true
    )
  ) {
    console.log("bob: invited alice to “Budget review”");
  }

  // bob invites, then cancels: alice gets a CANCEL.
  const offsiteCopies = await findUid(
    "alice",
    alice.calendars,
    SCHEDULED.offsite
  );
  if (offsiteCopies.length === 0) {
    await ensureEvent(
      "bob",
      bob,
      SCHEDULED.offsite,
      {
        duration: "PT2H",
        participants: {
          a: participant("alice", "Alice", false),
          o: participant("bob", "Bob", true),
        },
        start: at(ahead(4), "16:00"),
        timeZone: ZONE,
        title: "Quarterly offsite",
      },
      true
    );
    await sleep(2000);
    const [offsite] = await findUid("bob", bob.calendars, SCHEDULED.offsite);
    if (offsite !== undefined) {
      await jmap("bob", [
        [
          "CalendarEvent/set",
          {
            accountId: bob.calendars,
            destroy: [offsite.id],
            sendSchedulingMessages: true,
          },
        ],
      ]);
    }
    console.log("bob: invited alice to “Quarterly offsite” and cancelled it");
  }

  // alice invites bob, bob accepts: a REPLY for alice.
  if (
    await ensureEvent(
      "alice",
      alice,
      SCHEDULED.planning,
      {
        duration: "PT30M",
        participants: {
          b: participant("bob", "Bob", false),
          o: participant("alice", "Alice", true),
        },
        start: at(ahead(2), "15:00"),
        timeZone: ZONE,
        title: "Planning with bob",
      },
      true
    )
  ) {
    await sleep(2000);
    const [planning] = await findUid("bob", bob.calendars, SCHEDULED.planning);
    if (planning !== undefined) {
      await jmap("bob", [
        [
          "CalendarEvent/set",
          {
            accountId: bob.calendars,
            sendSchedulingMessages: true,
            update: {
              [planning.id]: {
                "participants/b/participationStatus": "accepted",
              },
            },
          },
        ],
      ]);
    }
    console.log("alice: invited bob to “Planning with bob”; bob accepted");
  }

  if (await hasOutsideInvitation(alice)) {
    console.log("alice: the outside invitation is already in her inbox");
  } else if (args.has("--smtp")) {
    await sendSmtp(
      "carol@partner.example",
      `alice@${DOMAIN}`,
      outsideMessage()
    );
    console.log(
      "SMTP: sent “Partner kickoff”; Stalwart delivers it in a minute or two"
    );
  } else {
    await importOutside(alice);
    console.log("alice: imported the outside invitation “Partner kickoff”");
  }
};

await main();
