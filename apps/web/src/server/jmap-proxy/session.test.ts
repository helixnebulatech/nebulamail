// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { makeUpstream } from "./config";
import { ProxyError } from "./errors";
import {
  MAX_PROXIED_REQUEST,
  browserSession,
  expandUpstream,
  upstreamEndpoints,
} from "./session";
import { stalwartSession } from "./test-fixtures";

const PUBLIC = "https://mail.example.com";
const INTERNAL = "http://stalwart:8080";
const upstream = makeUpstream(new URL(PUBLIC), new URL(INTERNAL));
const SESSION_URL = new URL("/jmap/session", INTERNAL);
const APP = new URL("https://nebulamail.example.com");

describe(upstreamEndpoints, () => {
  it("moves the public URLs to the internal origin", () => {
    const endpoints = upstreamEndpoints(
      stalwartSession(),
      SESSION_URL,
      upstream
    );
    expect(endpoints).toStrictEqual({
      apiUrl: new URL(`${INTERNAL}/jmap/`),
      downloadUrl: `${INTERNAL}/jmap/download/{accountId}/{blobId}/{name}?accept={type}`,
      eventSourceUrl: `${INTERNAL}/jmap/eventsource/?types={types}&closeafter={closeafter}&ping={ping}`,
      maxSizeRequest: 10_000_000,
      maxSizeUpload: 50_000_000,
      uploadUrl: `${INTERNAL}/jmap/upload/{accountId}/`,
    });
  });

  it("never reads more than its own cap of an API body in memory", () => {
    const session = stalwartSession({
      capabilities: {
        "urn:ietf:params:jmap:core": {
          maxSizeRequest: 1e15,
          maxSizeUpload: 50_000_000,
        },
        "urn:ietf:params:jmap:mail": {},
      },
    });
    expect(
      upstreamEndpoints(session, SESSION_URL, upstream).maxSizeRequest
    ).toBe(MAX_PROXIED_REQUEST);
  });

  it("takes relative URLs and the internal origin too", () => {
    const endpoints = upstreamEndpoints(
      stalwartSession({
        apiUrl: "/jmap/",
        uploadUrl: `${INTERNAL}/jmap/upload/{accountId}/`,
      }),
      SESSION_URL,
      upstream
    );
    expect(endpoints.apiUrl.href).toBe(`${INTERNAL}/jmap/`);
    expect(endpoints.uploadUrl).toBe(`${INTERNAL}/jmap/upload/{accountId}/`);
  });

  it("refuses a hostile session with URLs on other servers", () => {
    for (const overrides of [
      { apiUrl: "https://evil.example/jmap/" },
      { apiUrl: "//evil.example/jmap/" },
      { apiUrl: "https://mail.example.com.evil.example/jmap/" },
      { apiUrl: "https://user:pass@mail.example.com/jmap/" },
      { apiUrl: "http://mail.example.com/jmap/" },
      // oxlint-disable-next-line no-script-url -- a hostile value to refuse
      { apiUrl: "javascript:alert(1)" },
      { apiUrl: 42 },
      { downloadUrl: "https://evil.example/{accountId}/{blobId}" },
      { downloadUrl: "//evil.example/{accountId}/{blobId}" },
      { downloadUrl: "https://{accountId}.evil.example/{blobId}" },
      { downloadUrl: `${PUBLIC}@evil.example/{accountId}/{blobId}` },
      { downloadUrl: `${PUBLIC}.evil.example/{accountId}/{blobId}` },
      { downloadUrl: `${PUBLIC}/jmap/download/{blobId}` },
      { downloadUrl: `${PUBLIC}/jmap/{accountId}/{blobId}/{a{b}}` },
      { uploadUrl: "https://evil.example/upload/{accountId}" },
      { uploadUrl: undefined },
      { eventSourceUrl: "https://evil.example/events" },
      { eventSourceUrl: "wss://mail.example.com/events" },
    ]) {
      expect(() =>
        upstreamEndpoints(stalwartSession(overrides), SESSION_URL, upstream)
      ).toThrow(ProxyError);
    }
    expect(() => upstreamEndpoints("nope", SESSION_URL, upstream)).toThrow(
      ProxyError
    );
  });
});

describe(browserSession, () => {
  it("points every URL at the proxy and drops the WebSocket", () => {
    const session = browserSession(stalwartSession(), APP);
    expect(session.apiUrl).toBe(`${APP.origin}/jmap-proxy/api`);
    expect(session.downloadUrl).toBe(
      `${APP.origin}/jmap-proxy/download/{accountId}/{blobId}/{name}?type={type}`
    );
    expect(session.uploadUrl).toBe(
      `${APP.origin}/jmap-proxy/upload/{accountId}`
    );
    expect(session.eventSourceUrl).toBe(
      `${APP.origin}/jmap-proxy/eventsource?types={types}&closeafter={closeafter}&ping={ping}`
    );
    // Nothing of the JMAP server's own addresses is left.
    const text = JSON.stringify(session);
    expect(text).not.toMatch(/\/\/mail\.example\.com|stalwart/u);
  });

  it("keeps the rest of the session, without the WebSocket", () => {
    const session = browserSession(stalwartSession(), APP);
    expect(session.capabilities).not.toHaveProperty(
      "urn:ietf:params:jmap:websocket"
    );
    expect(session.capabilities).toHaveProperty("urn:ietf:params:jmap:mail");
    expect(session.primaryAccounts).toStrictEqual({
      "urn:ietf:params:jmap:mail": "c",
    });
  });
});

describe(expandUpstream, () => {
  const template = `${INTERNAL}/jmap/download/{accountId}/{blobId}/{name}?accept={type}`;

  it("re-encodes every value", () => {
    expect(
      expandUpstream(
        template,
        {
          accountId: "c",
          blobId: "G1",
          name: "../a b/\r\n?#.pdf",
          type: "text/plain",
        },
        upstream
      )?.href
    ).toBe(
      `${INTERNAL}/jmap/download/c/G1/..%2Fa%20b%2F%0D%0A%3F%23.pdf?accept=text%2Fplain`
    );
  });

  it("refuses anything that ends up on another origin", () => {
    expect(
      expandUpstream("https://evil.example/{x}", { x: "y" }, upstream)
    ).toBeUndefined();
  });
});
