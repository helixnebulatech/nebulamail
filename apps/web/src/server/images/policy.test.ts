// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { DEFAULT_ADMIN_IMAGE_POLICY } from "../../mail/image-policy";
import { ImagePolicyConfigError, imagePolicyFromEnv } from "./policy";

describe(imagePolicyFromEnv, () => {
  it("asks, through the proxy, with nothing set", () => {
    expect(imagePolicyFromEnv({}, true)).toStrictEqual(
      DEFAULT_ADMIN_IMAGE_POLICY
    );
  });

  it("allows direct by itself only without a proxy", () => {
    // With a proxy, direct would let an email point the browser at the
    // user's own network (a name like lvh.me): only if the admin says so.
    expect(imagePolicyFromEnv({}, true).direct).toBeFalsy();
    expect(imagePolicyFromEnv({}, false)).toStrictEqual({
      ...DEFAULT_ADMIN_IMAGE_POLICY,
      direct: true,
      proxy: false,
    });
    expect(
      imagePolicyFromEnv({ REMOTE_IMAGES_ALLOW_DIRECT: true }, true).direct
    ).toBeTruthy();
    expect(
      imagePolicyFromEnv({ REMOTE_IMAGES_VIA_DEFAULT: "direct" }, true).direct
    ).toBeTruthy();
    expect(
      imagePolicyFromEnv({ REMOTE_IMAGES_ALLOW_DIRECT: false }, false).direct
    ).toBeFalsy();
  });

  it("takes the admin's defaults", () => {
    expect(
      imagePolicyFromEnv(
        {
          REMOTE_IMAGES_DEFAULT: "trusted-senders",
          REMOTE_IMAGES_VIA_DEFAULT: "direct",
        },
        true
      )
    ).toStrictEqual({
      defaultLoad: "trusted-senders",
      defaultRoute: "direct",
      direct: true,
      forceNever: false,
      proxy: true,
    });
  });

  it("forbids direct", () => {
    expect(
      imagePolicyFromEnv({ REMOTE_IMAGES_ALLOW_DIRECT: false }, true).direct
    ).toBeFalsy();
  });

  it("forces never", () => {
    expect(
      imagePolicyFromEnv({ REMOTE_IMAGES_FORCE_NEVER: true }, true)
    ).toMatchObject({ defaultLoad: "never", forceNever: true });
    expect(
      imagePolicyFromEnv(
        { REMOTE_IMAGES_DEFAULT: "never", REMOTE_IMAGES_FORCE_NEVER: true },
        true
      ).forceNever
    ).toBeTruthy();
  });

  it("stops the server on a contradiction", () => {
    expect(() =>
      imagePolicyFromEnv(
        {
          REMOTE_IMAGES_ALLOW_DIRECT: false,
          REMOTE_IMAGES_VIA_DEFAULT: "direct",
        },
        true
      )
    ).toThrow(ImagePolicyConfigError);
    expect(() =>
      imagePolicyFromEnv({ REMOTE_IMAGES_VIA_DEFAULT: "proxy" }, false)
    ).toThrow(/needs an image proxy/u);
    expect(() =>
      imagePolicyFromEnv(
        { REMOTE_IMAGES_DEFAULT: "always", REMOTE_IMAGES_FORCE_NEVER: true },
        true
      )
    ).toThrow(/FORCE_NEVER/u);
  });
});
