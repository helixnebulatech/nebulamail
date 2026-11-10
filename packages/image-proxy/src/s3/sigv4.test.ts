// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// SigV4 against AWS's published vectors:
// - the "aws-sig-v4-test-suite" (as vendored in botocore,
//   tests/unit/auth/aws4_testsuite): service "service", us-east-1,
//   20150830T123600Z, AKIDEXAMPLE;
// - the four S3 examples of "Signature Calculations for the Authorization
//   Header: Transferring Payload in a Single Chunk" (S3 API reference):
//   examplebucket, 20130524T000000Z, AKIAIOSFODNN7EXAMPLE.

import { describe, expect, it } from "vitest";

import {
  amzDate,
  EMPTY_SHA256,
  sha256Hex,
  signRequest,
  uriEncode,
} from "./sigv4";

const SUITE = {
  credentials: {
    accessKeyId: "AKIDEXAMPLE",
    secretAccessKey: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
  },
  date: new Date("2015-08-30T12:36:00Z"),
  region: "us-east-1",
  service: "service",
} as const;

const S3 = {
  credentials: {
    accessKeyId: "AKIAIOSFODNN7EXAMPLE",
    secretAccessKey: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
  },
  date: new Date("2013-05-24T00:00:00Z"),
  region: "us-east-1",
  service: "s3",
} as const;

const suiteCase = async (
  method: string,
  path: string,
  headers: Record<string, string> = {},
  body = ""
) =>
  await signRequest({
    ...SUITE,
    doubleEncodePath: true,
    headers: { Host: "example.amazonaws.com", ...headers },
    method,
    payloadHash: await sha256Hex(body),
    url: new URL(`https://example.amazonaws.com${path}`),
  });

describe("aws-sig-v4-test-suite", () => {
  it("get-vanilla", async () => {
    const signed = await suiteCase("GET", "/");
    expect(signed.canonicalRequest).toBe(
      [
        "GET",
        "/",
        "",
        "host:example.amazonaws.com",
        "x-amz-date:20150830T123600Z",
        "",
        "host;x-amz-date",
        EMPTY_SHA256,
      ].join("\n")
    );
    expect(signed.headers.authorization).toBe(
      "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
    );
  });

  it.each([
    [
      "get-vanilla-query-order-key-case",
      "GET",
      "/?Param2=value2&Param1=value1",
      {},
      "",
      "b97d918cfa904a5beff61c982a1b6f458b799221646efd99d3219ec94cdf2500",
    ],
    [
      "get-unreserved",
      "GET",
      "/-._~0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
      {},
      "",
      "07ef7494c76fa4850883e2b006601f940f8a34d404d0cfa977f52a65bbf5f24f",
    ],
    [
      "get-vanilla-utf8-query",
      "GET",
      "/?ሴ=bar",
      {},
      "",
      "2cdec8eed098649ff3a119c94853b13c643bcf08f8b0a1d91e12c9027818dd04",
    ],
    [
      "get-header-value-trim",
      "GET",
      "/",
      { "My-Header1": " value1", "My-Header2": ' "a   b   c"' },
      "",
      "acc3ed3afb60bb290fc8d2dd0098b9911fcaa05412b367055dee359757a9c736",
    ],
    [
      "post-header-key-sort",
      "POST",
      "/",
      { "My-Header1": "value1" },
      "",
      "c5410059b04c1ee005303aed430f6e6645f61f4dc9e1461ec8f8916fdf18852c",
    ],
    [
      "post-x-www-form-urlencoded",
      "POST",
      "/",
      { "Content-Type": "application/x-www-form-urlencoded" },
      "Param1=value1",
      "ff11897932ad3f4e8b18135d722051e5ac45fc38421b1da7b9d196a0fe09473a",
    ],
  ])("%s", async (_name, method, path, headers, body, signature) => {
    const signed = await suiteCase(method, path, headers, body);
    expect(signed.signature).toBe(signature);
  });

  it("canonical query of get-vanilla-utf8-query", async () => {
    const signed = await suiteCase("GET", "/?ሴ=bar");
    expect(signed.canonicalRequest.split("\n")[2]).toBe("%E1%88%B4=bar");
  });
});

describe("S3 examples (single chunk)", () => {
  const host = "examplebucket.s3.amazonaws.com";

  it("GET Object", async () => {
    const signed = await signRequest({
      ...S3,
      headers: {
        range: "bytes=0-9",
        "x-amz-content-sha256": EMPTY_SHA256,
      },
      method: "GET",
      payloadHash: EMPTY_SHA256,
      url: new URL(`https://${host}/test.txt`),
    });
    expect(signed.headers.authorization).toBe(
      "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
    );
  });

  it("PUT Object", async () => {
    const payloadHash = await sha256Hex("Welcome to Amazon S3.");
    expect(payloadHash).toBe(
      "44ce7dd67c959e0d3524ffac1771dfbba87d2b6b4b4e99e42034a8b803f8b072"
    );
    const signed = await signRequest({
      ...S3,
      headers: {
        date: "Fri, 24 May 2013 00:00:00 GMT",
        "x-amz-content-sha256": payloadHash,
        "x-amz-storage-class": "REDUCED_REDUNDANCY",
      },
      method: "PUT",
      payloadHash,
      url: new URL(`https://${host}/test$file.text`),
    });
    expect(signed.canonicalRequest.split("\n")[1]).toBe("/test%24file.text");
    expect(signed.signature).toBe(
      "98ad721746da40c64f1a55b78f14c238d841ea1380cd77a1b5971af0ece108bd"
    );
  });

  it("GET Bucket Lifecycle", async () => {
    const signed = await signRequest({
      ...S3,
      headers: { "x-amz-content-sha256": EMPTY_SHA256 },
      method: "GET",
      payloadHash: EMPTY_SHA256,
      url: new URL(`https://${host}/?lifecycle`),
    });
    expect(signed.canonicalRequest.split("\n")[2]).toBe("lifecycle=");
    expect(signed.signature).toBe(
      "fea454ca298b7da1c68078a5d1bdbfbbe0d65c699e0f91ac7a200a0136783543"
    );
  });

  it("Get Bucket (List Objects)", async () => {
    const signed = await signRequest({
      ...S3,
      headers: { "x-amz-content-sha256": EMPTY_SHA256 },
      method: "GET",
      payloadHash: EMPTY_SHA256,
      url: new URL(`https://${host}/?max-keys=2&prefix=J`),
    });
    expect(signed.signature).toBe(
      "34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7"
    );
  });
});

describe("helpers", () => {
  it("encodes like RFC 3986 and formats the date", () => {
    expect(uriEncode("a b/ñ~*")).toBe("a%20b%2F%C3%B1~%2A");
    expect(amzDate(new Date("2015-08-30T12:36:00.123Z"))).toBe(
      "20150830T123600Z"
    );
  });

  it("adds the session token as a signed header", async () => {
    const signed = await signRequest({
      ...S3,
      credentials: { ...S3.credentials, sessionToken: "token" },
      headers: {},
      method: "GET",
      payloadHash: EMPTY_SHA256,
      url: new URL("https://examplebucket.s3.amazonaws.com/"),
    });
    expect(signed.headers["x-amz-security-token"]).toBe("token");
    expect(signed.headers.authorization).toContain(
      "SignedHeaders=host;x-amz-date;x-amz-security-token"
    );
  });
});
