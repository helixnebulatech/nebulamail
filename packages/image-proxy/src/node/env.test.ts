// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import { KEY_1 } from "../test-fixtures";
import {
  EnvError,
  parseServerEnv,
  pickServerEnv,
  SERVER_ENV_NAMES,
} from "./env";

describe(parseServerEnv, () => {
  it("defaults", () => {
    expect(
      parseServerEnv({
        APP_URL: "http://localhost:3000/app",
        IMAGE_PROXY_KEYS: KEY_1,
      })
    ).toStrictEqual({
      appOrigin: "http://localhost:3000",
      cacheDir: undefined,
      cacheMaxBytes: undefined,
      cacheTtlMs: undefined,
      clientIp: "socket",
      host: "127.0.0.1",
      keys: KEY_1,
      port: 3003,
      preload: undefined,
      s3: undefined,
    });
  });

  it("client address source: socket by default, only known headers", () => {
    const base = {
      APP_URL: "https://mail.example.com",
      IMAGE_PROXY_KEYS: KEY_1,
    };
    expect(
      parseServerEnv({ ...base, IMAGE_PROXY_CLIENT_IP: "x-forwarded-for" })
        .clientIp
    ).toBe("x-forwarded-for");
    expect(
      parseServerEnv({ ...base, IMAGE_PROXY_CLIENT_IP: "x-real-ip" }).clientIp
    ).toBe("x-real-ip");
    expect(() =>
      parseServerEnv({ ...base, IMAGE_PROXY_CLIENT_IP: "forwarded" })
    ).toThrow(EnvError);
  });

  it("cache dir and port", () => {
    const env = parseServerEnv({
      APP_URL: "https://mail.example.com",
      IMAGE_CACHE_DIR: "/var/cache/img",
      IMAGE_CACHE_MAX_BYTES: "1000000",
      IMAGE_PROXY_KEYS: KEY_1,
      IMAGE_PROXY_PORT: "8080",
    });
    expect(env).toMatchObject({
      cacheDir: "/var/cache/img",
      cacheMaxBytes: 1_000_000,
      port: 8080,
    });
  });

  it("the disk cache always has a cap: 10 GiB by default, positive", () => {
    const base = {
      APP_URL: "https://mail.example.com",
      IMAGE_CACHE_DIR: "/var/cache/img",
      IMAGE_PROXY_KEYS: KEY_1,
    };
    expect(parseServerEnv(base).cacheMaxBytes).toBe(10 * 2 ** 30);
    for (const bad of ["0", "-1", "1.5", "lots"]) {
      expect(() =>
        parseServerEnv({ ...base, IMAGE_CACHE_MAX_BYTES: bad })
      ).toThrow(EnvError);
    }
  });

  it("retention in whole days, from 1, no maximum", () => {
    const base = {
      APP_URL: "https://mail.example.com",
      IMAGE_PROXY_KEYS: KEY_1,
    };
    expect(
      parseServerEnv({ ...base, IMAGE_CACHE_TTL_DAYS: "7" }).cacheTtlMs
    ).toBe(7 * 24 * 60 * 60 * 1000);
    expect(
      parseServerEnv({ ...base, IMAGE_CACHE_TTL_DAYS: "3650" }).cacheTtlMs
    ).toBe(3650 * 24 * 60 * 60 * 1000);
    for (const bad of ["0", "-1", "1.5", "ten"]) {
      expect(() =>
        parseServerEnv({ ...base, IMAGE_CACHE_TTL_DAYS: bad })
      ).toThrow(/IMAGE_CACHE_TTL_DAYS/u);
    }
  });

  it("preload quota per recipient and index key", () => {
    const base = {
      APP_URL: "https://mail.example.com",
      IMAGE_CACHE_DIR: "/var/cache/img",
      IMAGE_PRELOAD_SECRET: "p".repeat(40),
      IMAGE_PROXY_KEYS: KEY_1,
    };
    expect(parseServerEnv(base).preload).toStrictEqual({
      emailTokens: false,
      fullEmail: false,
      indexKey: undefined,
      maxBytesPerRecipient: undefined,
      maxImagesPerRecipient: undefined,
      maxMessageBytes: undefined,
      secret: "p".repeat(40),
    });
    expect(
      parseServerEnv({
        ...base,
        IMAGE_PRELOAD_INDEX_KEY: "i".repeat(32),
        IMAGE_PRELOAD_MAX_BYTES_PER_RECIPIENT: "1000",
        IMAGE_PRELOAD_EMAIL_TOKENS: "true",
        IMAGE_PRELOAD_FULL_EMAIL: "true",
        IMAGE_PRELOAD_MAX_IMAGES_PER_RECIPIENT: "50",
      }).preload
    ).toMatchObject({
      emailTokens: true,
      fullEmail: true,
      indexKey: "i".repeat(32),
      maxBytesPerRecipient: 1000,
      maxImagesPerRecipient: 50,
    });
    for (const [name, bad] of [
      ["IMAGE_PRELOAD_INDEX_KEY", "short"],
      ["IMAGE_PRELOAD_MAX_BYTES_PER_RECIPIENT", "0"],
      ["IMAGE_PRELOAD_MAX_IMAGES_PER_RECIPIENT", "-1"],
    ] as const) {
      expect(() => parseServerEnv({ ...base, [name]: bad })).toThrow(
        new RegExp(name, "u")
      );
    }
  });

  it("names missing variables without echoing values", () => {
    expect(() =>
      parseServerEnv({ APP_URL: "ftp://x", IMAGE_CACHE_DIR: "" })
    ).toThrow(EnvError);
    const secret = "not-a-url-secret-value";
    const run = () => parseServerEnv({ APP_URL: secret, IMAGE_PROXY_KEYS: "" });
    expect(run).toThrow(/APP_URL/u);
    expect(run).toThrow(/IMAGE_PROXY_KEYS/u);
    expect(run).not.toThrow(/secret-value/u);
  });
});

describe("variables read by name", () => {
  it("pickServerEnv keeps only the server's variables", () => {
    expect(
      pickServerEnv({ APP_URL: "https://a.example", HTTP_PROXY: "x" })
    ).not.toHaveProperty("HTTP_PROXY");
  });

  it("scripts/start-deno.sh allows exactly the server's variables", () => {
    const script = readFileSync(
      new URL("../../scripts/start-deno.sh", import.meta.url),
      "utf-8"
    );
    const list = /PROXY_ENV="(?<names>[^"]+)"/u
      .exec(script)
      ?.groups?.names?.replaceAll("\\\n", "");
    expect(list?.split(",").toSorted()).toStrictEqual(
      [...SERVER_ENV_NAMES].toSorted()
    );
  });
});
