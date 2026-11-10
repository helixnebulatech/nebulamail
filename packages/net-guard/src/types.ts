// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Contract shared with the Rust crate `bridge/crates/net-guard` and with
// `security/ssrf-vectors.json`. Change both sides together.

/** Why a URL, connection or response was rejected. Matches `reason` in ssrf-vectors.json. */
export type BlockReason =
  | "scheme"
  | "userinfo"
  | "port"
  | "private-ip"
  /** DNS failed or gave nothing, or the name is never public (single label, `.internal`, ...). */
  | "dns"
  | "too-many-redirects"
  | "content-type"
  | "size"
  | "timeout"
  /** Connection refused/reset, TLS failure, malformed HTTP. Not a policy decision. */
  | "network";

/** Hard limits. Adapters may tighten them, never loosen. */
export const LIMITS = {
  allowedPorts: [80, 443],
  connectTimeoutMs: 5000,
  maxImageBytes: 5 * 1024 * 1024,
  maxRedirects: 5,
  totalTimeoutMs: 10_000,
} as const;
