// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// S3 storage against a fake S3 (a `fetch` that keeps objects in a Map and
// checks every request is signed), plus, when NEBULA_TEST_S3_ENDPOINT is
// set, the same round trip against a real server (MinIO, Garage, AWS):
//
//   NEBULA_TEST_S3_ENDPOINT=http://127.0.0.1:9000 NEBULA_TEST_S3_BUCKET=test \
//   NEBULA_TEST_S3_ACCESS_KEY_ID=… NEBULA_TEST_S3_SECRET_ACCESS_KEY=… bun run test

import { describe, expect, it } from "vitest";

import { getRecord, putRecord } from "../preload/records";
import { gif, png, text } from "../test-fixtures";
import { parseS3Env } from "./env";
import {
  createS3StateStore,
  createS3Storage,
  parseListing,
} from "./s3-storage";
import type { S3Fetch, S3StorageOptions } from "./s3-storage";

const DAY = 24 * 60 * 60 * 1000;
const KEY = `v1:${"a".repeat(64)}:${"b".repeat(64)}`;
const KEY_2 = `v1:${"c".repeat(64)}:${"d".repeat(64)}`;
const CREDENTIALS = { accessKeyId: "AKID", secretAccessKey: "secret" };

interface FakeObject {
  readonly bytes: Uint8Array;
  readonly type: string;
  readonly modified: number;
}

const fakeS3 = (clock: () => number = Date.now, pageSize = 1000) => {
  const objects = new Map<string, FakeObject>();
  const requests: { method: string; url: string; headers: Headers }[] = [];
  const fetch: S3Fetch = async (input, init) => {
    const headers = new Headers(init.headers);
    const method = init.method ?? "GET";
    requests.push({ headers, method, url: input });
    if (
      !(headers.get("authorization") ?? "").startsWith(
        "AWS4-HMAC-SHA256 Credential=AKID/"
      )
    ) {
      return new Response("<Error/>", { status: 403 });
    }
    expect(init.redirect).toBe("manual");
    const url = new URL(input);
    const [, bucket, ...rest] = url.pathname.split("/");
    expect(bucket).toBe("images");
    const key = rest.join("/");
    if (method === "GET" && key === "") {
      const keys = [...objects.keys()].toSorted();
      const after = url.searchParams.get("continuation-token");
      const start = after === null ? 0 : keys.indexOf(after) + 1;
      const page = keys.slice(start, start + pageSize);
      const more = start + pageSize < keys.length;
      const contents = page
        .map(
          (k) =>
            `<Contents><Key>${k}</Key><LastModified>${new Date(objects.get(k)?.modified ?? 0).toISOString()}</LastModified></Contents>`
        )
        .join("");
      return new Response(
        `<?xml version="1.0"?><ListBucketResult>${contents}<IsTruncated>${String(more)}</IsTruncated>${more ? `<NextContinuationToken>${page.at(-1) ?? ""}</NextContinuationToken>` : ""}</ListBucketResult>`,
        { status: 200 }
      );
    }
    if (method === "PUT") {
      const body = new Uint8Array(await new Response(init.body).arrayBuffer());
      objects.set(key, {
        bytes: body,
        modified: clock(),
        type: headers.get("content-type") ?? "",
      });
      return new Response(null, { status: 200 });
    }
    const object = objects.get(key);
    if (method === "DELETE") {
      objects.delete(key);
      return new Response(null, { status: 204 });
    }
    if (object === undefined) {
      return new Response("<Error><Key>secret-key</Key></Error>", {
        status: 404,
      });
    }
    return new Response(Uint8Array.from(object.bytes), {
      headers: {
        "content-length": String(object.bytes.byteLength),
        "content-type": object.type,
        "last-modified": new Date(object.modified).toUTCString(),
      },
      status: 200,
    });
  };
  return { fetch, objects, requests };
};

const storageWith = (
  fake: ReturnType<typeof fakeS3>,
  extra: Partial<S3StorageOptions> = {}
) =>
  createS3Storage({
    bucket: "images",
    credentials: CREDENTIALS,
    endpoint: "http://minio.internal:9000",
    fetch: fake.fetch,
    ...extra,
  });

describe("S3 storage (fake S3)", () => {
  it("stores and reads back; keys are hashes under img/", async () => {
    const fake = fakeS3();
    const storage = storageWith(fake);
    await expect(storage.get(KEY)).resolves.toBeNull();
    await storage.put(KEY, { bytes: png(), type: "image/png" }, DAY);
    const stored = await storage.get(KEY);
    expect(stored).toMatchObject({ bytes: png(), type: "image/png" });
    // The proxy never lets browsers keep it longer (RFC 9111).
    expect(stored?.expiresAt).toBeTypeOf("number");
    expect([...fake.objects.keys()]).toStrictEqual([
      `img/v1/${"a".repeat(64)}/${"b".repeat(64)}`,
    ]);
    const put = fake.requests.find((r) => r.method === "PUT");
    expect({
      host: put?.headers.get("host") ?? null,
      sha: /^[\da-f]{64}$/u.test(
        put?.headers.get("x-amz-content-sha256") ?? ""
      ),
      url: put?.url,
    }).toStrictEqual({
      host: null,
      sha: true,
      url: `http://minio.internal:9000/images/img/v1/${"a".repeat(64)}/${"b".repeat(64)}`,
    });
  });

  it("virtual-hosted style puts the bucket in the host name", async () => {
    const requests: string[] = [];
    const storage = createS3Storage({
      bucket: "images",
      credentials: CREDENTIALS,
      endpoint: "https://s3.eu-west-1.amazonaws.com",
      fetch: async (input) => {
        requests.push(input);
        return await Promise.resolve(new Response(null, { status: 404 }));
      },
      region: "eu-west-1",
      urlStyle: "virtual",
    });
    await storage.get(KEY);
    expect(requests).toStrictEqual([
      `https://images.s3.eu-west-1.amazonaws.com/img/v1/${"a".repeat(64)}/${"b".repeat(64)}`,
    ]);
  });

  it("expires maxAgeMs after upload (any value) and deletes on read", async () => {
    let now = Date.UTC(2026, 0, 1);
    const fake = fakeS3(() => now);
    const storage = storageWith(fake, { maxAgeMs: 400 * DAY, now: () => now });
    await storage.put(KEY, { bytes: png(), type: "image/png" }, 400 * DAY);
    now += 399 * DAY;
    await expect(storage.get(KEY)).resolves.not.toBeNull();
    now += DAY;
    await expect(storage.get(KEY)).resolves.toBeNull();
    expect(fake.objects.size).toBe(0);
  });

  it("treats oversized or non-image objects as missing", async () => {
    const fake = fakeS3();
    const storage = storageWith(fake, { maxBytes: 100 });
    await storage.put(KEY, { bytes: png(200), type: "image/png" }, DAY);
    await expect(storage.get(KEY)).resolves.toBeNull();
    await storage.put(KEY_2, { bytes: gif(), type: "image/svg+xml" }, DAY);
    await expect(storage.get(KEY_2)).resolves.toBeNull();
  });

  it("keeps preload records (email tokens), still drops other types", async () => {
    const fake = fakeS3();
    const storage = storageWith(fake);
    const hashes = ["1".repeat(64), "2".repeat(64)];
    await putRecord(storage, "e".repeat(64), hashes, DAY);
    await expect(getRecord(storage, "e".repeat(64))).resolves.toStrictEqual(
      new Set(hashes)
    );
    await storage.put(KEY, { bytes: text("<b>x</b>"), type: "text/html" }, DAY);
    await expect(storage.get(KEY)).resolves.toBeNull();
  });

  it("sweep deletes expired entries across pages", async () => {
    let now = Date.UTC(2026, 0, 1);
    const fake = fakeS3(() => now, 2);
    const storage = storageWith(fake, { maxAgeMs: DAY, now: () => now });
    for (const c of ["1", "2", "3", "4", "5"]) {
      // oxlint-disable-next-line no-await-in-loop
      await storage.put(
        `v1:${c.repeat(64)}:${"f".repeat(64)}`,
        { bytes: png(), type: "image/png" },
        DAY
      );
      now += 1000;
    }
    now += DAY - 2500;
    // Entries 1–3 are older than a day now; 4 and 5 are not.
    await expect(storage.sweep()).resolves.toBe(3);
    expect(fake.objects.size).toBe(2);
  });

  it("errors carry the status only; redirects are refused", async () => {
    const storage = createS3Storage({
      bucket: "images",
      credentials: CREDENTIALS,
      endpoint: "http://minio.internal:9000",
      fetch: async () =>
        await Promise.resolve(
          new Response("<Error><Key>img/v1/secret</Key></Error>", {
            status: 500,
          })
        ),
    });
    await expect(storage.get(KEY)).rejects.toThrow(
      /^image proxy: S3 GET failed with 500$/u
    );
    const redirecting = createS3Storage({
      bucket: "images",
      credentials: CREDENTIALS,
      endpoint: "http://minio.internal:9000",
      fetch: async () =>
        await Promise.resolve(
          new Response(null, {
            headers: { location: "http://169.254.169.254/" },
            status: 307,
          })
        ),
    });
    await expect(redirecting.get(KEY)).rejects.toThrow(/redirected/u);
  });

  it("refuses bad endpoints, buckets, retention and keys", async () => {
    const base = { bucket: "images", credentials: CREDENTIALS };
    for (const endpoint of [
      "ftp://x",
      "http://x/path",
      "http://u:p@x",
      "http://x/?a=1",
    ]) {
      expect(() => createS3Storage({ ...base, endpoint })).toThrow(TypeError);
    }
    expect(() =>
      createS3Storage({ ...base, bucket: "A_B", endpoint: "http://x" })
    ).toThrow(TypeError);
    expect(() =>
      createS3Storage({ ...base, endpoint: "http://x", maxAgeMs: 0 })
    ).toThrow(TypeError);
    const storage = storageWith(fakeS3());
    await expect(storage.get("https://evil.example/x")).rejects.toThrow(
      TypeError
    );
  });

  it("parses listings with entities and skips broken entries", () => {
    expect(
      parseListing(
        "<ListBucketResult><Contents><Key>img/a&amp;b</Key><LastModified>2026-01-01T00:00:00.000Z</LastModified></Contents><Contents><Key>x</Key></Contents><IsTruncated>true</IsTruncated><NextContinuationToken>t+/=&amp;</NextContinuationToken></ListBucketResult>"
      )
    ).toStrictEqual({
      next: "t+/=&",
      objects: [{ key: "img/a&b", lastModified: Date.UTC(2026, 0, 1) }],
    });
  });
});

describe("S3 preload state and delete", () => {
  const STATE = `prefs:${"e".repeat(64)}`;

  it("keeps state under pre/v1/, outside img/, and the sweep leaves it", async () => {
    let now = 0;
    const fake = fakeS3(() => now);
    const state = createS3StateStore({
      bucket: "images",
      credentials: CREDENTIALS,
      endpoint: "http://minio.internal:9000",
      fetch: fake.fetch,
    });
    await state.put(STATE, '{"v":1}');
    expect([...fake.objects.keys()]).toStrictEqual([
      `pre/v1/prefs/${"e".repeat(64)}`,
    ]);
    await expect(state.get(STATE)).resolves.toBe('{"v":1}');
    now = 400 * DAY;
    await storageWith(fake, { now: () => now }).sweep();
    await expect(state.get(STATE)).resolves.toBe('{"v":1}');
    await state.delete(STATE);
    await expect(state.get(STATE)).resolves.toBeNull();
    await expect(state.get("prefs:alice@example.org")).rejects.toThrow(
      TypeError
    );
  });

  it("lists the state keys of one kind across pages", async () => {
    const fake = fakeS3(Date.now, 2);
    const state = createS3StateStore({
      bucket: "images",
      credentials: CREDENTIALS,
      endpoint: "http://minio.internal:9000",
      fetch: fake.fetch,
    });
    for (const c of ["a", "b", "c"]) {
      // oxlint-disable-next-line no-await-in-loop
      await state.put(`usage:${c.repeat(64)}`, "{}");
    }
    await state.put(`refs:${"d".repeat(64)}`, "{}");
    await state.put(`prefs:${"e".repeat(64)}`, "{}");
    await storageWith(fake).put(
      `v1:${"1".repeat(64)}:${"2".repeat(64)}`,
      { bytes: png(), type: "image/png" },
      DAY
    );
    fake.objects.set("pre/v1/usage/junk", {
      bytes: new Uint8Array(),
      modified: 0,
      type: "application/json",
    });
    const keys: string[] = [];
    for await (const key of state.list("usage")) {
      keys.push(key);
    }
    expect(keys).toStrictEqual(
      ["a", "b", "c"].map((c) => `usage:${c.repeat(64)}`)
    );
  });

  it("deletes one image", async () => {
    const storage = storageWith(fakeS3());
    await storage.put(KEY, { bytes: png(), type: "image/png" }, DAY);
    await storage.delete?.(KEY);
    await expect(storage.get(KEY)).resolves.toBeNull();
  });
});

describe(parseS3Env, () => {
  const full = {
    IMAGE_CACHE_S3_ACCESS_KEY_ID: "AKID",
    IMAGE_CACHE_S3_BUCKET: "images",
    IMAGE_CACHE_S3_ENDPOINT: "http://minio:9000",
    IMAGE_CACHE_S3_SECRET_ACCESS_KEY: "very-secret-value",
  };

  it("is off without a bucket; defaults region and URL style", () => {
    expect(parseS3Env({})).toBeUndefined();
    expect(parseS3Env(full)).toStrictEqual({
      bucket: "images",
      credentials: {
        accessKeyId: "AKID",
        secretAccessKey: "very-secret-value",
      },
      endpoint: "http://minio:9000",
      region: "us-east-1",
      urlStyle: "path",
    });
  });

  it("names what is missing or wrong, never values", () => {
    expect(() =>
      parseS3Env({ ...full, IMAGE_CACHE_S3_SECRET_ACCESS_KEY: undefined })
    ).toThrow(/IMAGE_CACHE_S3_SECRET_ACCESS_KEY/u);
    expect(() =>
      parseS3Env({
        ...full,
        IMAGE_CACHE_S3_ENDPOINT: "http://minio:9000/very-secret-value",
      })
    ).toThrow(/IMAGE_CACHE_S3_ENDPOINT/u);
    expect(() =>
      parseS3Env({
        ...full,
        IMAGE_CACHE_S3_ENDPOINT: "http://minio:9000/very-secret-value",
      })
    ).not.toThrow(/very-secret/u);
    expect(() =>
      parseS3Env({ ...full, IMAGE_CACHE_S3_URL_STYLE: "x" })
    ).toThrow(/IMAGE_CACHE_S3_URL_STYLE/u);
  });
});

const live = process.env.NEBULA_TEST_S3_ENDPOINT;

describe.skipIf(live === undefined)("S3 storage (live server)", () => {
  it("put, get, sweep against the real server", async () => {
    const storage = createS3Storage({
      bucket: process.env.NEBULA_TEST_S3_BUCKET ?? "test",
      credentials: {
        accessKeyId: process.env.NEBULA_TEST_S3_ACCESS_KEY_ID ?? "",
        secretAccessKey: process.env.NEBULA_TEST_S3_SECRET_ACCESS_KEY ?? "",
      },
      endpoint: live ?? "",
      region: process.env.NEBULA_TEST_S3_REGION ?? "us-east-1",
    });
    const before = Date.now() - 1000;
    await storage.put(KEY, { bytes: png(1000), type: "image/png" }, DAY);
    const stored = await storage.get(KEY);
    // `expiresAt`: `maxAgeMs` (here the default) from the upload, whatever
    // the ttl, read from the server's Last-Modified (whole seconds).
    expect({
      ...stored,
      expiresAt: (stored?.expiresAt ?? 0) > before,
    }).toStrictEqual({
      bytes: png(1000),
      expiresAt: true,
      type: "image/png",
    });
    await expect(storage.get(KEY_2)).resolves.toBeNull();
    // Nothing is a day old yet.
    await expect(storage.sweep()).resolves.toBe(0);
    const later = createS3Storage({
      bucket: process.env.NEBULA_TEST_S3_BUCKET ?? "test",
      credentials: {
        accessKeyId: process.env.NEBULA_TEST_S3_ACCESS_KEY_ID ?? "",
        secretAccessKey: process.env.NEBULA_TEST_S3_SECRET_ACCESS_KEY ?? "",
      },
      endpoint: live ?? "",
      maxAgeMs: DAY,
      now: () => Date.now() + 2 * DAY,
      region: process.env.NEBULA_TEST_S3_REGION ?? "us-east-1",
    });
    await expect(later.sweep()).resolves.toBeGreaterThanOrEqual(1);
    await expect(storage.get(KEY)).resolves.toBeNull();
  });
});
