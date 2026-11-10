// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { describe, expect, it } from "vitest";

import { allowedImageType, checkImage, sniffImageType } from "./image";
import { bytes, SAMPLES } from "./test-fixtures";

describe(sniffImageType, () => {
  it.each([
    ["avif", "image/avif"],
    ["avifCompatible", "image/avif"],
    ["bmp", "image/bmp"],
    ["gif", "image/gif"],
    ["ico", "image/x-icon"],
    ["jpeg", "image/jpeg"],
    ["png", "image/png"],
    ["webp", "image/webp"],
  ] as const)("%s", (name, type) => {
    expect(sniffImageType(SAMPLES[name])).toBe(type);
  });

  it.each([
    ["empty", bytes()],
    ["svg", bytes('<svg xmlns="http://www.w3.org/2000/svg"/>')],
    ["svg with xml prolog", bytes('<?xml version="1.0"?><svg/>')],
    ["html", bytes("<!doctype html><img src=x onerror=alert(1)>")],
    ["truncated png", bytes([0x89, 0x50, 0x4e])],
    ["riff that is not webp", bytes("RIFF", [0, 0, 0, 0], "WAVEfmt ")],
    ["mp4 ftyp", bytes([0, 0, 0, 0x18], "ftypisom", [0, 0, 0, 0], "isomiso2")],
    ["short ico", bytes([0, 0, 1, 0])],
  ])("rejects %s", (_name, data) => {
    expect(sniffImageType(data)).toBeUndefined();
  });
});

describe(checkImage, () => {
  it("accepts a matching type, with parameters and aliases", () => {
    expect(checkImage("image/png", SAMPLES.png)).toStrictEqual({
      ok: true,
      type: "image/png",
    });
    expect(checkImage("IMAGE/JPG; charset=binary", SAMPLES.jpeg)).toStrictEqual(
      { ok: true, type: "image/jpeg" }
    );
    expect(checkImage("image/vnd.microsoft.icon", SAMPLES.ico)).toStrictEqual({
      ok: true,
      type: "image/x-icon",
    });
  });

  it.each([
    ["text/html", SAMPLES.png],
    ["image/svg+xml", bytes("<svg/>")],
    ["image/svg+xml", SAMPLES.png],
    ["image/png", SAMPLES.jpeg],
    ["image/png", bytes("<html>")],
    [undefined, SAMPLES.png],
    ["", SAMPLES.png],
    ["application/octet-stream", SAMPLES.png],
  ])("rejects %j with mismatching bytes", (type, data) => {
    expect(checkImage(type, data)).toStrictEqual({
      ok: false,
      reason: "content-type",
    });
  });

  it("never allows SVG", () => {
    expect(allowedImageType("image/svg+xml")).toBeUndefined();
    expect(allowedImageType("image/svg")).toBeUndefined();
  });
});
