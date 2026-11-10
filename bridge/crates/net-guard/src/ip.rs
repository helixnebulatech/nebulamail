// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! IP address classification.
//!
//! An allow-list: an address is public only if it is in no table below and,
//! for IPv6, inside `2000::/3` (the global unicast space). The tables are the
//! union of what `packages/net-guard` blocks (its own tables plus the
//! `ipaddr.js` special ranges), so both sides give the same answer.
//! Addresses that embed an IPv4 (mapped, NAT64, 6to4) are judged by it.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Which networks a destination may be in.
///
/// [`NetworkAccess::PublicOnly`] is the default and the only choice for
/// anything that does not come from the admin's configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum NetworkAccess {
    /// Only global unicast addresses.
    #[default]
    PublicOnly,
    /// Also the local networks where an admin may run a mail server
    /// (`private_network = true`): RFC 1918, carrier-grade
    /// NAT (Tailscale and similar), loopback and IPv6 unique-local.
    ///
    /// Still blocked: unspecified, link-local (cloud metadata lives at
    /// `169.254.169.254`; IPv6 link-local needs a zone id we do not accept),
    /// multicast, broadcast, documentation, benchmarking, reserved, every
    /// IPv6 form that embeds an IPv4 (a LAN server is reached by its plain
    /// address, never through mapped/NAT64/6to4 forms), and the cloud
    /// metadata services that live inside the opened ranges: `100.100.100.200`
    /// (Alibaba Cloud, in carrier-grade NAT), `fd00:ec2::/32` (AWS: IMDS at
    /// `::254`, DNS and time sync next to it) and `fd20:ce::254` (Google
    /// Cloud), both unique-local.
    PrivateAllowed,
}

/// A named special-purpose range. [`IpRange::as_str`] gives the same name
/// as the TypeScript `checkAddress` (for logs; never shown to users).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum IpRange {
    /// `0.0.0.0/8`.
    ThisNetwork,
    /// RFC 1918: `10/8`, `172.16/12`, `192.168/16`.
    Private,
    /// `100.64.0.0/10` (RFC 6598).
    CarrierGradeNat,
    /// `127/8`, `::1`.
    Loopback,
    /// `169.254/16`, `fe80::/10`.
    LinkLocal,
    /// `192.0.0.0/24`, `2001::/23`.
    IetfProtocol,
    /// `192.0.2/24`, `198.51.100/24`, `203.0.113/24`, `2001:db8::/32`, `3fff::/20`.
    Documentation,
    /// `192.88.99.0/24` (old 6to4 anycast).
    SixToFourRelay,
    /// `198.18.0.0/15`.
    Benchmarking,
    /// AS112 DNS sinks (RFC 7534/7535).
    As112,
    /// AMT relays (RFC 7450).
    Amt,
    /// `224/4`, `ff00::/8`.
    Multicast,
    /// `240/4`, which includes `255.255.255.255` (broadcast).
    Reserved,
    /// `::`.
    Unspecified,
    /// `::/96`, deprecated IPv4-compatible addresses.
    Ipv4Compatible,
    /// `::ffff:0:0:0/96` (RFC 6145).
    Ipv4Translated,
    /// `64:ff9b:1::/48`, local-use NAT64 (where the IPv4 sits is unknown).
    Nat64Local,
    /// `100::/64` (RFC 6666).
    Discard,
    /// `fc00::/7`.
    UniqueLocal,
    /// `fec0::/10` (deprecated).
    SiteLocal,
    /// `2001::/32`: hides the real IPv4.
    Teredo,
    /// Any IPv6 outside `2000::/3`.
    NotGlobalUnicast,
}

impl IpRange {
    /// The range name, as in the TypeScript `checkAddress`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ThisNetwork => "this-network",
            Self::Private => "private",
            Self::CarrierGradeNat => "carrier-grade-nat",
            Self::Loopback => "loopback",
            Self::LinkLocal => "link-local",
            Self::IetfProtocol => "ietf-protocol",
            Self::Documentation => "documentation",
            Self::SixToFourRelay => "6to4-relay",
            Self::Benchmarking => "benchmarking",
            Self::As112 => "as112",
            Self::Amt => "amt",
            Self::Multicast => "multicast",
            Self::Reserved => "reserved",
            Self::Unspecified => "unspecified",
            Self::Ipv4Compatible => "ipv4-compatible",
            Self::Ipv4Translated => "ipv4-translated",
            Self::Nat64Local => "nat64-local",
            Self::Discard => "discard",
            Self::UniqueLocal => "unique-local",
            Self::SiteLocal => "site-local",
            Self::Teredo => "teredo",
            Self::NotGlobalUnicast => "not-global-unicast",
        }
    }

    /// Ranges an admin may opt into with [`NetworkAccess::PrivateAllowed`].
    const fn is_local_network(self) -> bool {
        matches!(
            self,
            Self::Private | Self::CarrierGradeNat | Self::Loopback | Self::UniqueLocal
        )
    }
}

impl fmt::Display for IpRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An IPv6 form that carries an IPv4 inside.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Embedding {
    /// `::ffff:a.b.c.d`.
    Ipv4Mapped,
    /// `64:ff9b::a.b.c.d` (well-known NAT64 prefix).
    Nat64,
    /// `2002:aabb:ccdd::/48`.
    SixToFour,
}

impl Embedding {
    /// The prefix used in range names, as in TypeScript (`ipv4-mapped:private`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ipv4Mapped => "ipv4-mapped",
            Self::Nat64 => "nat64",
            Self::SixToFour => "6to4",
        }
    }
}

/// Why an address was refused: the range, and the IPv6 form if the range
/// is that of an embedded IPv4.
///
/// `Display` gives the TypeScript name, e.g. `loopback` or `nat64:private`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockedIp {
    range: IpRange,
    embedding: Option<Embedding>,
}

impl BlockedIp {
    /// The special-purpose range the address (or its embedded IPv4) is in.
    #[must_use]
    pub const fn range(self) -> IpRange {
        self.range
    }

    /// The IPv6 form, if the range applies to an embedded IPv4.
    #[must_use]
    pub const fn embedding(self) -> Option<Embedding> {
        self.embedding
    }

    const fn plain(range: IpRange) -> Self {
        Self {
            range,
            embedding: None,
        }
    }
}

impl fmt::Display for BlockedIp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.embedding {
            Some(embedding) => write!(f, "{}:{}", embedding.as_str(), self.range),
            None => fmt::Display::fmt(&self.range, f),
        }
    }
}

/// An IP address that passed [`check_ip`]. Only [`check_ip`] creates one,
/// so holding it proves the check happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AllowedIp(IpAddr);

impl AllowedIp {
    /// The address.
    #[must_use]
    pub const fn get(self) -> IpAddr {
        self.0
    }
}

impl fmt::Display for AllowedIp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// Decide whether `ip` is an allowed destination under `access`.
///
/// ```
/// use std::net::IpAddr;
/// use net_guard::{check_ip, IpRange, NetworkAccess};
///
/// let public: IpAddr = "93.184.215.14".parse().unwrap();
/// assert!(check_ip(public, NetworkAccess::PublicOnly).is_ok());
///
/// let lan: IpAddr = "192.168.1.10".parse().unwrap();
/// let blocked = check_ip(lan, NetworkAccess::PublicOnly).unwrap_err();
/// assert_eq!(blocked.range(), IpRange::Private);
/// assert!(check_ip(lan, NetworkAccess::PrivateAllowed).is_ok());
///
/// // An embedded IPv4 is judged by the IPv4, and never opens the LAN.
/// let mapped: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
/// let blocked = check_ip(mapped, NetworkAccess::PrivateAllowed).unwrap_err();
/// assert_eq!(blocked.to_string(), "ipv4-mapped:loopback");
///
/// // Never allowed, whatever the policy.
/// let metadata: IpAddr = "169.254.169.254".parse().unwrap();
/// assert!(check_ip(metadata, NetworkAccess::PrivateAllowed).is_err());
/// ```
///
/// # Errors
///
/// [`BlockedIp`] with the range the address is in.
pub fn check_ip(ip: IpAddr, access: NetworkAccess) -> Result<AllowedIp, BlockedIp> {
    let verdict = match ip {
        IpAddr::V4(v4) => check_v4(v4).map_err(BlockedIp::plain),
        IpAddr::V6(v6) => check_v6(v6),
    };
    match verdict {
        Ok(()) => Ok(AllowedIp(ip)),
        Err(blocked) if opens_local_network(blocked, access, ip) => Ok(AllowedIp(ip)),
        Err(blocked) => Err(blocked),
    }
}

fn opens_local_network(blocked: BlockedIp, access: NetworkAccess, ip: IpAddr) -> bool {
    match access {
        NetworkAccess::PublicOnly => false,
        NetworkAccess::PrivateAllowed => {
            blocked.embedding.is_none()
                && blocked.range.is_local_network()
                && !is_cloud_metadata(ip)
        }
    }
}

/// Cloud metadata services inside the ranges [`NetworkAccess::PrivateAllowed`]
/// opens (the link-local ones, like `169.254.169.254`, are never opened).
/// They keep the range they are in ([`IpRange::CarrierGradeNat`] or
/// [`IpRange::UniqueLocal`]) as the reason.
const CLOUD_METADATA_V4: [Ipv4Addr; 1] = [
    // Alibaba Cloud ECS metadata.
    Ipv4Addr::new(100, 100, 100, 200),
];

/// IPv6 cloud metadata, all inside `fc00::/7`.
const CLOUD_METADATA_V6: [Cidr<Ipv6Addr>; 2] = [
    // AWS Nitro: IMDS (`::254`), Route 53 resolver (`::253`), time sync
    // (`::123`) and the rest of the block.
    v6([0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0], 32),
    // Google Cloud metadata server for IPv6-only VMs.
    v6([0xfd20, 0x0ce, 0, 0, 0, 0, 0, 0x0254], 128),
];

fn is_cloud_metadata(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => CLOUD_METADATA_V4.contains(&v4),
        IpAddr::V6(v6) => CLOUD_METADATA_V6.iter().any(|cidr| cidr.contains(v6)),
    }
}

/// A CIDR block: the addresses whose first `prefix` bits match `network`.
struct Cidr<A> {
    network: A,
    prefix: u32,
}

const fn v4(a: u8, b: u8, c: u8, d: u8, prefix: u32) -> Cidr<Ipv4Addr> {
    Cidr {
        network: Ipv4Addr::new(a, b, c, d),
        prefix,
    }
}

const fn v6(groups: [u16; 8], prefix: u32) -> Cidr<Ipv6Addr> {
    let [s0, s1, s2, s3, s4, s5, s6, s7] = groups;
    Cidr {
        network: Ipv6Addr::new(s0, s1, s2, s3, s4, s5, s6, s7),
        prefix,
    }
}

impl Cidr<Ipv4Addr> {
    fn contains(&self, ip: Ipv4Addr) -> bool {
        let mask = u32::MAX
            .checked_shl(32_u32.saturating_sub(self.prefix))
            .unwrap_or(0);
        u32::from(ip) & mask == u32::from(self.network) & mask
    }
}

impl Cidr<Ipv6Addr> {
    fn contains(&self, ip: Ipv6Addr) -> bool {
        let mask = u128::MAX
            .checked_shl(128_u32.saturating_sub(self.prefix))
            .unwrap_or(0);
        u128::from(ip) & mask == u128::from(self.network) & mask
    }
}

/// Blocked IPv4 ranges (special-purpose ranges plus the `ipaddr.js` extras).
const BLOCKED_V4: &[(Cidr<Ipv4Addr>, IpRange)] = &[
    (v4(0, 0, 0, 0, 8), IpRange::ThisNetwork),
    (v4(10, 0, 0, 0, 8), IpRange::Private),
    (v4(100, 64, 0, 0, 10), IpRange::CarrierGradeNat),
    (v4(127, 0, 0, 0, 8), IpRange::Loopback),
    (v4(169, 254, 0, 0, 16), IpRange::LinkLocal),
    (v4(172, 16, 0, 0, 12), IpRange::Private),
    (v4(192, 0, 0, 0, 24), IpRange::IetfProtocol),
    (v4(192, 0, 2, 0, 24), IpRange::Documentation),
    (v4(192, 31, 196, 0, 24), IpRange::As112),
    (v4(192, 52, 193, 0, 24), IpRange::Amt),
    (v4(192, 88, 99, 0, 24), IpRange::SixToFourRelay),
    (v4(192, 168, 0, 0, 16), IpRange::Private),
    (v4(192, 175, 48, 0, 24), IpRange::As112),
    (v4(198, 18, 0, 0, 15), IpRange::Benchmarking),
    (v4(198, 51, 100, 0, 24), IpRange::Documentation),
    (v4(203, 0, 113, 0, 24), IpRange::Documentation),
    (v4(224, 0, 0, 0, 4), IpRange::Multicast),
    // Also holds 255.255.255.255 (broadcast).
    (v4(240, 0, 0, 0, 4), IpRange::Reserved),
];

/// Blocked IPv6 ranges, checked in order after the embedded-IPv4 forms.
/// Those outside `2000::/3` are listed only to give a precise name.
const BLOCKED_V6: &[(Cidr<Ipv6Addr>, IpRange)] = &[
    (v6([0; 8], 128), IpRange::Unspecified),
    (v6([0, 0, 0, 0, 0, 0, 0, 1], 128), IpRange::Loopback),
    // `::/96` also holds the IPv4-compatible `::a.b.c.d`: deprecated and
    // never a valid destination, so the whole /96 goes instead of extracting.
    (v6([0; 8], 96), IpRange::Ipv4Compatible),
    (
        v6([0, 0, 0, 0, 0xffff, 0, 0, 0], 96),
        IpRange::Ipv4Translated,
    ),
    // Where the IPv4 sits depends on the operator's prefix length: block.
    (
        v6([0x64, 0xff9b, 1, 0, 0, 0, 0, 0], 48),
        IpRange::Nat64Local,
    ),
    (v6([0x100, 0, 0, 0, 0, 0, 0, 0], 64), IpRange::Discard),
    (v6([0xfc00, 0, 0, 0, 0, 0, 0, 0], 7), IpRange::UniqueLocal),
    (v6([0xfe80, 0, 0, 0, 0, 0, 0, 0], 10), IpRange::LinkLocal),
    (v6([0xfec0, 0, 0, 0, 0, 0, 0, 0], 10), IpRange::SiteLocal),
    (v6([0xff00, 0, 0, 0, 0, 0, 0, 0], 8), IpRange::Multicast),
    // Teredo hides the client IPv4 and the relay is arbitrary.
    (v6([0x2001, 0, 0, 0, 0, 0, 0, 0], 32), IpRange::Teredo),
    // IETF assignments: benchmarking, AMT, AS112, ORCHID, drone tags...
    (v6([0x2001, 0, 0, 0, 0, 0, 0, 0], 23), IpRange::IetfProtocol),
    (
        v6([0x2001, 0xdb8, 0, 0, 0, 0, 0, 0], 32),
        IpRange::Documentation,
    ),
    (
        v6([0x2620, 0x4f, 0x8000, 0, 0, 0, 0, 0], 48),
        IpRange::As112,
    ),
    (
        v6([0x3fff, 0, 0, 0, 0, 0, 0, 0], 20),
        IpRange::Documentation,
    ),
];

const GLOBAL_UNICAST_V6: Cidr<Ipv6Addr> = v6([0x2000, 0, 0, 0, 0, 0, 0, 0], 3);
const IPV4_MAPPED: Cidr<Ipv6Addr> = v6([0, 0, 0, 0, 0, 0xffff, 0, 0], 96);
const NAT64_WELL_KNOWN: Cidr<Ipv6Addr> = v6([0x64, 0xff9b, 0, 0, 0, 0, 0, 0], 96);
const SIX_TO_FOUR: Cidr<Ipv6Addr> = v6([0x2002, 0, 0, 0, 0, 0, 0, 0], 16);

fn check_v4(ip: Ipv4Addr) -> Result<(), IpRange> {
    match BLOCKED_V4.iter().find(|(cidr, _)| cidr.contains(ip)) {
        Some(&(_, range)) => Err(range),
        None => Ok(()),
    }
}

fn check_v6(ip: Ipv6Addr) -> Result<(), BlockedIp> {
    if let Some((inner, embedding)) = embedded_v4(ip) {
        return check_v4(inner).map_err(|range| BlockedIp {
            range,
            embedding: Some(embedding),
        });
    }
    if let Some(&(_, range)) = BLOCKED_V6.iter().find(|(cidr, _)| cidr.contains(ip)) {
        return Err(BlockedIp::plain(range));
    }
    if !GLOBAL_UNICAST_V6.contains(ip) {
        return Err(BlockedIp::plain(IpRange::NotGlobalUnicast));
    }
    Ok(())
}

/// The IPv4 inside a mapped, NAT64 (well-known prefix) or 6to4 address.
fn embedded_v4(ip: Ipv6Addr) -> Option<(Ipv4Addr, Embedding)> {
    let bits = u128::from(ip);
    if IPV4_MAPPED.contains(ip) {
        Some((v4_at(bits, 0), Embedding::Ipv4Mapped))
    } else if NAT64_WELL_KNOWN.contains(ip) {
        Some((v4_at(bits, 0), Embedding::Nat64))
    } else if SIX_TO_FOUR.contains(ip) {
        // 2002:AABB:CCDD::/48 carries the IPv4 in bits 16..48.
        Some((v4_at(bits, 80), Embedding::SixToFour))
    } else {
        None
    }
}

/// The 32 bits starting `shift` bits from the right, as an IPv4.
fn v4_at(bits: u128, shift: u32) -> Ipv4Addr {
    let word = bits.checked_shr(shift).unwrap_or(0) & u128::from(u32::MAX);
    // The mask keeps 32 bits, so this cannot fail; if it did, 0.0.0.0 is
    // blocked, so the fallback is safe.
    u32::try_from(word).map_or(Ipv4Addr::UNSPECIFIED, Ipv4Addr::from)
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use proptest::prelude::*;

    use super::{BLOCKED_V4, BlockedIp, IpRange, NetworkAccess, check_ip};

    const PUBLIC: NetworkAccess = NetworkAccess::PublicOnly;
    const PRIVATE: NetworkAccess = NetworkAccess::PrivateAllowed;

    fn verdict(text: &str, access: NetworkAccess) -> Result<(), String> {
        let ip: IpAddr = text.parse().unwrap();
        check_ip(ip, access)
            .map(|allowed| assert_eq!(allowed.get(), ip))
            .map_err(|blocked| blocked.to_string())
    }

    #[test]
    fn public_addresses_pass() {
        for text in [
            "93.184.215.14",
            "1.1.1.1",
            "8.8.8.8",
            "2606:2800:21f:cb07:6820:80da:af6b:8b2c",
            "2a00:1450:4001:81c::200e",
            "::ffff:93.184.215.14",
            "64:ff9b::5db8:d70e",
            "2002:5db8:d70e::1",
        ] {
            assert_eq!(verdict(text, PUBLIC), Ok(()), "{text}");
        }
    }

    #[test]
    fn range_names_match_typescript() {
        let cases = [
            ("0.0.0.0", "this-network"),
            ("10.1.2.3", "private"),
            ("100.64.0.1", "carrier-grade-nat"),
            ("127.0.0.1", "loopback"),
            ("169.254.169.254", "link-local"),
            ("172.31.255.255", "private"),
            ("192.0.0.8", "ietf-protocol"),
            ("192.0.2.1", "documentation"),
            ("192.31.196.1", "as112"),
            ("192.52.193.1", "amt"),
            ("192.88.99.1", "6to4-relay"),
            ("192.168.0.1", "private"),
            ("192.175.48.1", "as112"),
            ("198.19.255.255", "benchmarking"),
            ("198.51.100.1", "documentation"),
            ("203.0.113.1", "documentation"),
            ("224.0.0.1", "multicast"),
            ("255.255.255.255", "reserved"),
            ("::", "unspecified"),
            ("::1", "loopback"),
            ("::127.0.0.1", "ipv4-compatible"),
            ("::8.8.8.8", "ipv4-compatible"),
            ("::ffff:0:7f00:1", "ipv4-translated"),
            ("::ffff:10.0.0.1", "ipv4-mapped:private"),
            ("64:ff9b::7f00:1", "nat64:loopback"),
            ("64:ff9b:1::a00:1", "nat64-local"),
            ("2002:7f00:1::", "6to4:loopback"),
            ("2002:a9fe:a9fe::1", "6to4:link-local"),
            ("100::1", "discard"),
            ("fd00::1", "unique-local"),
            ("fe80::1", "link-local"),
            ("fec0::1", "site-local"),
            ("ff02::1", "multicast"),
            ("2001:0:4136:e378:8000:63bf:3fff:fdd2", "teredo"),
            ("2001:2::1", "ietf-protocol"),
            ("2001:db8::1", "documentation"),
            ("2620:4f:8000::1", "as112"),
            ("3fff::1", "documentation"),
            ("5f00::1", "not-global-unicast"),
            ("4000::1", "not-global-unicast"),
        ];
        for (text, name) in cases {
            assert_eq!(verdict(text, PUBLIC), Err(name.to_owned()), "{text}");
        }
    }

    #[test]
    fn private_network_opens_only_local_unicast() {
        for text in [
            "10.0.0.5",
            "172.16.0.1",
            "192.168.1.1",
            "100.100.1.1",
            "127.0.0.1",
            "127.1.2.3",
            "::1",
            "fd12:3456::1",
        ] {
            assert!(verdict(text, PUBLIC).is_err(), "{text}");
            assert_eq!(verdict(text, PRIVATE), Ok(()), "{text}");
        }
        for text in [
            "0.0.0.0",
            "::",
            "169.254.169.254",
            "fe80::1",
            "fec0::1",
            "224.0.0.1",
            "ff02::1",
            "255.255.255.255",
            "192.0.2.1",
            "2001:db8::1",
            "198.18.0.1",
            "240.0.0.1",
            "::127.0.0.1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "64:ff9b::a00:1",
            "2002:a00:1::",
            "64:ff9b:1::a00:1",
        ] {
            assert!(verdict(text, PRIVATE).is_err(), "{text}");
        }
    }

    #[test]
    fn private_network_keeps_cloud_metadata_blocked() {
        for (text, name) in [
            ("100.100.100.200", "carrier-grade-nat"),
            ("fd00:ec2::254", "unique-local"),
            ("fd00:ec2::253", "unique-local"),
            ("fd00:ec2::123", "unique-local"),
            ("fd00:ec2:ffff:ffff:ffff:ffff:ffff:ffff", "unique-local"),
            ("fd20:ce::254", "unique-local"),
        ] {
            assert_eq!(verdict(text, PRIVATE), Err(name.to_owned()), "{text}");
            assert_eq!(verdict(text, PUBLIC), Err(name.to_owned()), "{text}");
        }
        // Their neighbours are ordinary local addresses.
        for text in [
            "100.100.100.201",
            "100.64.0.1",
            "fd00:ec3::254",
            "fd20:ce::253",
        ] {
            assert_eq!(verdict(text, PRIVATE), Ok(()), "{text}");
            assert!(verdict(text, PUBLIC).is_err(), "{text}");
        }
    }

    /// Any address inside a blocked IPv4 range: a table entry plus random host bits.
    fn blocked_v4() -> impl Strategy<Value = Ipv4Addr> {
        (0..BLOCKED_V4.len(), any::<u32>()).prop_map(|(index, noise)| {
            let (cidr, _) = &BLOCKED_V4[index];
            let host_mask = u32::MAX.checked_shr(cidr.prefix).unwrap_or(0);
            Ipv4Addr::from(u32::from(cidr.network) | (noise & host_mask))
        })
    }

    fn mapped(ip: Ipv4Addr) -> IpAddr {
        IpAddr::V6(ip.to_ipv6_mapped())
    }

    fn nat64(ip: Ipv4Addr) -> IpAddr {
        let bits = (0x64_u128 << 112) | (0xff9b_u128 << 96) | u128::from(u32::from(ip));
        IpAddr::V6(Ipv6Addr::from(bits))
    }

    fn six_to_four(ip: Ipv4Addr, low: u128) -> IpAddr {
        let low = low & ((1_u128 << 80) - 1);
        let bits = (0x2002_u128 << 112) | (u128::from(u32::from(ip)) << 80) | low;
        IpAddr::V6(Ipv6Addr::from(bits))
    }

    proptest! {
        #[test]
        fn blocked_v4_ranges_are_rejected(ip in blocked_v4()) {
            prop_assert!(check_ip(IpAddr::V4(ip), PUBLIC).is_err());
        }

        #[test]
        fn embedded_forms_agree_with_plain_v4(ip in any::<Ipv4Addr>(), low in any::<u128>()) {
            let plain = check_ip(IpAddr::V4(ip), PUBLIC).map(|_| ()).map_err(BlockedIp::range);
            for embedded in [mapped(ip), nat64(ip), six_to_four(ip, low)] {
                let result = check_ip(embedded, PUBLIC).map(|_| ()).map_err(BlockedIp::range);
                prop_assert_eq!(result, plain);
            }
        }

        #[test]
        fn embedded_forms_never_open_the_lan(ip in any::<Ipv4Addr>(), low in any::<u128>()) {
            for embedded in [mapped(ip), nat64(ip), six_to_four(ip, low)] {
                let public = check_ip(embedded, PUBLIC).is_ok();
                prop_assert_eq!(check_ip(embedded, PRIVATE).is_ok(), public);
            }
        }

        #[test]
        fn v6_outside_global_unicast_is_rejected(bits in any::<u128>()) {
            let ip = Ipv6Addr::from(bits);
            let first = ip.segments()[0];
            let global = (0x2000..0x4000).contains(&first);
            let embedded = first == 0x0064 && ip.segments()[1] == 0xff9b
                || ip.to_ipv4_mapped().is_some();
            if !global && !embedded {
                prop_assert!(check_ip(IpAddr::V6(ip), PUBLIC).is_err());
            }
        }

        #[test]
        fn private_access_only_adds(bits in any::<u128>(), v4 in any::<Ipv4Addr>()) {
            for ip in [IpAddr::V6(Ipv6Addr::from(bits)), IpAddr::V4(v4)] {
                if check_ip(ip, PUBLIC).is_ok() {
                    prop_assert!(check_ip(ip, PRIVATE).is_ok());
                }
                if let Err(blocked) = check_ip(ip, PRIVATE) {
                    prop_assert!(!matches!(
                        blocked.range(),
                        IpRange::Private | IpRange::Loopback
                            if blocked.embedding().is_none()
                    ));
                }
            }
        }
    }
}
