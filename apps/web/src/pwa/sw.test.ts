// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// sw.ts is a classic script with build-time constants, not a module: it is
// loaded once here with fake globals, and its listeners are called with
// fake events and clients.

import {
  afterEach,
  beforeAll,
  beforeEach,
  describe,
  expect,
  it,
  vi,
} from "vitest";

type Listener = (event: unknown) => void;

interface FakeTab {
  readonly url: string;
  focus?: () => Promise<FakeTab>;
  navigate?: (url: string) => Promise<FakeTab | null>;
  postMessage: (message: unknown) => void;
}

const listeners = new Map<string, Listener>();
let controlled: FakeTab[] = [];
let uncontrolled: FakeTab[] = [];
const openWindow = vi.fn<(url: string) => Promise<null>>(
  async () => await Promise.resolve(null)
);
const added: Request[] = [];

const tab = (url: string, overrides: Partial<FakeTab> = {}): FakeTab => {
  const self: FakeTab = {
    focus: vi.fn<() => Promise<FakeTab>>(
      async () => await Promise.resolve(self)
    ),
    navigate: vi.fn<(url: string) => Promise<FakeTab | null>>(
      async () => await Promise.resolve(self)
    ),
    postMessage: vi.fn<(message: unknown) => void>(),
    url,
    ...overrides,
  };
  return self;
};

/** Runs a listener and waits for what it passed to waitUntil. */
const fire = async (type: string, event: object): Promise<void> => {
  const pending: Promise<unknown>[] = [];
  listeners.get(type)?.({
    ...event,
    waitUntil: (promise: Promise<unknown>) => {
      pending.push(promise);
    },
  });
  await Promise.all(pending);
};

const clickMail = async (): Promise<void> => {
  await fire("notificationclick", {
    action: "",
    notification: {
      close: vi.fn<() => void>(),
      data: { "@type": "MailNotice", emailId: "e1" },
    },
  });
};

/** A BFF and JMAP server for the worker; records each API request. */
const fakeJmap = (): unknown[] => {
  const posted: unknown[] = [];
  const fetch = vi.fn<typeof globalThis.fetch>(async (input, init) => {
    const url = input instanceof Request ? input.url : input.toString();
    if (url.endsWith("/auth/token")) {
      return await Promise.resolve(
        Response.json({
          accessToken: "t",
          sessionUrl: "https://mail.example.com/jmap/session",
        })
      );
    }
    if (url.endsWith("/jmap/session")) {
      return await Promise.resolve(
        Response.json({ apiUrl: "https://mail.example.com/jmap/api" })
      );
    }
    const body: unknown = JSON.parse(
      typeof init?.body === "string" ? init.body : "null"
    );
    posted.push(body);
    return await Promise.resolve(
      Response.json({
        methodResponses: [
          ["PushSubscription/set", { updated: { P1: null } }, "0"],
        ],
      })
    );
  });
  vi.stubGlobal("fetch", fetch);
  vi.stubGlobal("SW_TOKEN_ENDPOINT", "/nebulamail/auth/token");
  return posted;
};

describe("service worker", () => {
  beforeAll(async () => {
    vi.stubGlobal(
      "location",
      new URL("https://mail.example.com/nebulamail/sw.js")
    );
    vi.stubGlobal("SW_ASSETS", []);
    vi.stubGlobal("SW_VERSION", "test");
    vi.stubGlobal("SW_BASE", "/nebulamail/");
    vi.stubGlobal("SW_TOKEN_ENDPOINT", null);
    vi.stubGlobal("SW_MESSAGES", { en: {} });
    vi.stubGlobal("addEventListener", (type: string, listener: Listener) => {
      listeners.set(type, listener);
    });
    vi.stubGlobal("clients", {
      claim: async () => {
        await Promise.resolve();
      },
      matchAll: async (options: { includeUncontrolled?: boolean }) =>
        await Promise.resolve(
          options.includeUncontrolled === true
            ? [...controlled, ...uncontrolled]
            : controlled
        ),
      openWindow,
    });
    vi.stubGlobal("registration", {
      showNotification: vi.fn<() => Promise<void>>(),
    });
    vi.stubGlobal("skipWaiting", async () => {
      await Promise.resolve();
    });
    vi.stubGlobal("caches", {
      open: async () =>
        await Promise.resolve({
          addAll: async (requests: Request[]) => {
            added.push(...requests);
            await Promise.resolve();
          },
        }),
    });
    // Not a module (no exports), so TypeScript can't type this import.
    const script = "./sw.ts";
    // oxlint-disable-next-line no-inline-comments -- Vite reads this hint inside import().
    await import(/* @vite-ignore */ script);
  });

  beforeEach(() => {
    controlled = [];
    uncontrolled = [];
    openWindow.mockClear();
  });

  describe("notificationclick", () => {
    it("navigates and focuses a controlled tab of the app", async () => {
      const app = tab("https://mail.example.com/nebulamail/mail/inbox");
      controlled = [app];
      await clickMail();

      expect(app.focus).toHaveBeenCalledOnce();
      expect(app.navigate).toHaveBeenCalledWith("/nebulamail/mail/inbox/e1");
      expect(openWindow).not.toHaveBeenCalled();
    });

    it("opens a window when the only tab isn't controlled", async () => {
      // A hard reload (Shift+F5): the tab exists but this worker doesn't
      // control it, and navigate() would reject.
      const reloaded = tab("https://mail.example.com/nebulamail/mail/inbox", {
        navigate: vi.fn<(url: string) => Promise<FakeTab | null>>(
          async () => await Promise.reject(new TypeError("no"))
        ),
      });
      uncontrolled = [reloaded];
      await clickMail();

      expect(reloaded.navigate).not.toHaveBeenCalled();
      expect(openWindow).toHaveBeenCalledWith("/nebulamail/mail/inbox/e1");
    });

    it("ignores tabs of the origin outside the app", async () => {
      const other = tab("https://mail.example.com/other/page");
      controlled = [other];
      await clickMail();

      expect(other.navigate).not.toHaveBeenCalled();
      expect(openWindow).toHaveBeenCalledOnce();
    });

    it("doesn't open a second window when focus() throws", async () => {
      const app = tab("https://mail.example.com/nebulamail/", {
        focus: vi.fn<() => Promise<FakeTab>>(
          async () => await Promise.reject(new Error("InvalidAccessError"))
        ),
      });
      controlled = [app];
      await clickMail();

      expect(app.navigate).toHaveBeenCalledOnce();
      expect(openWindow).not.toHaveBeenCalled();
    });

    it("opens a window when navigate() rejects", async () => {
      const app = tab("https://mail.example.com/nebulamail/", {
        navigate: vi.fn<(url: string) => Promise<FakeTab | null>>(
          async () => await Promise.reject(new TypeError("no"))
        ),
      });
      controlled = [app];
      await clickMail();

      expect(openWindow).toHaveBeenCalledOnce();
    });
  });

  // RFC 8620 §7.2.2: until the code is sent back, the server pushes
  // nothing else.
  describe("PushVerification", () => {
    const verification = {
      "@type": "PushVerification",
      pushSubscriptionId: "P1",
      verificationCode: "abc_DEF-123",
    };
    afterEach(() => {
      vi.stubGlobal("SW_TOKEN_ENDPOINT", null);
    });
    const pushVerification = async (): Promise<void> => {
      await fire("push", { data: { json: () => verification } });
    };

    it("is passed to the open tabs", async () => {
      const posted = fakeJmap();
      const app = tab("https://mail.example.com/nebulamail/mail/inbox");
      controlled = [app];
      await pushVerification();

      expect(app.postMessage).toHaveBeenCalledWith({
        object: verification,
        type: "nebula-push",
      });
      expect(posted).toStrictEqual([]);
    });

    it("is sent back by the worker when no tab is open", async () => {
      const posted = fakeJmap();
      await pushVerification();

      expect(posted).toStrictEqual([
        {
          methodCalls: [
            [
              "PushSubscription/set",
              { update: { P1: { verificationCode: "abc_DEF-123" } } },
              "0",
            ],
          ],
          using: ["urn:ietf:params:jmap:core"],
        },
      ]);
    });
  });

  describe("install", () => {
    it("precaches the offline files skipping the HTTP cache", async () => {
      await fire("install", {});

      expect(
        added.map((request) => [request.url, request.cache])
      ).toStrictEqual([
        [expect.stringMatching(/\/nebulamail\/offline\.html$/u), "reload"],
        [expect.stringMatching(/\/nebulamail\/offline\.css$/u), "reload"],
        [expect.stringMatching(/\/nebulamail\/favicon\.svg$/u), "reload"],
      ]);
    });
  });
});
