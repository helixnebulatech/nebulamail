// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! TCP connections pinned to a validated IP.

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::TcpStream;

use crate::{AllowedIp, BlockReason, Host, NetworkAccess, Port, PortPolicy, Resolver, resolve};

/// Longest wait for each TCP connection attempt.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// At most this many validated addresses are tried, so a long DNS answer
/// cannot stretch the connection time (`4 × CONNECT_TIMEOUT` at worst).
pub const MAX_ATTEMPTS: usize = 4;

/// What a destination may be: its ports and its networks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    ports: PortPolicy,
    network: NetworkAccess,
}

impl Policy {
    /// Any ports and networks.
    #[must_use]
    pub const fn new(ports: PortPolicy, network: NetworkAccess) -> Self {
        Self { ports, network }
    }

    /// A mail server from the configuration: [`PortPolicy::MAIL`], and the
    /// local networks only if the admin set `private_network = true`.
    #[must_use]
    pub const fn mail_server(network: NetworkAccess) -> Self {
        Self::new(PortPolicy::MAIL, network)
    }

    /// The allowed ports.
    #[must_use]
    pub const fn ports(&self) -> &PortPolicy {
        &self.ports
    }

    /// The allowed networks.
    #[must_use]
    pub const fn network(&self) -> NetworkAccess {
        self.network
    }
}

/// Where to connect: a host and a port.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Target {
    host: Host,
    port: Port,
}

impl Target {
    /// A target.
    #[must_use]
    pub const fn new(host: Host, port: Port) -> Self {
        Self { host, port }
    }

    /// The host as configured (use it for TLS, not for connecting).
    #[must_use]
    pub const fn host(&self) -> &Host {
        &self.host
    }

    /// The port.
    #[must_use]
    pub const fn port(&self) -> Port {
        self.port
    }
}

/// A connection was refused by the policy or could not be opened.
///
/// `Display` shows only the reason, never the host or address.
#[derive(Debug, thiserror::Error)]
#[error("net-guard: {reason}")]
pub struct GuardError {
    reason: BlockReason,
    #[source]
    source: Option<io::Error>,
}

impl GuardError {
    /// The shared reason (`network` when the connection itself failed).
    #[must_use]
    pub const fn reason(&self) -> BlockReason {
        self.reason
    }
}

impl From<BlockReason> for GuardError {
    fn from(reason: BlockReason) -> Self {
        Self {
            reason,
            source: None,
        }
    }
}

/// An open TCP connection to a validated address.
#[derive(Debug)]
pub struct GuardedStream {
    stream: TcpStream,
    peer: AllowedIp,
    host: Host,
}

impl GuardedStream {
    /// The address actually connected to.
    #[must_use]
    pub const fn peer(&self) -> AllowedIp {
        self.peer
    }

    /// The original host, for TLS: SNI and the certificate check use the
    /// name (see [`DomainName::tls_name`](crate::DomainName::tls_name)),
    /// never the IP we dialed.
    #[must_use]
    pub const fn host(&self) -> &Host {
        &self.host
    }

    /// The stream.
    #[must_use]
    pub fn into_stream(self) -> TcpStream {
        self.stream
    }

    /// The stream, the address connected to and the original host.
    #[must_use]
    pub fn into_parts(self) -> (TcpStream, AllowedIp, Host) {
        (self.stream, self.peer, self.host)
    }
}

/// Check the port, resolve the host ONCE, check every address, then open a
/// TCP connection to the first validated address that answers.
///
/// The OS never resolves the name, so DNS rebinding cannot change where we
/// connect after the check.
///
/// ```no_run
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// use net_guard::{connect, NetworkAccess, Policy, Port, SystemResolver, Target};
///
/// let resolver = SystemResolver::new()?;
/// let target = Target::new("imap.example.com".parse()?, Port::IMAPS);
/// let policy = Policy::mail_server(NetworkAccess::PublicOnly);
/// let stream = connect(&target, &policy, &resolver).await?;
/// // TLS with SNI = stream.host(), over stream.into_stream().
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// [`GuardError`] with `port`, `private-ip` or `dns` when the policy refuses
/// the target, `timeout` when the last attempt timed out, and `network` when
/// it failed otherwise.
pub async fn connect(
    target: &Target,
    policy: &Policy,
    resolver: &dyn Resolver,
) -> Result<GuardedStream, GuardError> {
    connect_with(target, policy, resolver, TcpStream::connect).await
}

/// [`connect`] with the dial step injected, so tests can see which address
/// is dialed and redirect it to a local listener.
pub(crate) async fn connect_with<D, F>(
    target: &Target,
    policy: &Policy,
    resolver: &dyn Resolver,
    dial: D,
) -> Result<GuardedStream, GuardError>
where
    D: Fn(SocketAddr) -> F,
    F: Future<Output = io::Result<TcpStream>>,
{
    let host = target.host();
    if let Err(reason) = policy.ports().check(target.port()) {
        tracing::warn!(%host, %reason, "net-guard: connection refused");
        return Err(reason.into());
    }
    let addresses = resolve(host, policy.network(), resolver)
        .await
        .inspect_err(|reason| tracing::warn!(%host, %reason, "net-guard: connection refused"))?;

    let mut last = GuardError::from(BlockReason::Network);
    for peer in addresses.iter().take(MAX_ATTEMPTS) {
        let address = SocketAddr::new(peer.get(), target.port().get());
        match tokio::time::timeout(CONNECT_TIMEOUT, dial(address)).await {
            Ok(Ok(stream)) => {
                return Ok(GuardedStream {
                    stream,
                    peer,
                    host: host.clone(),
                });
            }
            Ok(Err(error)) => {
                last = GuardError {
                    reason: BlockReason::Network,
                    source: Some(error),
                };
            }
            Err(_elapsed) => last = BlockReason::Timeout.into(),
        }
    }
    tracing::debug!(%host, reason = %last.reason, "net-guard: connection failed");
    Err(last)
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::io;
    use std::net::{Ipv4Addr, SocketAddr};
    use std::pin::Pin;
    use std::sync::Mutex;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    use super::{Policy, Target, connect, connect_with};
    use crate::resolve::testing::FakeDns;
    use crate::{BlockReason, NetworkAccess, Port, PortPolicy};

    async fn listener() -> (TcpListener, Port) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = Port::try_from(listener.local_addr().unwrap().port()).unwrap();
        (listener, port)
    }

    fn policy(port: Port, network: NetworkAccess) -> Policy {
        Policy::new(PortPolicy::only(vec![port]), network)
    }

    /// The bridge spawns connections on a multi-threaded runtime.
    #[test]
    fn connect_future_is_send() {
        fn assert_send<T: Send>(_: &T) {}
        let target = Target::new("imap.example.com".parse().unwrap(), Port::IMAPS);
        let policy = Policy::mail_server(NetworkAccess::PublicOnly);
        let dns = FakeDns::failing();
        assert_send(&connect(&target, &policy, &dns));
    }

    #[tokio::test]
    async fn loopback_needs_private_network() {
        let (listener, port) = listener().await;
        let target = Target::new("127.0.0.1".parse().unwrap(), port);
        let dns = FakeDns::failing();

        let refused = connect(&target, &policy(port, NetworkAccess::PublicOnly), &dns).await;
        assert_eq!(refused.unwrap_err().reason(), BlockReason::PrivateIp);

        let allowed = policy(port, NetworkAccess::PrivateAllowed);
        let guarded = connect(&target, &allowed, &dns).await.unwrap();
        assert_eq!(guarded.peer().to_string(), "127.0.0.1");
        assert_eq!(guarded.host(), &target.host().clone());

        // It is a real connection.
        let (mut server, _) = listener.accept().await.unwrap();
        let mut client = guarded.into_stream();
        client.write_all(b"ping").await.unwrap();
        let mut buffer = [0_u8; 4];
        server.read_exact(&mut buffer).await.unwrap();
        assert_eq!(&buffer, b"ping");
    }

    #[tokio::test]
    async fn names_resolving_to_loopback_need_private_network() {
        let (_listener, port) = listener().await;
        let target = Target::new("loopback.example.com".parse().unwrap(), port);

        let dns = FakeDns::answering(&[&["127.0.0.1"]]);
        let refused = connect(&target, &policy(port, NetworkAccess::PublicOnly), &dns).await;
        assert_eq!(refused.unwrap_err().reason(), BlockReason::PrivateIp);

        let dns = FakeDns::answering(&[&["127.0.0.1"]]);
        let allowed = policy(port, NetworkAccess::PrivateAllowed);
        let guarded = connect(&target, &allowed, &dns).await.unwrap();
        assert_eq!(guarded.host().to_string(), "loopback.example.com");

        // `localhost` itself: refused by name when public only, before DNS.
        let localhost = Target::new("localhost".parse().unwrap(), port);
        let dns = FakeDns::answering(&[&["127.0.0.1"]]);
        let refused = connect(&localhost, &policy(port, NetworkAccess::PublicOnly), &dns).await;
        assert_eq!(refused.unwrap_err().reason(), BlockReason::Dns);
        assert!(dns.calls().is_empty());
        let guarded = connect(&localhost, &allowed, &dns).await.unwrap();
        assert_eq!(guarded.host().to_string(), "localhost");
    }

    #[tokio::test]
    async fn port_outside_the_policy_is_refused_before_dns() {
        let target = Target::new(
            "imap.example.com".parse().unwrap(),
            Port::try_from(22).unwrap(),
        );
        let dns = FakeDns::answering(&[&["93.184.215.14"]]);
        let result = connect(
            &target,
            &Policy::mail_server(NetworkAccess::PublicOnly),
            &dns,
        )
        .await;
        assert_eq!(result.unwrap_err().reason(), BlockReason::Port);
        assert!(dns.calls().is_empty());
    }

    #[tokio::test]
    async fn closed_port_is_a_network_error() {
        let (listener, port) = listener().await;
        drop(listener);
        let target = Target::new("127.0.0.1".parse().unwrap(), port);
        let result = connect(
            &target,
            &policy(port, NetworkAccess::PrivateAllowed),
            &FakeDns::failing(),
        )
        .await;
        let error = result.unwrap_err();
        assert_eq!(error.reason(), BlockReason::Network);
        assert_eq!(error.to_string(), "net-guard: network");
    }

    /// Records the addresses dialed and sends them all to `local` instead.
    fn redirecting_dial(
        local: SocketAddr,
        dialed: &Mutex<Vec<SocketAddr>>,
    ) -> impl Fn(SocketAddr) -> Pin<Box<dyn Future<Output = io::Result<TcpStream>> + Send>> + '_
    {
        move |address| {
            dialed.lock().unwrap().push(address);
            Box::pin(TcpStream::connect(local))
        }
    }

    #[tokio::test]
    async fn tries_the_next_validated_address() {
        let (listener, port) = listener().await;
        let local = listener.local_addr().unwrap();
        let dns = FakeDns::answering(&[&["93.184.215.14", "1.1.1.1"]]);
        let dialed = Mutex::new(Vec::new());
        let dial = |address: SocketAddr| {
            let first = dialed.lock().unwrap().is_empty();
            dialed.lock().unwrap().push(address);
            Box::pin(async move {
                if first {
                    Err(io::Error::from(io::ErrorKind::ConnectionRefused))
                } else {
                    TcpStream::connect(local).await
                }
            })
        };
        let target = Target::new("two.example.com".parse().unwrap(), port);
        let guarded = connect_with(
            &target,
            &policy(port, NetworkAccess::PublicOnly),
            &dns,
            dial,
        )
        .await
        .unwrap();
        assert_eq!(guarded.peer().to_string(), "1.1.1.1");
        assert_eq!(dialed.lock().unwrap().len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn slow_connect_times_out() {
        let dns = FakeDns::answering(&[&["93.184.215.14"]]);
        let port = Port::IMAPS;
        let target = Target::new("slow.example.com".parse().unwrap(), port);
        let dial = |_address: SocketAddr| std::future::pending::<io::Result<TcpStream>>();
        let result = connect_with(
            &target,
            &Policy::mail_server(NetworkAccess::PublicOnly),
            &dns,
            dial,
        )
        .await;
        assert_eq!(result.unwrap_err().reason(), BlockReason::Timeout);
    }

    #[tokio::test]
    async fn mixed_public_and_private_dns_is_refused() {
        let target = Target::new("dual.example.com".parse().unwrap(), Port::IMAPS);
        let dns = FakeDns::answering(&[&["93.184.215.14", "::1"]]);
        let dialed = Mutex::new(Vec::new());
        let dial = redirecting_dial(SocketAddr::from((Ipv4Addr::LOCALHOST, 9)), &dialed);
        let result = connect_with(
            &target,
            &Policy::mail_server(NetworkAccess::PublicOnly),
            &dns,
            dial,
        )
        .await;
        assert_eq!(result.unwrap_err().reason(), BlockReason::PrivateIp);
        assert!(dialed.lock().unwrap().is_empty());
    }
}
