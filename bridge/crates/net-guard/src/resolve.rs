// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! DNS resolution and validation.

use std::fmt;
use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::time::Duration;

use hickory_resolver::TokioResolver;
use hickory_resolver::config::LookupIpStrategy;

use crate::{AllowedIp, BlockReason, DomainName, Host, NetworkAccess, check_ip};

/// Longest wait for a DNS answer. A resolver that does not answer is a DNS
/// failure (`dns`), whatever its own retry settings are.
pub const DNS_TIMEOUT: Duration = Duration::from_secs(5);

/// The DNS lookup failed. Carries no detail: callers only need to know that
/// it failed, and the reason is always [`BlockReason::Dns`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("DNS lookup failed")]
pub struct DnsError;

/// The future returned by [`Resolver::lookup`].
pub type LookupFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<IpAddr>, DnsError>> + Send + 'a>>;

/// Resolves a name to ALL its addresses (A and AAAA).
///
/// Injected so tests can use a fake DNS. An empty answer (NXDOMAIN, no
/// records) may be `Ok(vec![])` or an error: both block with `dns`.
///
/// ```
/// use std::net::IpAddr;
/// use net_guard::{DomainName, DnsError, LookupFuture, Resolver};
///
/// /// Answers every name with the same addresses.
/// struct Fixed(Vec<IpAddr>);
///
/// impl Resolver for Fixed {
///     fn lookup<'a>(&'a self, _name: &'a DomainName) -> LookupFuture<'a> {
///         Box::pin(async move { Ok(self.0.clone()) })
///     }
/// }
/// ```
pub trait Resolver: Send + Sync {
    /// Look up the A and AAAA records of `name`.
    fn lookup<'a>(&'a self, name: &'a DomainName) -> LookupFuture<'a>;
}

/// The system resolver (`/etc/resolv.conf` and the hosts file), through
/// `hickory-resolver`. It only does DNS: proxy variables such as
/// `HTTP_PROXY` are never read.
#[derive(Clone)]
pub struct SystemResolver {
    inner: TokioResolver,
}

impl SystemResolver {
    /// Build it from the system configuration. Needs a Tokio runtime.
    ///
    /// # Errors
    ///
    /// [`DnsError`] if the system configuration cannot be read.
    pub fn new() -> Result<Self, DnsError> {
        let mut builder = TokioResolver::builder_tokio().map_err(|error| {
            tracing::warn!(%error, "net-guard: cannot read the system DNS configuration");
            DnsError
        })?;
        // Both families, always: every address must be checked.
        builder.options_mut().ip_strategy = LookupIpStrategy::Ipv4AndIpv6;
        let inner = builder.build().map_err(|error| {
            tracing::warn!(%error, "net-guard: cannot build the DNS resolver");
            DnsError
        })?;
        Ok(Self { inner })
    }
}

impl fmt::Debug for SystemResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SystemResolver").finish_non_exhaustive()
    }
}

impl Resolver for SystemResolver {
    fn lookup<'a>(&'a self, name: &'a DomainName) -> LookupFuture<'a> {
        Box::pin(async move {
            match self.inner.lookup_ip(name.as_str()).await {
                Ok(answer) => Ok(answer.iter().collect()),
                Err(error) => {
                    tracing::debug!(host = %name, %error, "net-guard: DNS lookup failed");
                    Err(DnsError)
                }
            }
        })
    }
}

/// The validated addresses of a host: never empty, without duplicates, in
/// the order the resolver gave them. Only [`resolve`] creates it. Connect to
/// these and nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Addresses {
    first: AllowedIp,
    rest: Vec<AllowedIp>,
}

impl Addresses {
    /// The first address.
    #[must_use]
    pub const fn first(&self) -> AllowedIp {
        self.first
    }

    /// All addresses, first one first.
    pub fn iter(&self) -> impl Iterator<Item = AllowedIp> + '_ {
        std::iter::once(self.first).chain(self.rest.iter().copied())
    }
}

/// Resolve `host` once and check every address under `access`.
///
/// If ANY address is refused the whole host is: a mixed answer
/// is an attack or a mistake, and we would not know which one is safe. IP
/// literals are checked without calling the resolver.
///
/// Needs a Tokio runtime with the time driver (any `#[tokio::main]` has it).
///
/// ```
/// # let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
/// # rt.block_on(async {
/// use std::net::IpAddr;
/// use net_guard::{resolve, BlockReason, DomainName, Host, LookupFuture, NetworkAccess, Resolver};
///
/// struct Fixed(Vec<IpAddr>);
/// impl Resolver for Fixed {
///     fn lookup<'a>(&'a self, _name: &'a DomainName) -> LookupFuture<'a> {
///         Box::pin(async move { Ok(self.0.clone()) })
///     }
/// }
///
/// let host: Host = "mixed.example.com".parse().unwrap();
/// let dns = Fixed(vec!["93.184.215.14".parse().unwrap(), "10.0.0.1".parse().unwrap()]);
/// let result = resolve(&host, NetworkAccess::PublicOnly, &dns).await;
/// assert_eq!(result.unwrap_err(), BlockReason::PrivateIp);
/// # });
/// ```
///
/// # Errors
///
/// [`BlockReason::PrivateIp`] if an address is refused, [`BlockReason::Dns`]
/// if the lookup fails, times out or gives nothing, or (public only) if the
/// name is never public ([`DomainName::is_non_public`]).
pub async fn resolve(
    host: &Host,
    access: NetworkAccess,
    resolver: &dyn Resolver,
) -> Result<Addresses, BlockReason> {
    let found = match host {
        Host::Ip(ip) => vec![*ip],
        // Single-label and special-use names are never public: refused
        // before asking any resolver.
        Host::Domain(name) if access == NetworkAccess::PublicOnly && name.is_non_public() => {
            tracing::debug!(%host, "net-guard: name is not public");
            return Err(BlockReason::Dns);
        }
        Host::Domain(name) => lookup(name, resolver).await?,
    };
    let mut allowed: Vec<AllowedIp> = Vec::with_capacity(found.len());
    for ip in found {
        let checked = check_ip(ip, access).map_err(|blocked| {
            tracing::debug!(%host, range = %blocked, "net-guard: address refused");
            BlockReason::PrivateIp
        })?;
        if !allowed.contains(&checked) {
            allowed.push(checked);
        }
    }
    let mut iter = allowed.into_iter();
    let Some(first) = iter.next() else {
        tracing::debug!(%host, "net-guard: DNS gave no address");
        return Err(BlockReason::Dns);
    };
    Ok(Addresses {
        first,
        rest: iter.collect(),
    })
}

async fn lookup(name: &DomainName, resolver: &dyn Resolver) -> Result<Vec<IpAddr>, BlockReason> {
    match tokio::time::timeout(DNS_TIMEOUT, resolver.lookup(name)).await {
        Ok(Ok(found)) => Ok(found),
        Ok(Err(DnsError)) => Err(BlockReason::Dns),
        Err(_elapsed) => {
            tracing::debug!(host = %name, "net-guard: DNS timed out");
            Err(BlockReason::Dns)
        }
    }
}

#[cfg(test)]
pub(crate) mod testing {
    //! A scripted fake DNS for tests.

    use std::collections::VecDeque;
    use std::net::IpAddr;
    use std::sync::Mutex;

    use super::{DnsError, LookupFuture, Resolver};
    use crate::DomainName;

    /// Gives the scripted answers in order, then fails. Records every call.
    #[derive(Debug, Default)]
    pub(crate) struct FakeDns {
        answers: Mutex<VecDeque<Result<Vec<IpAddr>, DnsError>>>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeDns {
        pub(crate) fn answering(answers: &[&[&str]]) -> Self {
            let answers = answers
                .iter()
                .map(|ips| Ok(ips.iter().map(|ip| ip.parse().unwrap()).collect()))
                .collect();
            Self {
                answers: Mutex::new(answers),
                calls: Mutex::default(),
            }
        }

        pub(crate) fn failing() -> Self {
            Self::default()
        }

        pub(crate) fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Resolver for FakeDns {
        fn lookup<'a>(&'a self, name: &'a DomainName) -> LookupFuture<'a> {
            self.calls.lock().unwrap().push(name.as_str().to_owned());
            let answer = self.answers.lock().unwrap().pop_front();
            Box::pin(async move { answer.unwrap_or(Err(DnsError)) })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::testing::FakeDns;
    use super::{DnsError, LookupFuture, Resolver, resolve};
    use crate::{BlockReason, DomainName, Host, NetworkAccess};

    const PUBLIC: NetworkAccess = NetworkAccess::PublicOnly;

    async fn run(
        host: &str,
        dns: &FakeDns,
        access: NetworkAccess,
    ) -> Result<Vec<String>, BlockReason> {
        let host: Host = host.parse().unwrap();
        resolve(&host, access, dns)
            .await
            .map(|found| found.iter().map(|ip| ip.to_string()).collect())
    }

    #[tokio::test]
    async fn keeps_order_and_removes_duplicates() {
        let dns = FakeDns::answering(&[&[
            "93.184.215.14",
            "1.1.1.1",
            "93.184.215.14",
            "2606:4700::1111",
        ]]);
        let found = run("a.example.com", &dns, PUBLIC).await.unwrap();
        assert_eq!(found, ["93.184.215.14", "1.1.1.1", "2606:4700::1111"]);
        assert_eq!(dns.calls(), ["a.example.com"]);
    }

    #[tokio::test]
    async fn one_bad_address_blocks_all() {
        let dns = FakeDns::answering(&[&["93.184.215.14", "::1"]]);
        assert_eq!(
            run("a.example.com", &dns, PUBLIC).await,
            Err(BlockReason::PrivateIp)
        );
    }

    #[tokio::test]
    async fn dns_failure_and_empty_answer_block() {
        assert_eq!(
            run("a.example.com", &FakeDns::failing(), PUBLIC).await,
            Err(BlockReason::Dns)
        );
        let empty = FakeDns::answering(&[&[]]);
        assert_eq!(
            run("a.example.com", &empty, PUBLIC).await,
            Err(BlockReason::Dns)
        );
    }

    #[tokio::test]
    async fn ip_literals_skip_the_resolver() {
        let dns = FakeDns::failing();
        assert_eq!(
            run("93.184.215.14", &dns, PUBLIC).await.unwrap(),
            ["93.184.215.14"]
        );
        assert_eq!(
            run("[::1]", &dns, PUBLIC).await,
            Err(BlockReason::PrivateIp)
        );
        assert_eq!(
            run("[::1]", &dns, NetworkAccess::PrivateAllowed)
                .await
                .unwrap(),
            ["::1"]
        );
        assert!(dns.calls().is_empty());
    }

    #[tokio::test]
    async fn private_network_policy_applies_to_dns_answers() {
        let dns = FakeDns::answering(&[&["192.168.1.5"], &["169.254.169.254"]]);
        let lan = NetworkAccess::PrivateAllowed;
        assert_eq!(run("nas.lan", &dns, lan).await.unwrap(), ["192.168.1.5"]);
        assert_eq!(run("nas.lan", &dns, lan).await, Err(BlockReason::PrivateIp));
    }

    #[tokio::test]
    async fn non_public_names_are_refused_before_resolving() {
        let dns = FakeDns::answering(&[&["93.184.215.14"]]);
        for name in [
            "intranet",
            "localhost.",
            "grafana.internal",
            "printer.local",
            "abc.onion",
            "nas.home.arpa",
        ] {
            assert_eq!(
                run(name, &dns, PUBLIC).await,
                Err(BlockReason::Dns),
                "{name}"
            );
        }
        assert!(dns.calls().is_empty());
    }

    #[tokio::test]
    async fn private_network_policy_keeps_local_names() {
        let dns = FakeDns::answering(&[&["192.168.1.5"], &["10.0.0.7"], &["127.0.0.1"]]);
        let lan = NetworkAccess::PrivateAllowed;
        assert_eq!(run("stalwart", &dns, lan).await.unwrap(), ["192.168.1.5"]);
        assert_eq!(run("mail.internal", &dns, lan).await.unwrap(), ["10.0.0.7"]);
        assert_eq!(run("localhost", &dns, lan).await.unwrap(), ["127.0.0.1"]);
        assert_eq!(dns.calls(), ["stalwart", "mail.internal", "localhost"]);
    }

    /// The real system resolver. Needs DNS, so it only runs with
    /// `NEBULA_TEST_DNS=1`.
    #[tokio::test]
    async fn system_resolver_checks_real_answers() {
        if std::env::var_os("NEBULA_TEST_DNS").is_none() {
            return;
        }
        let dns = super::SystemResolver::new().unwrap();
        let localhost: Host = "localhost".parse().unwrap();
        // Refused by name before resolving.
        assert_eq!(
            resolve(&localhost, PUBLIC, &dns).await,
            Err(BlockReason::Dns)
        );
        let found = resolve(&localhost, NetworkAccess::PrivateAllowed, &dns)
            .await
            .unwrap();
        assert!(found.iter().all(|ip| ip.get().is_loopback()));

        let public: Host = "example.com".parse().unwrap();
        assert!(resolve(&public, PUBLIC, &dns).await.is_ok());
        let missing: Host = "does-not-exist.invalid".parse().unwrap();
        assert_eq!(resolve(&missing, PUBLIC, &dns).await, Err(BlockReason::Dns));
    }

    /// Never answers.
    struct Silent;

    impl Resolver for Silent {
        fn lookup<'a>(&'a self, _name: &'a DomainName) -> LookupFuture<'a> {
            Box::pin(std::future::pending::<Result<Vec<IpAddr>, DnsError>>())
        }
    }

    #[tokio::test(start_paused = true)]
    async fn silent_dns_times_out_as_dns() {
        let host: Host = "slow.example.com".parse().unwrap();
        assert_eq!(resolve(&host, PUBLIC, &Silent).await, Err(BlockReason::Dns));
    }
}
