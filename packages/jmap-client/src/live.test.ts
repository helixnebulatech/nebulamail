// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Integration test against a real Stalwart. Skipped unless these are set
// (the local values come from `bun run dev:setup`):
//
//   JMAP_LIVE_URL=http://localhost:8080 JMAP_LIVE_USER=alice@nebulamail.test \
//   JMAP_LIVE_PASSWORD=… JMAP_LIVE_CLIENT_ID=nebula-dev \
//   JMAP_LIVE_CLIENT_SECRET=… JMAP_LIVE_REDIRECT=http://localhost:3001/auth/callback \
//   bun run test
//
// It logs in the way Stalwart 1.0's login page does (POST /api/auth, then
// the token endpoint with PKCE), so it only works with Stalwart.

import { createHash, randomBytes } from "node:crypto";

import { describe, expect, it } from "vitest";
import { z } from "zod";

import { CAPABILITIES } from "./capabilities";
import { JmapClient } from "./client";

const env = {
  clientId: process.env.JMAP_LIVE_CLIENT_ID,
  clientSecret: process.env.JMAP_LIVE_CLIENT_SECRET,
  password: process.env.JMAP_LIVE_PASSWORD,
  redirect: process.env.JMAP_LIVE_REDIRECT,
  url: process.env.JMAP_LIVE_URL,
  user: process.env.JMAP_LIVE_USER,
};

const configured = Object.values(env).every(
  (value) => value !== undefined && value !== ""
);

const base64Url = (bytes: Buffer): string => bytes.toString("base64url");

/** What the web app asks for (apps/web/src/lib/oauth-scope.ts). */
const SCOPE = [
  "urn:ietf:params:oauth:scope:mail",
  "urn:ietf:params:oauth:scope:contacts",
  "urn:ietf:params:oauth:scope:calendars",
  "urn:ietf:params:oauth:scope:files",
].join(" ");

const LoginResponse = z.object({
  location: z.string().optional(),
  type: z.string(),
});

/**
 * OAuth code + PKCE against Stalwart 1.0, without a browser: what its login
 * page sends (`POST /api/auth` with the authorization request's query),
 * with `consent: true` for its consent screen.
 */
const stalwartToken = async (): Promise<string> => {
  const verifier = base64Url(randomBytes(32));
  const challenge = base64Url(createHash("sha256").update(verifier).digest());
  const state = base64Url(randomBytes(16));
  const query = new URLSearchParams({
    client_id: env.clientId ?? "",
    code_challenge: challenge,
    code_challenge_method: "S256",
    redirect_uri: env.redirect ?? "",
    response_type: "code",
    scope: SCOPE,
    state,
  }).toString();
  const login = await fetch(`${env.url}/api/auth`, {
    body: JSON.stringify({
      accountName: env.user,
      accountSecret: env.password,
      consent: true,
      query,
      type: "authCode",
    }),
    headers: { "content-type": "application/json" },
    method: "POST",
  });
  const answer = LoginResponse.parse(await login.json());
  const location = new URL(answer.location ?? "about:blank");
  const code = location.searchParams.get("code");
  if (
    answer.type !== "authenticated" ||
    code === null ||
    location.searchParams.get("state") !== state
  ) {
    throw new Error(`Stalwart refused the login: ${JSON.stringify(answer)}`);
  }
  const token = await fetch(`${env.url}/auth/token`, {
    body: new URLSearchParams({
      code,
      code_verifier: verifier,
      grant_type: "authorization_code",
      redirect_uri: env.redirect ?? "",
    }),
    headers: {
      authorization: `Basic ${btoa(`${env.clientId}:${env.clientSecret}`)}`,
    },
    method: "POST",
  });
  return z.object({ access_token: z.string() }).parse(await token.json())
    .access_token;
};

describe.skipIf(!configured)("live Stalwart", () => {
  it("reads the inbox, its emails and the full body of one", async () => {
    const token = await stalwartToken();
    const client = await JmapClient.connect({
      getAccessToken: async () => await Promise.resolve(token),
      sessionUrl: `${env.url}/.well-known/jmap`,
    });
    const accountId = client.primaryAccountId();

    const [mailboxes, query, emails] = await client.request((b) => {
      const boxes = b.call("Mailbox/get", {
        accountId,
        properties: ["name", "role", "totalEmails", "unreadEmails"],
      });
      const q = b.call("Email/query", {
        accountId,
        collapseThreads: true,
        limit: 10,
        sort: [{ isAscending: false, property: "receivedAt" }],
      });
      const g = b.call("Email/get", {
        "#ids": q.ref("/ids"),
        accountId,
        bodyProperties: [
          "partId",
          "blobId",
          "type",
          "size",
          "name",
          "disposition",
          "cid",
        ],
        fetchHTMLBodyValues: true,
        fetchTextBodyValues: true,
        properties: [
          "threadId",
          "mailboxIds",
          "keywords",
          "from",
          "to",
          "subject",
          "receivedAt",
          "preview",
          "hasAttachment",
          "textBody",
          "htmlBody",
          "attachments",
          "bodyValues",
        ],
      });
      return [boxes, q, g] as const;
    });

    expect(mailboxes.list.map((box) => box.role)).toContain("inbox");
    expect(query.ids.length).toBeGreaterThan(0);
    expect(emails.list).toHaveLength(query.ids.length);
    expect(emails.list.every((email) => email.receivedAt !== "")).toBeTruthy();
  });

  it("downloads an attachment and marks an email as read", async () => {
    const token = await stalwartToken();
    const client = await JmapClient.connect({
      getAccessToken: async () => await Promise.resolve(token),
      sessionUrl: `${env.url}/.well-known/jmap`,
    });
    const accountId = client.primaryAccountId();
    const [query, emails] = await client.request((b) => {
      const q = b.call("Email/query", {
        accountId,
        filter: { hasAttachment: true },
        limit: 1,
      });
      const g = b.call("Email/get", {
        "#ids": q.ref("/ids"),
        accountId,
        properties: ["attachments", "keywords"],
      });
      return [q, g] as const;
    });
    const [email] = emails.list;
    const [attachment] = email?.attachments ?? [];
    const [id] = query.ids;
    if (
      attachment?.blobId === undefined ||
      attachment.blobId === null ||
      id === undefined
    ) {
      throw new Error(
        "Expected a sample email with an attachment (bun run dev:setup)"
      );
    }
    const blob = await client.downloadBlob({
      accountId,
      blobId: attachment.blobId,
      maxBytes: 1_000_000,
      name: attachment.name ?? "file",
      ...(attachment.type === undefined ? {} : { type: attachment.type }),
    });
    const [updated] = await client.request((b) => [
      b.call("Email/set", {
        accountId,
        update: { [id]: { "keywords/$seen": true } },
      }),
    ]);

    await expect(blob.text()).resolves.toContain("42 EUR");
    expect(updated.updated).toHaveProperty(id);
  });

  it("creates, reads, parses and deletes a contact card", async () => {
    const token = await stalwartToken();
    const client = await JmapClient.connect({
      getAccessToken: async () => await Promise.resolve(token),
      sessionUrl: `${env.url}/.well-known/jmap`,
    });
    const accountId = client.primaryAccountId(CAPABILITIES.contacts);
    const [books] = await client.request((b) => [
      b.call("AddressBook/get", { accountId }),
    ]);
    const book = books.list.find((entry) => entry.isDefault) ?? books.list[0];
    if (book === undefined) {
      throw new Error("Expected an address book");
    }
    const uid = `urn:uuid:${crypto.randomUUID()}`;
    const [created] = await client.request((b) => [
      b.call("ContactCard/set", {
        accountId,
        create: {
          card: {
            "@type": "Card",
            addressBookIds: { [book.id]: true },
            emails: { e1: { address: "live@example.com" } },
            name: { full: "Live Test" },
            phones: { p1: { number: "+34 600" } },
            uid,
            version: "1.0",
          },
        },
      }),
    ]);
    const id = created.created.card?.id;
    if (id === undefined) {
      throw new Error(JSON.stringify(created.notCreated));
    }
    const upload = await client.upload(
      accountId,
      new Blob(["BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Parsed\r\nEND:VCARD\r\n"], {
        type: "text/vcard",
      })
    );
    const [cards, parsed] = await client.request((b) => [
      // `organizations` is left out by Stalwart; the client fills in null.
      b.call("ContactCard/get", {
        accountId,
        ids: [id],
        properties: ["uid", "name", "phones", "organizations"],
      }),
      b.call("ContactCard/parse", { accountId, blobIds: [upload.blobId] }),
    ]);
    const [destroyed] = await client.request((b) => [
      b.call("ContactCard/set", { accountId, destroy: [id] }),
    ]);

    expect(cards.list[0]).toMatchObject({ organizations: null, uid });
    expect(parsed.parsed[upload.blobId]?.name?.full).toBe("Parsed");
    expect(destroyed.destroyed).toStrictEqual([id]);
  });

  it("reads the quota and the directory, and shares a folder", async () => {
    const token = await stalwartToken();
    const client = await JmapClient.connect({
      getAccessToken: async () => await Promise.resolve(token),
      sessionUrl: `${env.url}/.well-known/jmap`,
    });
    const accountId = client.primaryAccountId();
    const [quotas, , principals, folder] = await client.request((b) => {
      b.using(CAPABILITIES.mailShare);
      const quota = b.call("Quota/get", { accountId, ids: null });
      const query = b.call("Principal/query", { accountId, limit: 50 });
      const people = b.call("Principal/get", {
        "#ids": query.ref("/ids"),
        accountId,
        properties: ["name", "email", "type"],
      });
      const box = b.call("Mailbox/set", {
        accountId,
        create: { box: { name: `Live share ${Date.now()}` } },
      });
      return [quota, query, people, box] as const;
    });
    const boxId = folder.created.box?.id;
    const other = principals.list.find(
      (principal) => principal.email !== client.session.username
    );
    if (boxId === undefined || other === undefined) {
      throw new Error("Expected a new folder and another principal");
    }
    const rights = {
      mayAddItems: false,
      mayCreateChild: false,
      mayDelete: false,
      mayReadItems: true,
      mayRemoveItems: false,
      mayRename: false,
      maySetKeywords: false,
      maySetSeen: false,
      mayShare: false,
      maySubmit: false,
    };
    const [, shared] = await client.request((b) => {
      b.using(CAPABILITIES.mailShare);
      return [
        b.call("Mailbox/set", {
          accountId,
          update: { [boxId]: { [`shareWith/${other.id}`]: rights } },
        }),
        b.call("Mailbox/get", {
          accountId,
          ids: [boxId],
          properties: ["shareWith"],
        }),
      ];
    });
    await client.request((b) => [
      b.call("Mailbox/set", { accountId, destroy: [boxId] }),
    ]);

    expect(Array.isArray(quotas.list)).toBeTruthy();
    expect(shared.list[0]?.shareWith?.[other.id]?.mayReadItems).toBeTruthy();
  });
});
