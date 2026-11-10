<!--
SPDX-FileCopyrightText: 2026 Nebula Mail contributors
SPDX-License-Identifier: MIT OR Apache-2.0
-->

# @nebulamail/jmap-client

Typed, runtime-agnostic JMAP client for [RFC 8620] (core) and [RFC 8621] (mail). It only uses `fetch`, `URL`, `AbortSignal` and `ReadableStream`, so it runs in browsers, Node, Bun and Workers. Its only dependency is `zod`. License: MIT OR Apache-2.0.

```ts
import { JmapClient, MailboxId, keywordPatch } from "@nebulamail/jmap-client";

const client = await JmapClient.connect({
  sessionUrl: "https://mail.example.com/.well-known/jmap",
  // Short-lived OAuth token, e.g. from your BFF. `forceRefresh` is true
  // after a 401; the client then retries the request once.
  getAccessToken: async ({ forceRefresh }) => await bff.token(forceRefresh),
});
const accountId = client.primaryAccountId(); // mail by default

// One HTTP request, two method calls, the second uses the first's result.
const [query, emails] = await client.request((b) => {
  const q = b.call("Email/query", {
    accountId,
    filter: { inMailbox: MailboxId.parse("a") },
    sort: [{ property: "receivedAt", isAscending: false }],
    collapseThreads: true,
    limit: 50,
  });
  const g = b.call("Email/get", {
    accountId,
    "#ids": q.ref("/ids"),
    properties: ["threadId", "subject", "from", "receivedAt", "preview"],
  });
  return [q, g] as const;
});
emails.list[0]?.subject; // string | null — and `htmlBody` is a type error,
// because it wasn't requested.

const [first] = query.ids;
if (first !== undefined) {
  await client.request((b) =>
    b.call("Email/set", {
      accountId,
      update: { [first]: keywordPatch({ $seen: true }) },
    })
  );
}
```

## API

|  |  |
| --- | --- |
| `JmapClient.connect(options)` | Fetches and validates the session. Options: `sessionUrl`, `getAccessToken`, `fetch?`, `allowCrossOriginUrls?`, `maxResponseBytes?`, `signal?` |
| `client.request(b => …)` | Batched request. Return a handle, a tuple or an object of handles; get the typed responses back. Throws the first error |
| `client.requestSettled(b => …)` | Same, but every result is `{ ok: true, value } \| { ok: false, error }` |
| `b.call(method, args)` | Typed call (see `MethodMap` for the list). `handle.ref("/path")` makes a typed result reference; invalid paths are compile errors |
| `b.extension(name, args, { using, response })` | Untyped methods (Sieve, Quota, vendor), checked with your zod schema |
| `client.download({ accountId, blobId, name?, type?, maxBytes? })` | `Response` with a size-limited body; `downloadBlob` buffers it |
| `client.upload(accountId, body, { type?, size?, onProgress? })` | `Blob`, bytes or `ReadableStream`; checks `maxSizeUpload` first |
| `client.push({ types?, ping?, signal? })` | `for await (const change of client.push())`: typed `StateChange`s over SSE, with reconnects |
| `client.session`, `hasCapability(session, cap, accountId?)`, `mailCapability`, `submissionCapability` | Session and capabilities |
| `keywordPatch`, `mailboxPatch` | `Email/set` patches with correct JSON Pointer escaping |
| `parseSse`, `feedSse` | The pure `text/event-stream` parser |

Typed methods: `Core/echo`, `Mailbox/get|changes|set`, `Email/get|query|queryChanges|changes|set|import`, `Thread/get|changes`, `Identity/get|changes|set`, `EmailSubmission/get|set` (the implicit `Email/set` from `onSuccessUpdateEmail` is returned as `emailSet`), `SearchSnippet/get`, `VacationResponse/get|set`. Requests above `maxCallsInRequest` are split into several HTTP requests (in order) unless a result reference would cross them.

Ids are branded: `EmailId.parse("…")` checks the RFC 8620 Id syntax once, and then a `MailboxId` can't be passed where an `EmailId` is expected.

## Errors

Everything throws `JmapError`; switch on `error.detail.kind`: `network`, `aborted`, `auth` (getAccessToken threw), `unauthorized` (401 after a refresh), `http`, `request` (RFC 7807 problem: `type`, `status`, `limit`…), `method` (`["error", …]`: `type`, `callId`, `method`), `invalidResponse` (failed schema; only paths, never values), `invalidUrl`, `crossOrigin`, `limit` (`maxCallsInRequest`, `maxSizeRequest`, `maxSizeUpload`, `maxBytes`, `maxRedirects`), `usage`. Messages never contain tokens.

## Security choices

- **Origin guard.** `apiUrl`, `downloadUrl`, `uploadUrl` and `eventSourceUrl` must be on the session URL's origin (scheme included, so no HTTPS → HTTP downgrade), unless `allowCrossOriginUrls` lists other origins (or is `true`). A malicious or compromised session document therefore can't make the client send the Bearer token somewhere else. URLs must be http(s) and without `user:pass@`. Templates are checked when the session loads and again after every expansion.
- **No redirects with the token.** API, upload, download and push use `redirect: "error"`. For the session (`/.well-known/jmap` often redirects) the client uses `redirect: "manual"`, checks the target and only then sends the token there. Browsers hide `Location` (`opaqueredirect`), so there the target is found with an _unauthenticated_ request.
- `credentials: "omit"` (no cookies or ambient HTTP auth) and `referrerPolicy: "no-referrer"` on every request.
- Size limits: session (1 MiB), API responses (`maxResponseBytes`, 32 MiB by default), error bodies (64 KiB), downloads (`maxBytes`), uploads (`maxSizeUpload`), requests (`maxSizeRequest`).
- Server HTML (`SearchSnippet`, signatures, bodies) is returned as-is: sanitize it before rendering.
- On a server (BFF), pass a `fetch` that is protected against SSRF.

## Notes and limits

- Upload progress: browsers only stream request bodies in Chromium over HTTP/2+, so `onProgress` is fine-grained only for stream bodies. For a progress bar elsewhere, use `client.uploadUrl(accountId)` with XHR.
- Bodies that are streams can't be resent, so a 401 during a stream upload fails with `unauthorized` instead of retrying.
- Push sends `types`/`closeafter`/`ping` encoded as RFC 6570 level 1 requires (`*` → `%2A`, `,` → `%2C`).
- Not yet: WebSocket push (RFC 8887), `PushSubscription` (Web Push), `Mailbox/query`, `Email/copy`, `Blob/*` (RFC 9404), Sieve/Quota types (use `b.extension`).

[RFC 8620]: https://www.rfc-editor.org/rfc/rfc8620
[RFC 8621]: https://www.rfc-editor.org/rfc/rfc8621
