// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The shared block reasons.

use std::fmt;

/// Why a URL, connection or response was rejected.
///
/// Same contract as `BlockReason` in `packages/net-guard/src/types.ts` and
/// `reason` in `security/ssrf-vectors.json`: change both sides together.
/// [`BlockReason::as_str`] gives the shared name.
///
/// ```
/// use net_guard::BlockReason;
///
/// assert_eq!(BlockReason::PrivateIp.as_str(), "private-ip");
/// assert_eq!(BlockReason::Network.to_string(), "network");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BlockReason {
    /// Not an allowed scheme (or not a URL at all).
    Scheme,
    /// The URL has a user name or password.
    Userinfo,
    /// The port is not in the policy.
    Port,
    /// An address (literal or resolved) is not allowed by the policy.
    PrivateIp,
    /// DNS failed, timed out or gave no address, or the name is never
    /// public (single label, `.internal`, `.local`, ...).
    Dns,
    /// Too many redirects (HTTP only).
    TooManyRedirects,
    /// The response is not of an allowed type (HTTP only).
    ContentType,
    /// The response is too big (HTTP only).
    Size,
    /// A time limit was reached.
    Timeout,
    /// Connection refused or reset, TLS failure, malformed response.
    /// Not a policy decision.
    Network,
}

impl BlockReason {
    /// The shared name, as in `ssrf-vectors.json`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Scheme => "scheme",
            Self::Userinfo => "userinfo",
            Self::Port => "port",
            Self::PrivateIp => "private-ip",
            Self::Dns => "dns",
            Self::TooManyRedirects => "too-many-redirects",
            Self::ContentType => "content-type",
            Self::Size => "size",
            Self::Timeout => "timeout",
            Self::Network => "network",
        }
    }
}

impl fmt::Display for BlockReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::BlockReason;

    #[test]
    fn names_match_the_typescript_contract() {
        let all = [
            (BlockReason::Scheme, "scheme"),
            (BlockReason::Userinfo, "userinfo"),
            (BlockReason::Port, "port"),
            (BlockReason::PrivateIp, "private-ip"),
            (BlockReason::Dns, "dns"),
            (BlockReason::TooManyRedirects, "too-many-redirects"),
            (BlockReason::ContentType, "content-type"),
            (BlockReason::Size, "size"),
            (BlockReason::Timeout, "timeout"),
            (BlockReason::Network, "network"),
        ];
        for (reason, name) in all {
            assert_eq!(reason.as_str(), name);
            assert_eq!(reason.to_string(), name);
        }
    }
}
