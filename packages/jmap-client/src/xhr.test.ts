// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { describe, expect, it } from "vitest";

import { JmapClient } from "./client";
import { AccountId } from "./ids";
import {
  SESSION_URL,
  fakeFetch,
  rejection,
  sessionThen,
  stalwartSession,
  tokens,
} from "./test-support";
import type { XhrLike, XhrProgress } from "./xhr";

const accountId = AccountId.parse("d");
// Asking for progress is what selects XHR; the values aren't checked here.
const ignoreProgress = (): void => {
  // Nothing to do.
};
const UPLOAD_URL = "https://mail.example.com/jmap/upload/d/";
// The app on its own origin, JMAP on another one (the server version).
const APP_PAGE = "https://app.example.com";

type Reply =
  | {
      readonly status: number;
      readonly body: string;
      readonly url?: string;
      readonly progress?: readonly [number, number][];
    }
  | "error"
  | "hang";

/** A scripted XMLHttpRequest: each `send` takes the next reply. */
const fakeXhr = (replies: Reply[]) => {
  const sent: {
    url: string;
    headers: Record<string, string>;
    withCredentials: boolean;
    body: unknown;
  }[] = [];
  const factory = (): XhrLike => {
    const listeners = new Map<string, (() => void)[]>();
    const progress: ((event: XhrProgress) => void)[] = [];
    const fire = (type: string): void => {
      for (const listener of listeners.get(type) ?? []) {
        listener();
      }
    };
    let url = "";
    const headers: Record<string, string> = {};
    const xhr = {
      abort: () => {
        fire("abort");
      },
      addEventListener: (type: string, listener: () => void) => {
        listeners.set(type, [...(listeners.get(type) ?? []), listener]);
      },
      open: (_method: "POST", target: string) => {
        url = target;
      },
      responseText: "",
      responseURL: "",
      send: (body: unknown) => {
        sent.push({ body, headers, url, withCredentials: xhr.withCredentials });
        const reply = replies.shift() ?? "hang";
        queueMicrotask(() => {
          if (reply === "hang") {
            return;
          }
          if (reply === "error") {
            fire("error");
            return;
          }
          for (const [loaded, total] of reply.progress ?? []) {
            for (const listener of progress) {
              listener({ lengthComputable: true, loaded, total });
            }
          }
          xhr.status = reply.status;
          xhr.responseText = reply.body;
          xhr.responseURL = reply.url ?? url;
          fire("load");
        });
      },
      setRequestHeader: (name: string, value: string) => {
        headers[name] = value;
      },
      status: 0,
      upload: {
        addEventListener: (
          _type: "progress",
          listener: (event: XhrProgress) => void
        ) => {
          progress.push(listener);
        },
      },
      withCredentials: true,
    };
    return xhr;
  };
  return { factory, sent };
};

const ok = (size: number): Exclude<Reply, string> => ({
  body: JSON.stringify({
    accountId: "d",
    blobId: "G9",
    size,
    type: "text/plain",
  }),
  progress: [
    [2, size],
    [size, size],
  ],
  status: 201,
});

const setup = async (replies: Reply[]) => {
  const xhr = fakeXhr(replies);
  const fake = fakeFetch(
    sessionThen(() => new Response("fetch used", { status: 500 }))
  );
  const auth = tokens();
  const client = await JmapClient.connect({
    fetch: fake.fetch,
    getAccessToken: auth.getAccessToken,
    sessionUrl: SESSION_URL,
    xhr: { create: xhr.factory, pageOrigin: APP_PAGE },
  });
  return { client, fetchCalls: fake.calls, sent: xhr.sent, tokens: auth };
};

describe("upload with progress (XHR)", () => {
  it("reports progress and sends the token, never cookies", async () => {
    const { client, fetchCalls, sent } = await setup([ok(5)]);
    const progress: [number, number | undefined][] = [];
    const result = await client.upload(accountId, new Blob(["hello"]), {
      onProgress: (loaded, total) => {
        progress.push([loaded, total]);
      },
      type: "text/plain",
    });
    expect(result.blobId).toBe("G9");
    expect(progress).toStrictEqual([
      [2, 5],
      [5, 5],
      [5, 5],
    ]);
    expect(sent).toMatchObject([
      {
        headers: {
          Authorization: "Bearer old-token",
          "Content-Type": "text/plain",
        },
        url: UPLOAD_URL,
        withCredentials: false,
      },
    ]);
    // Only the session went through fetch.
    expect(fetchCalls).toHaveLength(1);
  });

  it("retries once with a fresh token after a 401", async () => {
    const {
      client,
      sent,
      tokens: auth,
    } = await setup([{ body: "", status: 401 }, ok(1)]);
    await client.upload(accountId, new Blob(["x"]), {
      onProgress: ignoreProgress,
    });
    expect(sent.map((call) => call.headers.Authorization)).toStrictEqual([
      "Bearer old-token",
      "Bearer fresh-token",
    ]);
    expect(auth.requests).toStrictEqual([false, false, true]);
  });

  it("refuses an answer that came through a redirect", async () => {
    const { client } = await setup([
      { ...ok(1), url: "https://elsewhere.example/upload" },
    ]);
    const error = await rejection(
      client.upload(accountId, new Blob(["x"]), { onProgress: ignoreProgress })
    );
    expect(error.detail).toMatchObject({
      kind: "limit",
      limit: "maxRedirects",
    });
  });

  it("maps network errors and aborts", async () => {
    const { client } = await setup(["error", "hang"]);
    const network = await rejection(
      client.upload(accountId, new Blob(["x"]), { onProgress: ignoreProgress })
    );
    expect(network.detail.kind).toBe("network");
    const controller = new AbortController();
    const pending = client.upload(accountId, new Blob(["x"]), {
      onProgress: ignoreProgress,
      signal: controller.signal,
    });
    await Promise.resolve();
    await Promise.resolve();
    controller.abort();
    const aborted = await rejection(pending);
    expect(aborted.detail.kind).toBe("aborted");
  });

  it("checks maxSizeUpload before sending anything", async () => {
    const xhr = fakeXhr([ok(1)]);
    const fake = fakeFetch(
      sessionThen(
        () => new Response(null, { status: 500 }),
        stalwartSession({}, { maxSizeUpload: 3 })
      )
    );
    const client = await JmapClient.connect({
      fetch: fake.fetch,
      sessionUrl: SESSION_URL,
      xhr: { create: xhr.factory, pageOrigin: APP_PAGE },
      ...tokens(),
    });
    const error = await rejection(
      client.upload(accountId, new Blob(["toolong"]), {
        onProgress: ignoreProgress,
      })
    );
    expect(error.detail).toMatchObject({
      kind: "limit",
      limit: "maxSizeUpload",
    });
    expect(xhr.sent).toHaveLength(0);
  });

  it("uses fetch when nobody asked for progress", async () => {
    const { client, sent } = await setup([]);
    const error = await rejection(client.upload(accountId, new Blob(["x"])));
    expect(error.detail.kind).toBe("http");
    expect(sent).toHaveLength(0);
  });
});

const blobCreated = (): Response =>
  Response.json(
    { accountId: "d", blobId: "G9", size: 5, type: "text/plain" },
    { status: 201 }
  );

const connectWithPage = async (
  pageOrigin: string,
  session: Record<string, unknown> = stalwartSession(),
  serverAliases?: readonly string[]
) => {
  const xhr = fakeXhr([ok(5)]);
  const fake = fakeFetch(sessionThen(blobCreated, session));
  const client = await JmapClient.connect({
    fetch: fake.fetch,
    sessionUrl: SESSION_URL,
    xhr: { create: xhr.factory, pageOrigin },
    ...(serverAliases === undefined ? {} : { serverAliases }),
    ...tokens(),
  });
  const progress: [number, number | undefined][] = [];
  await client.upload(accountId, new Blob(["hello"]), {
    onProgress: (loaded, total) => {
      progress.push([loaded, total]);
    },
    type: "text/plain",
  });
  return { fetchCalls: fake.calls, progress, sent: xhr.sent };
};

describe("uploads on the page's own origin never use XHR (cookies)", () => {
  it("same origin: fetch with credentials omit, progress only at the end", async () => {
    const { fetchCalls, progress, sent } = await connectWithPage(
      "https://mail.example.com"
    );
    expect(sent).toHaveLength(0);
    const uploadCall = fetchCalls.find((call) => call.url === UPLOAD_URL);
    expect(uploadCall?.init.credentials).toBe("omit");
    expect(uploadCall?.authorization).toBe("Bearer old-token");
    expect(progress).toStrictEqual([[5, 5]]);
  });

  it("another origin: XHR with withCredentials false", async () => {
    const { sent } = await connectWithPage(APP_PAGE);
    expect(sent).toMatchObject([{ url: UPLOAD_URL, withCredentials: false }]);
  });

  it("compares with the upload URL after serverAliases", async () => {
    // Stalwart writes its public name in the session; the static build
    // serves the app on another name of the same server.
    const session = stalwartSession({
      uploadUrl: "https://public.example.com/jmap/upload/{accountId}/",
    });
    const { fetchCalls, sent } = await connectWithPage(
      "https://mail.example.com",
      session,
      ["https://public.example.com"]
    );
    expect(sent).toHaveLength(0);
    const uploadCall = fetchCalls.find((call) => call.url === UPLOAD_URL);
    expect(uploadCall?.init.credentials).toBe("omit");
  });

  it("a page origin that is not a URL counts as the same origin", async () => {
    const { fetchCalls, sent } = await connectWithPage("not a url");
    expect(sent).toHaveLength(0);
    expect(fetchCalls.map((call) => call.url)).toContain(UPLOAD_URL);
  });
});

const connectWithXhr = async (xhr?: { readonly pageOrigin: string }) => {
  const fake = fakeFetch(sessionThen(blobCreated));
  return await JmapClient.connect({
    fetch: fake.fetch,
    sessionUrl: SESSION_URL,
    ...(xhr === undefined
      ? {}
      : {
          xhr: { create: fakeXhr([]).factory, pageOrigin: xhr.pageOrigin },
        }),
    ...tokens(),
  });
};

describe("uploadReportsProgress says whether the bar can be real", () => {
  it("another origin with XHR: real progress", async () => {
    const client = await connectWithXhr({ pageOrigin: APP_PAGE });
    expect(
      client.uploadReportsProgress(accountId, new Blob(["x"]))
    ).toBeTruthy();
  });

  it("same origin: no progress until the end (fetch, cookies omitted)", async () => {
    const client = await connectWithXhr({
      pageOrigin: "https://mail.example.com",
    });
    expect(
      client.uploadReportsProgress(accountId, new Blob(["x"]))
    ).toBeFalsy();
  });

  it("no XHR factory: no progress for a Blob", async () => {
    const client = await connectWithXhr();
    expect(
      client.uploadReportsProgress(accountId, new Uint8Array(1))
    ).toBeFalsy();
  });

  it("stream bodies count the bytes handed to fetch", async () => {
    const client = await connectWithXhr({
      pageOrigin: "https://mail.example.com",
    });
    expect(
      client.uploadReportsProgress(accountId, new ReadableStream<Uint8Array>())
    ).toBeTruthy();
  });
});

describe("uploads with same-origin cookies (BFF proxy)", () => {
  const PROXY_SESSION = `${APP_PAGE}/jmap-proxy/session`;
  const connectProxy = async (pageOrigin: string) => {
    const xhr = fakeXhr([ok(5)]);
    const fake = fakeFetch((call) =>
      call.url === PROXY_SESSION
        ? Response.json(
            stalwartSession({
              apiUrl: `${APP_PAGE}/jmap-proxy/api`,
              downloadUrl: `${APP_PAGE}/jmap-proxy/download/{accountId}/{blobId}/{name}?type={type}`,
              eventSourceUrl: `${APP_PAGE}/jmap-proxy/eventsource?types={types}&closeafter={closeafter}&ping={ping}`,
              uploadUrl: `${APP_PAGE}/jmap-proxy/upload/{accountId}`,
            })
          )
        : Response.json(
            { accountId: "d", blobId: "G9", size: 5, type: "text/plain" },
            { status: 201 }
          )
    );
    const client = await JmapClient.connect({
      fetch: fake.fetch,
      headers: { "X-Nebula-Proxy": "1" },
      sameOriginCookies: true,
      sessionUrl: PROXY_SESSION,
      xhr: { create: xhr.factory, pageOrigin },
    });
    return { client, fetchCalls: fake.calls, sent: xhr.sent };
  };

  it("uses XHR on the page's own origin, with the header and no token", async () => {
    const { client, sent } = await connectProxy(APP_PAGE);
    expect(
      client.uploadReportsProgress(accountId, new Blob(["hello"]))
    ).toBeTruthy();
    await client.upload(accountId, new Blob(["hello"]), {
      onProgress: ignoreProgress,
    });
    expect(sent).toHaveLength(1);
    expect(sent[0]?.withCredentials).toBeFalsy();
    expect(sent[0]?.headers["X-Nebula-Proxy"]).toBe("1");
    expect(sent[0]?.headers.Authorization).toBeUndefined();
  });

  it("never uses XHR for another origin", async () => {
    const { client, fetchCalls, sent } = await connectProxy(
      "https://other.example.com"
    );
    expect(
      client.uploadReportsProgress(accountId, new Blob(["hello"]))
    ).toBeFalsy();
    await client.upload(accountId, new Blob(["hello"]), {
      onProgress: ignoreProgress,
    });
    expect(sent).toHaveLength(0);
    expect(fetchCalls.at(-1)?.init.credentials).toBe("same-origin");
  });
});
