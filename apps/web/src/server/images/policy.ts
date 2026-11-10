// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The admin's side of the remote images settings
// from the environment (.env.schema):
//   REMOTE_IMAGES_DEFAULT       never | ask | trusted-senders | always (ask)
//   REMOTE_IMAGES_VIA_DEFAULT   proxy | direct (the proxy if there is one)
//   REMOTE_IMAGES_ALLOW_DIRECT  true/false: users may load from the sender
//                               (unset: only without a proxy)
//   REMOTE_IMAGES_FORCE_NEVER   true: never, for everyone (locked)
// varlock checks each value; this checks that they make sense together,
// when the server starts (server.ts): a contradiction stops it instead of
// being quietly ignored.

import type {
  AdminImagePolicy,
  ImageLoadMode,
  ImageRoute,
} from "../../mail/image-policy";

export interface ImagePolicyEnv {
  readonly REMOTE_IMAGES_DEFAULT?: ImageLoadMode | undefined;
  readonly REMOTE_IMAGES_VIA_DEFAULT?: ImageRoute | undefined;
  readonly REMOTE_IMAGES_ALLOW_DIRECT?: boolean | undefined;
  readonly REMOTE_IMAGES_FORCE_NEVER?: boolean | undefined;
}

export class ImagePolicyConfigError extends Error {
  override readonly name = "ImagePolicyConfigError";
}

export const imagePolicyFromEnv = (
  env: ImagePolicyEnv,
  /** IMAGE_PROXY_ORIGIN and IMAGE_PROXY_KEYS are set. */
  proxy: boolean
): AdminImagePolicy => {
  const via = env.REMOTE_IMAGES_VIA_DEFAULT;
  // Unset: direct only without a proxy (the only way to see images then)
  // or when the admin made it the default. Straight from the sender, the
  // browser can be sent to the user's own network by a public name that
  // resolves to a private address (`lvh.me`); it can't check where a name
  // points, the proxy can.
  const direct = env.REMOTE_IMAGES_ALLOW_DIRECT ?? (!proxy || via === "direct");
  const forceNever = env.REMOTE_IMAGES_FORCE_NEVER ?? false;
  const load = env.REMOTE_IMAGES_DEFAULT;
  if (via === "direct" && !direct) {
    throw new ImagePolicyConfigError(
      "REMOTE_IMAGES_VIA_DEFAULT=direct, but REMOTE_IMAGES_ALLOW_DIRECT=false forbids it"
    );
  }
  if (via === "proxy" && !proxy) {
    throw new ImagePolicyConfigError(
      "REMOTE_IMAGES_VIA_DEFAULT=proxy needs an image proxy (IMAGE_PROXY_ORIGIN and IMAGE_PROXY_KEYS)"
    );
  }
  if (forceNever && load !== undefined && load !== "never") {
    throw new ImagePolicyConfigError(
      `REMOTE_IMAGES_FORCE_NEVER=true, but REMOTE_IMAGES_DEFAULT=${load}`
    );
  }
  return {
    defaultLoad: forceNever ? "never" : (load ?? "ask"),
    defaultRoute: via ?? null,
    direct,
    forceNever,
    proxy,
  };
};
