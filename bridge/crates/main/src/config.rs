// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The TOML configuration, parsed once at startup into typed values
//! ("parse, don't validate"). See `bridge.example.toml`.

use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, anyhow, bail, ensure};
use nebula_crypto::{ClientSecretHash, KeyVersion, Keyring, MasterKey};
use nebula_http::{
    AppOrigin, ClientId, ClientIpSource, HttpConfig, JmapLimits, Lifetimes, LoginLimits,
    OAuthClient, PublicUrl, RateLimit, RedirectUri, SignedOutSending,
};
use nebula_jmap::HoldSettings;
use nebula_jmap_proto::UnsignedInt;
use nebula_jmap_proto::method::{Collation, MAX_FILTER_LIST};
use nebula_jmap_proto::session::CoreCapability;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

/// Environment variable with the master key(s), instead of files.
pub(crate) const MASTER_KEY_ENV: &str = "NEBULA_BRIDGE_MASTER_KEY";

// ------------------------------------------------------------ raw TOML

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    server: RawServer,
    store: RawStore,
    #[serde(default)]
    secrets: RawSecrets,
    oauth: RawOAuth,
    upstream: RawUpstream,
    #[serde(default)]
    session: RawSession,
    #[serde(default)]
    limits: RawLimits,
    #[serde(default)]
    login_limits: RawLoginLimits,
    #[serde(default)]
    submission: RawSubmission,
    push: Option<RawPush>,
}

/// `[submission]`: messages the bridge holds (RFC 4865 FUTURERELEASE):
/// undo send and scheduled send.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct RawSubmission {
    hold_days: u64,
    max_held: u64,
    max_held_mb: u64,
    wait_hours: u64,
    send_while_signed_out: bool,
}

impl Default for RawSubmission {
    fn default() -> Self {
        let defaults = HoldSettings::new();
        Self {
            hold_days: defaults.max_delay.as_secs() / 86_400,
            max_held: defaults.max_count,
            max_held_mb: defaults.max_bytes / (1024 * 1024),
            wait_hours: defaults.max_wait.as_secs() / 3600,
            send_while_signed_out: defaults.send_while_signed_out,
        }
    }
}

/// `[push]`: Web Push from the bridge.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPush {
    contact: String,
    allowed_hosts: Option<Vec<String>>,
    #[serde(default)]
    loopback: bool,
    #[serde(default = "default_max_subscriptions")]
    max_subscriptions: usize,
    #[serde(default = "yes")]
    calendar_alerts: bool,
}

const fn default_max_subscriptions() -> usize {
    10
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawServer {
    listen: SocketAddr,
    public_url: String,
    #[serde(default)]
    app_origins: Vec<String>,
    /// Required with an https `public_url` (see [`client_ip`]).
    client_ip: Option<RawClientIp>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum RawClientIp {
    Peer,
    XForwardedFor,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStore {
    path: PathBuf,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSecrets {
    master_key_file: Option<PathBuf>,
    #[serde(default)]
    keys: Vec<RawKey>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawKey {
    version: u16,
    file: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOAuth {
    clients: Vec<RawClient>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawClient {
    id: String,
    secret_hash: String,
    redirect_uris: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawUpstream {
    imap: RawMailServer,
    smtp: RawMailServer,
    #[serde(default)]
    private_network: bool,
    #[serde(default)]
    sent_copy: SentCopy,
    #[serde(default)]
    email_domain: Option<String>,
    #[serde(default)]
    dav: Option<RawDav>,
    #[serde(default)]
    sieve: Option<RawSieve>,
}

/// `[upstream.sieve]`: ManageSieve. Every field is
/// optional: the IMAP host, port 4190, STARTTLS, and the IMAP server's
/// `tls_verify` when the host is the IMAP one.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSieve {
    host: Option<String>,
    port: Option<u16>,
    tls: Option<TlsMode>,
    tls_verify: Option<bool>,
    max_script_size: Option<u64>,
    max_scripts: Option<u64>,
    poll_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDav {
    url: String,
    #[serde(default)]
    contacts_url: Option<String>,
    #[serde(default = "yes")]
    calendars: bool,
    #[serde(default = "yes")]
    contacts: bool,
    #[serde(default = "yes")]
    tls_verify: bool,
    #[serde(default)]
    allowed_hosts: Vec<String>,
    #[serde(default)]
    scheduling: Scheduling,
    #[serde(default = "default_poll")]
    poll_seconds: u64,
    #[serde(default = "default_poll_idle")]
    poll_idle_seconds: u64,
    #[serde(default)]
    alerts: RawAlerts,
}

const fn default_poll() -> u64 {
    60
}

/// `[upstream.dav.alerts]`: calendar alerts (4.6). The
/// defaults are what the bridge did before the section existed.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct RawAlerts {
    enabled: bool,
    horizon_hours: u64,
    grace_minutes: u64,
    lead_days: u64,
    max_events: usize,
    max_per_event: usize,
    max_repeats: u32,
}

impl Default for RawAlerts {
    fn default() -> Self {
        Self {
            enabled: true,
            horizon_hours: 24,
            grace_minutes: 10,
            lead_days: 8,
            max_events: 2000,
            max_per_event: 64,
            max_repeats: 10,
        }
    }
}

const fn default_poll_idle() -> u64 {
    300
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMailServer {
    host: String,
    port: Option<u16>,
    #[serde(default)]
    tls: TlsMode,
    #[serde(default = "yes")]
    tls_verify: bool,
}

const fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct RawSession {
    access_token_minutes: u64,
    refresh_token_days: u64,
    max_days: u64,
}

impl Default for RawSession {
    fn default() -> Self {
        Self {
            access_token_minutes: 15,
            refresh_token_days: 30,
            max_days: 30,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct RawLimits {
    max_size_upload: u32,
    max_concurrent_upload: u32,
    max_size_request: u32,
    max_concurrent_requests: u32,
    max_calls_in_request: u32,
    max_objects_in_get: u32,
    max_objects_in_set: u32,
    event_streams_per_account: usize,
}

impl Default for RawLimits {
    fn default() -> Self {
        Self {
            max_size_upload: 50_000_000,
            max_concurrent_upload: 4,
            max_size_request: 10_000_000,
            max_concurrent_requests: 4,
            max_calls_in_request: 32,
            max_objects_in_get: 500,
            max_objects_in_set: 500,
            event_streams_per_account: 8,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct RawLoginLimits {
    per_login: u32,
    per_ip: u32,
    window_minutes: u64,
}

impl Default for RawLoginLimits {
    fn default() -> Self {
        Self {
            per_login: 5,
            per_ip: 20,
            window_minutes: 15,
        }
    }
}

// ------------------------------------------------------------ typed

/// How TLS starts on a mail connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum TlsMode {
    /// TLS from the first byte (993, 465). The default.
    #[default]
    Implicit,
    /// Plain connection upgraded with STARTTLS, which is required.
    Starttls,
}

/// Whether the server certificate is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CertificateCheck {
    /// Against the system's roots (`rustls-platform-verifier`).
    Verify,
    /// Not checked. Only accepted for loopback hosts (a development
    /// server with a self-signed certificate).
    InsecureLoopbackOnly,
}

/// A mail server from the configuration.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct MailServer {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) tls: TlsMode,
    pub(crate) certificate: CertificateCheck,
}

impl fmt::Debug for MailServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{} ({:?}, {:?})",
            self.host, self.port, self.tls, self.certificate
        )
    }
}

/// Whether the mail servers may be on private addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NetworkScope {
    /// Public addresses only (protects if DNS changes).
    Public,
    /// The admin allowed a server on the local network (`private_network`).
    Private,
}

/// Who saves sent mail in the Sent folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SentCopy {
    /// The bridge, with IMAP `APPEND`.
    #[default]
    Append,
    /// The server itself (Gmail): no `APPEND`, or it would be saved twice.
    Server,
}

/// Who sends invitations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Scheduling {
    /// The server if it announces `calendar-auto-schedule`, else the bridge.
    #[default]
    Auto,
    /// Always the bridge.
    Bridge,
    /// Always the server.
    Server,
}

/// The CalDAV/CardDAV server (`[upstream.dav]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DavUpstream {
    /// Where discovery starts: http(s), no user info, query or fragment.
    pub(crate) url: url::Url,
    /// Where address book discovery starts instead, if set (same rules;
    /// gets the same login as `url`).
    pub(crate) contacts_url: Option<url::Url>,
    pub(crate) calendars: bool,
    pub(crate) contacts: bool,
    pub(crate) certificate: CertificateCheck,
    /// Other origins (`https://host[:port]`) the server may send us to.
    pub(crate) allowed_hosts: Vec<url::Url>,
    pub(crate) scheduling: Scheduling,
    /// Sync interval with a push connection open.
    pub(crate) poll: Duration,
    /// Sync interval with a session but no push connection.
    pub(crate) poll_idle: Duration,
    /// Calendar alerts.
    pub(crate) alerts: AlertSettings,
}

/// The ManageSieve server (`[upstream.sieve]`), checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SieveUpstream {
    pub(crate) server: MailServer,
    /// Largest script in bytes.
    pub(crate) max_script_size: usize,
    /// Most scripts per account (`maxNumberScripts`; also the most a
    /// sync reads).
    pub(crate) max_scripts: usize,
    /// How often scripts are read while someone is listening.
    pub(crate) poll: Duration,
}

/// Largest `max_script_size` (a script is read whole into memory).
pub(crate) const MAX_SIEVE_SCRIPT: u64 = 16 * 1024 * 1024;

/// Default and largest `max_scripts`: every Sieve method call reads each
/// script once (GETSCRIPT).
const DEFAULT_SIEVE_SCRIPTS: u64 = 100;
const MAX_SIEVE_SCRIPTS: u64 = 1000;

fn sieve(raw: &RawSieve, imap: &RawMailServer) -> anyhow::Result<SieveUpstream> {
    let same_host = raw.host.is_none();
    let server = RawMailServer {
        host: raw.host.clone().unwrap_or_else(|| imap.host.clone()),
        port: raw.port,
        tls: raw.tls.unwrap_or(TlsMode::Starttls),
        tls_verify: raw
            .tls_verify
            .unwrap_or(if same_host { imap.tls_verify } else { true }),
    };
    let server = mail_server(&server, 4190, "sieve")?;
    let size = raw.max_script_size.unwrap_or(1024 * 1024);
    ensure!(
        (1024..=MAX_SIEVE_SCRIPT).contains(&size),
        "upstream.sieve.max_script_size must be 1024 to {MAX_SIEVE_SCRIPT} bytes"
    );
    let scripts = raw.max_scripts.unwrap_or(DEFAULT_SIEVE_SCRIPTS);
    ensure!(
        (1..=MAX_SIEVE_SCRIPTS).contains(&scripts),
        "upstream.sieve.max_scripts must be 1 to {MAX_SIEVE_SCRIPTS}"
    );
    // A login each time: at least 15 s. Up to an hour.
    let poll = raw.poll_seconds.unwrap_or(60);
    ensure!(
        (15..=3600).contains(&poll),
        "upstream.sieve.poll_seconds must be 15 to 3600"
    );
    Ok(SieveUpstream {
        server,
        max_script_size: usize::try_from(size).context("upstream.sieve.max_script_size")?,
        max_scripts: usize::try_from(scripts).context("upstream.sieve.max_scripts")?,
        poll: Duration::from_secs(poll),
    })
}

/// Calendar alerts (`[upstream.dav.alerts]`), checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AlertSettings {
    /// Alerts are sent at all (event source and Web Push).
    pub(crate) enabled: bool,
    /// How far ahead alerts are planned (2 h to 7 days).
    pub(crate) horizon: Duration,
    /// How late an alert may still be sent, and how long a push service
    /// keeps it (1 min to 1 day).
    pub(crate) grace: Duration,
    /// How long before an event its alerts may ring (1 to 366 days).
    pub(crate) lead: Duration,
    /// Events looked at per plan (1 to 100 000).
    pub(crate) max_events: usize,
    /// Alerts per event and plan (1 to 1000).
    pub(crate) max_per_event: usize,
    /// Repeats of one alarm, `REPEAT` of a `VALARM` (0 to 100).
    pub(crate) max_repeats: u32,
}

fn alerts(raw: &RawAlerts) -> anyhow::Result<AlertSettings> {
    let what = "upstream.dav.alerts";
    ensure!(
        (2..=168).contains(&raw.horizon_hours),
        "{what}.horizon_hours must be 2 to 168"
    );
    ensure!(
        (1..=1440).contains(&raw.grace_minutes),
        "{what}.grace_minutes must be 1 to 1440"
    );
    ensure!(
        (1..=366).contains(&raw.lead_days),
        "{what}.lead_days must be 1 to 366"
    );
    ensure!(
        (1..=100_000).contains(&raw.max_events),
        "{what}.max_events must be 1 to 100000"
    );
    ensure!(
        (1..=1000).contains(&raw.max_per_event),
        "{what}.max_per_event must be 1 to 1000"
    );
    ensure!(
        raw.max_repeats <= 100,
        "{what}.max_repeats must be 0 to 100"
    );
    Ok(AlertSettings {
        enabled: raw.enabled,
        horizon: Duration::from_hours(raw.horizon_hours),
        grace: Duration::from_mins(raw.grace_minutes),
        lead: Duration::from_hours(raw.lead_days.saturating_mul(24)),
        max_events: raw.max_events,
        max_per_event: raw.max_per_event,
        max_repeats: raw.max_repeats,
    })
}

/// The mail servers (one of each in this phase).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Upstream {
    pub(crate) imap: MailServer,
    pub(crate) smtp: MailServer,
    pub(crate) network: NetworkScope,
    pub(crate) sent_copy: SentCopy,
    /// Domain for logins without `@` (`alice` → `alice@domain`).
    pub(crate) email_domain: Option<String>,
    /// Calendars and contacts, if configured.
    pub(crate) dav: Option<DavUpstream>,
    /// Sieve scripts and the vacation response, if configured.
    pub(crate) sieve: Option<SieveUpstream>,
}

/// Web Push (`[push]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PushConfig {
    /// The VAPID `sub`.
    pub(crate) contact: nebula_webpush::Contact,
    /// Which endpoints are accepted.
    pub(crate) policy: nebula_webpush::EndpointPolicy,
    pub(crate) max_subscriptions: usize,
    /// Calendar alerts go by Web Push too.
    pub(crate) calendar_alerts: bool,
}

/// The parsed configuration (the master keys are loaded apart).
#[derive(Debug)]
pub(crate) struct Config {
    pub(crate) listen: SocketAddr,
    pub(crate) http: HttpConfig,
    pub(crate) store_path: PathBuf,
    pub(crate) upstream: Upstream,
    /// `None`: no Web Push.
    pub(crate) push: Option<PushConfig>,
    /// Undo send and scheduled send (`[submission]`).
    pub(crate) hold: HoldSettings,
    /// `client_ip = "peer"` with an https `public_url`: every login shares
    /// the TLS proxy's address in the rate limits (`serve` warns).
    pub(crate) client_ip_warning: bool,
    keys: KeySource,
}

/// Where the client IP comes from. The bridge speaks only plain HTTP, so an
/// https `public_url` means a TLS terminator in front, and `peer` would be
/// the proxy's address: the operator has to choose. Plain http is only
/// allowed on loopback, where `peer` is right.
/// Returns the source and whether `serve` must warn.
fn client_ip(raw: Option<RawClientIp>, https: bool) -> anyhow::Result<(ClientIpSource, bool)> {
    match (raw, https) {
        (None, true) => bail!(
            "server.client_ip is required when public_url is https: the bridge speaks only \
             plain HTTP behind a TLS proxy, so \"peer\" would be the proxy's address for every \
             client. Use \"x-forwarded-for\" if the proxy sets that header, or \"peer\" to \
             accept that all clients share the login rate limits"
        ),
        (None | Some(RawClientIp::Peer), false) => Ok((ClientIpSource::Peer, false)),
        (Some(RawClientIp::Peer), true) => Ok((ClientIpSource::Peer, true)),
        (Some(RawClientIp::XForwardedFor), _) => Ok((ClientIpSource::XForwardedFor, false)),
    }
}

fn push(raw: &RawPush) -> anyhow::Result<PushConfig> {
    use nebula_webpush::{EndpointPolicy, HostPattern};
    let contact = raw
        .contact
        .parse()
        .context("push.contact must be mailto:<address> or an https: URL")?;
    let mut policy = match &raw.allowed_hosts {
        None => EndpointPolicy::known_services(),
        Some(hosts) if hosts.iter().any(|h| h == "*") => {
            ensure!(hosts.len() == 1, "push.allowed_hosts: \"*\" goes alone");
            EndpointPolicy::any_public_host()
        }
        Some(hosts) => EndpointPolicy::only(
            hosts
                .iter()
                .map(|host| {
                    host.parse::<HostPattern>()
                        .map_err(|_| anyhow!("push.allowed_hosts: invalid host {host:?}"))
                })
                .collect::<anyhow::Result<Vec<_>>>()?,
        ),
    };
    if raw.loopback {
        policy = policy.with_loopback();
    }
    ensure!(
        (1..=100).contains(&raw.max_subscriptions),
        "push.max_subscriptions must be 1 to 100"
    );
    Ok(PushConfig {
        contact,
        policy,
        max_subscriptions: raw.max_subscriptions,
        calendar_alerts: raw.calendar_alerts,
    })
}

/// Where the master keys come from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum KeySource {
    /// Files by version (resolved against the config file's directory).
    Files(Vec<(KeyVersion, PathBuf)>),
    /// `NEBULA_BRIDGE_MASTER_KEY`.
    Environment,
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// A host name or IP address: no scheme, port, path or spaces.
fn parse_host(text: &str) -> anyhow::Result<String> {
    let text = text.trim();
    if let Ok(ip) = text
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
    {
        return Ok(ip.to_string());
    }
    match url::Host::parse(text) {
        Ok(url::Host::Domain(domain)) if !domain.is_empty() => Ok(domain),
        Ok(url::Host::Ipv4(ip)) => Ok(ip.to_string()),
        _ => bail!("{text:?} is not a host name or IP address"),
    }
}

fn mail_server(raw: &RawMailServer, default_port: u16, what: &str) -> anyhow::Result<MailServer> {
    let host = parse_host(&raw.host).with_context(|| format!("upstream.{what}.host"))?;
    let port = raw.port.unwrap_or(default_port);
    ensure!(port != 0, "upstream.{what}.port must not be 0");
    let certificate = if raw.tls_verify {
        CertificateCheck::Verify
    } else {
        ensure!(
            is_loopback_host(&host),
            "upstream.{what}.tls_verify = false is only allowed for loopback hosts (development)"
        );
        CertificateCheck::InsecureLoopbackOnly
    };
    Ok(MailServer {
        host,
        port,
        tls: raw.tls,
        certificate,
    })
}

/// An http(s) URL without user info, query or fragment.
fn http_url(text: &str, what: &str) -> anyhow::Result<url::Url> {
    let url = url::Url::parse(text.trim()).with_context(|| format!("{what}: not a URL"))?;
    ensure!(
        matches!(url.scheme(), "http" | "https"),
        "{what}: only http and https"
    );
    ensure!(
        url.username().is_empty() && url.password().is_none(),
        "{what}: no user or password in the URL (users log in with their own)"
    );
    ensure!(
        url.query().is_none() && url.fragment().is_none(),
        "{what}: no query or fragment"
    );
    ensure!(url.host_str().is_some(), "{what}: the URL needs a host");
    Ok(url)
}

/// A DAV server URL of the configuration (`url`, `contacts_url`): http(s),
/// no user info, query or fragment, and plain http only for loopback.
fn dav_url(text: &str, what: &str) -> anyhow::Result<url::Url> {
    let url = http_url(text, what)?;
    ensure!(
        url.scheme() == "https" || is_loopback_host(url.host_str().unwrap_or_default()),
        "{what}: http is only allowed for loopback hosts (development); use https"
    );
    Ok(url)
}

fn dav(raw: &RawDav) -> anyhow::Result<DavUpstream> {
    let url = dav_url(&raw.url, "upstream.dav.url")?;
    let contacts_url = raw
        .contacts_url
        .as_deref()
        .map(|text| dav_url(text, "upstream.dav.contacts_url"))
        .transpose()?;
    ensure!(
        contacts_url.as_ref() != Some(&url),
        "upstream.dav.contacts_url is the same as url; remove it"
    );
    ensure!(
        contacts_url.is_none() || raw.contacts,
        "upstream.dav.contacts_url is set but contacts = false; remove one of them"
    );
    let certificate = if raw.tls_verify {
        CertificateCheck::Verify
    } else {
        let loopback = |url: &url::Url| is_loopback_host(url.host_str().unwrap_or_default());
        ensure!(
            loopback(&url) && contacts_url.as_ref().is_none_or(loopback),
            "upstream.dav.tls_verify = false is only allowed for loopback hosts (development)"
        );
        CertificateCheck::InsecureLoopbackOnly
    };
    let allowed_hosts = raw
        .allowed_hosts
        .iter()
        .map(|origin| {
            let what = format!("upstream.dav.allowed_hosts: {origin:?}");
            let url = http_url(origin, &what)?;
            ensure!(
                url.path() == "/",
                "{what}: an origin only (https://host[:port]), no path"
            );
            ensure!(url.scheme() == "https", "{what}: only https");
            Ok(url)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    // The servers that get the login are not "allowed hosts" (which never
    // get it): with two of them, neither may send the other's client there.
    if let Some(contacts) = &contacts_url {
        for configured in [&url, contacts] {
            ensure!(
                !allowed_hosts
                    .iter()
                    .any(|allowed| allowed.origin() == configured.origin()),
                "upstream.dav.allowed_hosts: {} is already a configured server (url or contacts_url); remove it from allowed_hosts",
                configured.origin().ascii_serialization()
            );
        }
    }
    ensure!(
        raw.calendars || raw.contacts,
        "upstream.dav: calendars and contacts are both off; remove [upstream.dav] instead"
    );
    ensure!(
        (10..=86_400).contains(&raw.poll_seconds) && (10..=86_400).contains(&raw.poll_idle_seconds),
        "upstream.dav: poll_seconds and poll_idle_seconds must be 10 to 86400"
    );
    Ok(DavUpstream {
        url,
        contacts_url,
        calendars: raw.calendars,
        contacts: raw.contacts,
        certificate,
        allowed_hosts,
        scheduling: raw.scheduling,
        poll: Duration::from_secs(raw.poll_seconds),
        poll_idle: Duration::from_secs(raw.poll_idle_seconds),
        alerts: alerts(&raw.alerts)?,
    })
}

fn upstream(raw: &RawUpstream) -> anyhow::Result<Upstream> {
    let imap_port = match raw.imap.tls {
        TlsMode::Implicit => 993,
        TlsMode::Starttls => 143,
    };
    let smtp_port = match raw.smtp.tls {
        TlsMode::Implicit => 465,
        TlsMode::Starttls => 587,
    };
    Ok(Upstream {
        imap: mail_server(&raw.imap, imap_port, "imap")?,
        smtp: mail_server(&raw.smtp, smtp_port, "smtp")?,
        network: if raw.private_network {
            NetworkScope::Private
        } else {
            NetworkScope::Public
        },
        sent_copy: raw.sent_copy,
        email_domain: raw
            .email_domain
            .as_deref()
            .map(|domain| parse_host(domain).context("upstream.email_domain"))
            .transpose()?,
        dav: raw.dav.as_ref().map(dav).transpose()?,
        sieve: raw
            .sieve
            .as_ref()
            .map(|sieve_raw| sieve(sieve_raw, &raw.imap))
            .transpose()?,
    })
}

fn clients(raw: Vec<RawClient>) -> anyhow::Result<Vec<OAuthClient>> {
    raw.into_iter()
        .map(|client| {
            let id: ClientId = client
                .id
                .parse()
                .with_context(|| format!("oauth.clients: id {:?}", client.id))?;
            let secret_hash: ClientSecretHash = client.secret_hash.parse().with_context(|| {
                format!("oauth.clients {id}: secret_hash (use `nebula-bridge hash-secret`)")
            })?;
            let redirect_uris = client
                .redirect_uris
                .iter()
                .map(|uri| {
                    RedirectUri::parse(uri).with_context(|| format!("oauth.clients {id}: {uri:?}"))
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            Ok(OAuthClient::new(id, secret_hash, redirect_uris)?)
        })
        .collect()
}

fn jmap_limits(raw: &RawLimits) -> anyhow::Result<JmapLimits> {
    // A client filters by the ids of one /get page (`emailIds`…), and
    // filter lists stop at MAX_FILTER_LIST.
    ensure!(
        usize::try_from(raw.max_objects_in_get).is_ok_and(|max| max <= MAX_FILTER_LIST),
        "limits.max_objects_in_get must be {MAX_FILTER_LIST} or less"
    );
    let n = UnsignedInt::from;
    let core = CoreCapability {
        max_size_upload: n(raw.max_size_upload),
        max_concurrent_upload: n(raw.max_concurrent_upload),
        max_size_request: n(raw.max_size_request),
        max_concurrent_requests: n(raw.max_concurrent_requests),
        max_calls_in_request: n(raw.max_calls_in_request),
        max_objects_in_get: n(raw.max_objects_in_get),
        max_objects_in_set: n(raw.max_objects_in_set),
        collation_algorithms: vec![
            Collation::AsciiNumeric,
            Collation::AsciiCasemap,
            Collation::UnicodeCasemap,
        ],
    };
    JmapLimits::new(core, raw.event_streams_per_account).context("limits")
}

fn lifetimes(raw: &RawSession) -> anyhow::Result<Lifetimes> {
    let day = |days: u64| {
        Duration::from_hours(24).saturating_mul(u32::try_from(days).unwrap_or(u32::MAX))
    };
    Lifetimes::new(
        Duration::from_mins(raw.access_token_minutes),
        day(raw.refresh_token_days),
        day(raw.max_days),
    )
    .context("session")
}

fn hold(raw: &RawSubmission) -> anyhow::Result<HoldSettings> {
    ensure!(
        raw.hold_days <= 30,
        "submission.hold_days must be 0 (no undo send or scheduled send) to 30"
    );
    ensure!(
        (1..=10_000).contains(&raw.max_held),
        "submission.max_held must be 1 to 10000"
    );
    ensure!(
        (1..=100_000).contains(&raw.max_held_mb),
        "submission.max_held_mb must be 1 to 100000"
    );
    ensure!(
        (1..=720).contains(&raw.wait_hours),
        "submission.wait_hours must be 1 to 720"
    );
    let mut hold = HoldSettings::new();
    hold.max_delay = Duration::from_hours(raw.hold_days.saturating_mul(24));
    hold.max_count = raw.max_held;
    hold.max_bytes = raw.max_held_mb.saturating_mul(1024 * 1024);
    hold.max_wait = Duration::from_hours(raw.wait_hours);
    hold.send_while_signed_out = raw.send_while_signed_out;
    Ok(hold)
}

fn login_limits(raw: &RawLoginLimits) -> anyhow::Result<LoginLimits> {
    let window = Duration::from_mins(raw.window_minutes);
    Ok(LoginLimits {
        per_login: RateLimit::new(raw.per_login, window).context("login_limits")?,
        per_ip: RateLimit::new(raw.per_ip, window).context("login_limits")?,
    })
}

fn key_source(raw: RawSecrets, base: &Path, env_set: bool) -> anyhow::Result<KeySource> {
    let mut files = Vec::new();
    if let Some(file) = raw.master_key_file {
        files.push((KeyVersion::new(1)?, base.join(file)));
    }
    for key in raw.keys {
        let version =
            KeyVersion::new(key.version).context("secrets.keys: version must be 1 or more")?;
        files.push((version, base.join(key.file)));
    }
    match (files.is_empty(), env_set) {
        (true, true) => Ok(KeySource::Environment),
        (false, false) => Ok(KeySource::Files(files)),
        (true, false) => bail!(
            "no master key: set secrets.master_key_file, [[secrets.keys]] or {MASTER_KEY_ENV}"
        ),
        (false, true) => {
            bail!("master keys are set both in the file and in {MASTER_KEY_ENV}: use only one")
        }
    }
}

impl Config {
    /// Parses the configuration text. `base` resolves relative paths;
    /// `env_key_set` says whether `NEBULA_BRIDGE_MASTER_KEY` is set.
    pub(crate) fn parse(text: &str, base: &Path, env_key_set: bool) -> anyhow::Result<Self> {
        let raw: RawConfig = toml::from_str(text).context("invalid configuration")?;
        let public_url = PublicUrl::parse(&raw.server.public_url).context("server.public_url")?;
        let (client_ip_source, client_ip_warning) =
            client_ip(raw.server.client_ip, public_url.is_https())?;
        let app_origins = raw
            .server
            .app_origins
            .iter()
            .map(|origin| {
                AppOrigin::parse(origin).with_context(|| format!("server.app_origins: {origin:?}"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let mut http = HttpConfig::new(public_url, app_origins, clients(raw.oauth.clients)?)
            .context("oauth.clients")?;
        http.lifetimes = lifetimes(&raw.session)?;
        http.jmap = jmap_limits(&raw.limits)?;
        http.login_limits = login_limits(&raw.login_limits)?;
        http.client_ip = client_ip_source;
        let hold = hold(&raw.submission)?;
        // The HTTP layer keeps the password of a signed-out account for
        // its held messages, as long as the scheduler may still send them.
        http.signed_out_sending = if hold.sends_while_signed_out() {
            SignedOutSending::On {
                max_wait: hold.max_wait,
            }
        } else {
            SignedOutSending::Off
        };
        Ok(Self {
            listen: raw.server.listen,
            http,
            store_path: base.join(raw.store.path),
            upstream: upstream(&raw.upstream)?,
            push: raw.push.as_ref().map(push).transpose()?,
            hold,
            client_ip_warning,
            keys: key_source(raw.secrets, base, env_key_set)?,
        })
    }

    /// Reads the master keys. The highest version is the current one;
    /// the others only unwrap what they wrapped (rotation).
    pub(crate) fn keyring(&self, env_value: Option<&SecretString>) -> anyhow::Result<Keyring> {
        let keys = match &self.keys {
            KeySource::Files(files) => files
                .iter()
                .map(|(version, path)| read_key_file(*version, path))
                .collect::<anyhow::Result<Vec<_>>>()?,
            KeySource::Environment => {
                let value = env_value.ok_or_else(|| anyhow!("{MASTER_KEY_ENV} is not set"))?;
                parse_key_list(value)?
            }
        };
        keyring(keys)
    }
}

fn keyring(mut keys: Vec<MasterKey>) -> anyhow::Result<Keyring> {
    keys.sort_by_key(MasterKey::version);
    let current = keys.pop().ok_or_else(|| anyhow!("no master key"))?;
    Ok(Keyring::new(current, keys)?)
}

/// `base64` (version 1) or `version:base64,version:base64`.
fn parse_key_list(value: &SecretString) -> anyhow::Result<Vec<MasterKey>> {
    let text = value.expose_secret().trim();
    if !text.contains(':') {
        let key = MasterKey::from_base64(KeyVersion::new(1)?, &SecretString::from(text))
            .context(MASTER_KEY_ENV)?;
        return Ok(vec![key]);
    }
    text.split(',')
        .map(|entry| {
            let (version, key) = entry
                .trim()
                .split_once(':')
                .ok_or_else(|| anyhow!("{MASTER_KEY_ENV}: each entry is version:base64"))?;
            let version: u16 = version
                .parse()
                .map_err(|_| anyhow!("{MASTER_KEY_ENV}: bad version"))?;
            MasterKey::from_base64(KeyVersion::new(version)?, &SecretString::from(key))
                .context(MASTER_KEY_ENV)
        })
        .collect()
}

fn read_key_file(version: KeyVersion, path: &Path) -> anyhow::Result<MasterKey> {
    warn_if_readable_by_others(path);
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read master key file {}", path.display()))?;
    MasterKey::from_base64(version, &SecretString::from(text))
        .with_context(|| format!("master key file {}", path.display()))
}

fn warn_if_readable_by_others(path: &Path) {
    warn_if_exposed(
        path,
        0o077,
        "master key file is readable by group or others; use chmod 600",
    );
}

/// Whether a file mode has any of the permission bits in `mask`.
pub(crate) const fn exposed(mode: u32, mask: u32) -> bool {
    mode & mask != 0
}

/// Logs `message` if `path` exists and its mode has any bit of `mask`
/// (0o077: group or others; 0o007: others). Warns instead of changing the
/// mode, which is the operator's choice.
#[cfg(unix)]
pub(crate) fn warn_if_exposed(path: &Path, mask: u32, message: &str) {
    use std::os::unix::fs::PermissionsExt as _;
    if let Ok(metadata) = std::fs::metadata(path)
        && exposed(metadata.permissions().mode(), mask)
    {
        tracing::warn!(path = %path.display(), "{message}");
    }
}

#[cfg(not(unix))]
pub(crate) fn warn_if_exposed(_path: &Path, _mask: u32, _message: &str) {}

#[cfg(test)]
mod tests {
    use data_encoding::BASE64;

    use super::*;

    const HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHRzYWx0$aGFzaGhhc2hoYXNoaGFzaGhhc2hoYXNoaGFzaGhhc2g";

    fn example(upstream: &str) -> String {
        format!(
            r#"
[server]
listen = "127.0.0.1:8080"
public_url = "https://jmap.example.com"
app_origins = ["https://mail.example.com"]
client_ip = "x-forwarded-for"

[store]
path = "bridge.db"

[secrets]
master_key_file = "master.key"

[[oauth.clients]]
id = "nebulamail"
secret_hash = "{HASH}"
redirect_uris = ["https://mail.example.com/auth/callback"]

{upstream}
"#
        )
    }

    const UPSTREAM: &str = r#"
[upstream.imap]
host = "imap.example.com"

[upstream.smtp]
host = "smtp.example.com"
tls = "starttls"
"#;

    #[test]
    fn parses_the_example_with_defaults() {
        let config = Config::parse(&example(UPSTREAM), Path::new("/etc/bridge"), false).unwrap();
        assert_eq!(config.listen, "127.0.0.1:8080".parse().unwrap());
        assert_eq!(config.store_path, Path::new("/etc/bridge/bridge.db"));
        assert_eq!(
            config.http.public_url().issuer(),
            "https://jmap.example.com"
        );
        assert_eq!(config.upstream.imap.port, 993);
        assert_eq!(config.upstream.imap.tls, TlsMode::Implicit);
        assert_eq!(config.upstream.smtp.port, 587);
        assert_eq!(config.upstream.network, NetworkScope::Public);
        assert_eq!(config.upstream.sent_copy, SentCopy::Append);
        assert_eq!(config.upstream.email_domain, None);
        assert_eq!(
            config.keys,
            KeySource::Files(vec![(
                KeyVersion::new(1).unwrap(),
                PathBuf::from("/etc/bridge/master.key")
            )])
        );
    }

    /// The example with another `[server]` tail (`client_ip` and the URL).
    fn with_server(public_url: &str, client_ip: Option<&str>) -> String {
        let line = client_ip.map_or(String::new(), |value| format!("client_ip = \"{value}\""));
        example(UPSTREAM)
            .replace("client_ip = \"x-forwarded-for\"", &line)
            .replace("https://jmap.example.com", public_url)
    }

    #[test]
    fn https_needs_an_explicit_client_ip() {
        let error = Config::parse(
            &with_server("https://jmap.example.com", None),
            Path::new("/etc/bridge"),
            false,
        )
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("server.client_ip"),
            "{error:#}"
        );

        let config = Config::parse(&example(UPSTREAM), Path::new("/etc/bridge"), false).unwrap();
        assert_eq!(config.http.client_ip, ClientIpSource::XForwardedFor);
        assert!(!config.client_ip_warning);

        let config = Config::parse(
            &with_server("https://jmap.example.com", Some("peer")),
            Path::new("/etc/bridge"),
            false,
        )
        .unwrap();
        assert_eq!(config.http.client_ip, ClientIpSource::Peer);
        assert!(config.client_ip_warning);
    }

    #[test]
    fn plain_http_on_loopback_defaults_to_the_peer() {
        let config = Config::parse(
            &with_server("http://localhost:8080", None),
            Path::new("/etc/bridge"),
            false,
        )
        .unwrap();
        assert_eq!(config.http.client_ip, ClientIpSource::Peer);
        assert!(!config.client_ip_warning);
    }

    #[test]
    fn exposed_modes() {
        assert!(!exposed(0o100_600, 0o077));
        assert!(exposed(0o100_640, 0o077));
        assert!(exposed(0o100_604, 0o077));
        assert!(!exposed(0o040_750, 0o007));
        assert!(exposed(0o040_755, 0o007));
    }

    #[test]
    fn the_shipped_examples_parse() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for file in ["bridge.example.toml", "dev/dev.toml", "dev/dovecot.toml"] {
            let text = std::fs::read_to_string(dir.join(file)).unwrap();
            Config::parse(&text, &dir, false).unwrap_or_else(|e| panic!("{file}: {e:#}"));
        }
    }

    #[test]
    fn submission_holds() {
        let path = Path::new("/etc/bridge");
        let defaults = Config::parse(&example(UPSTREAM), path, false).unwrap();
        assert_eq!(defaults.hold, HoldSettings::new());
        let with =
            |block: &str| Config::parse(&example(&format!("{UPSTREAM}\n{block}")), path, false);
        let config =
            with("[submission]\nhold_days = 30\nmax_held = 5\nmax_held_mb = 10\nwait_hours = 1")
                .unwrap();
        assert_eq!(config.hold.max_delay, Duration::from_hours(30 * 24));
        assert_eq!(config.hold.max_count, 5);
        assert_eq!(config.hold.max_bytes, 10 * 1024 * 1024);
        assert_eq!(config.hold.max_wait, Duration::from_hours(1));
        // Off by default: the password goes with the last session.
        assert!(!defaults.hold.send_while_signed_out);
        assert_eq!(defaults.http.signed_out_sending, SignedOutSending::Off);
        let signed_out =
            with("[submission]\nsend_while_signed_out = true\nwait_hours = 5").unwrap();
        assert!(signed_out.hold.sends_while_signed_out());
        assert_eq!(
            signed_out.http.signed_out_sending,
            SignedOutSending::On {
                max_wait: Duration::from_hours(5)
            }
        );
        let off = with("[submission]\nhold_days = 0").unwrap();
        assert!(!off.hold.enabled());
        // Nothing is held: nothing to send after sign-out.
        let nothing_held =
            with("[submission]\nhold_days = 0\nsend_while_signed_out = true").unwrap();
        assert_eq!(nothing_held.http.signed_out_sending, SignedOutSending::Off);
        for bad in [
            "hold_days = 31",
            "max_held = 0",
            "max_held_mb = 0",
            "wait_hours = 0",
            "wait_hours = 721",
            "hold_days = -1",
            "holdfor = 10",
            "send_while_signed_out = 1",
        ] {
            assert!(with(&format!("[submission]\n{bad}")).is_err(), "{bad}");
        }
    }

    #[test]
    fn web_push() {
        use nebula_webpush::{EndpointPolicy, PushEndpoint};
        let parse = |push: &str| {
            Config::parse(
                &example(&format!("{UPSTREAM}{push}")),
                Path::new("/"),
                false,
            )
        };
        assert!(parse("").unwrap().push.is_none());
        let push = parse("[push]\ncontact = \"mailto:admin@example.com\"\n")
            .unwrap()
            .push
            .unwrap();
        assert_eq!(push.policy, EndpointPolicy::known_services());
        assert_eq!(push.max_subscriptions, 10);
        assert_eq!(push.contact.as_str(), "mailto:admin@example.com");
        let any = parse("[push]\ncontact = \"https://example.com\"\nallowed_hosts = [\"*\"]\n")
            .unwrap()
            .push
            .unwrap();
        assert_eq!(any.policy, EndpointPolicy::any_public_host());
        let listed = parse(
            "[push]\ncontact = \"mailto:a@example.com\"\nallowed_hosts = [\"push.example.org\"]\nloopback = true\n",
        )
        .unwrap()
        .push
        .unwrap();
        assert!(PushEndpoint::parse("https://push.example.org/x", &listed.policy).is_ok());
        assert!(PushEndpoint::parse("https://fcm.googleapis.com/x", &listed.policy).is_err());
        assert!(PushEndpoint::parse("http://127.0.0.1:9/x", &listed.policy).is_ok());
        for bad in [
            "[push]\n",
            "[push]\ncontact = \"admin@example.com\"\n",
            "[push]\ncontact = \"mailto:a@b.c\"\nallowed_hosts = [\"*\", \"x.example.com\"]\n",
            "[push]\ncontact = \"mailto:a@b.c\"\nallowed_hosts = [\"*.com\"]\n",
            "[push]\ncontact = \"mailto:a@b.c\"\nmax_subscriptions = 0\n",
            "[push]\ncontact = \"mailto:a@b.c\"\nunknown = 1\n",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn sieve_server() {
        let parse = |sieve: &str| {
            Config::parse(
                &example(&format!("{UPSTREAM}{sieve}")),
                Path::new("/"),
                false,
            )
        };
        assert!(parse("").unwrap().upstream.sieve.is_none());
        let defaults = parse("[upstream.sieve]\n").unwrap().upstream.sieve.unwrap();
        assert_eq!(defaults.server.host, "imap.example.com");
        assert_eq!(defaults.server.port, 4190);
        assert_eq!(defaults.server.tls, TlsMode::Starttls);
        assert_eq!(defaults.server.certificate, CertificateCheck::Verify);
        assert_eq!(defaults.max_script_size, 1024 * 1024);
        assert_eq!(defaults.max_scripts, 100);
        let custom = parse(
            "[upstream.sieve]\nhost = \"sieve.example.com\"\nport = 5190\ntls = \"implicit\"\nmax_script_size = 65536\nmax_scripts = 20\n",
        )
        .unwrap()
        .upstream
        .sieve
        .unwrap();
        assert_eq!(
            (custom.server.host.as_str(), custom.server.port),
            ("sieve.example.com", 5190)
        );
        assert_eq!(custom.server.tls, TlsMode::Implicit);
        assert_eq!(custom.max_script_size, 65536);
        assert_eq!(custom.max_scripts, 20);
        // The IMAP server's tls_verify = false carries over to the same
        // (loopback) host only.
        let local = Config::parse(
            &example(
                "[upstream.imap]\nhost = \"127.0.0.1\"\ntls_verify = false\n[upstream.smtp]\nhost = \"127.0.0.1\"\ntls_verify = false\n[upstream.sieve]\nport = 24190\n",
            ),
            Path::new("/"),
            false,
        )
        .unwrap();
        assert_eq!(
            local.upstream.sieve.unwrap().server.certificate,
            CertificateCheck::InsecureLoopbackOnly
        );
        for (bad, why) in [
            ("tls_verify = false", "loopback"),
            ("host = \"sieve.example.com/x\"", "host"),
            ("poll_seconds = 5", "poll_seconds"),
            ("poll_seconds = 7200", "poll_seconds"),
            ("port = 0", "port"),
            ("max_script_size = 10", "max_script_size"),
            ("max_script_size = 99999999999", "max_script_size"),
            ("max_scripts = 0", "max_scripts"),
            ("max_scripts = 5000", "max_scripts"),
            ("tls = \"none\"", "tls"),
            ("unknown = 1", "unknown"),
        ] {
            let error = parse(&format!("[upstream.sieve]\n{bad}\n")).unwrap_err();
            assert!(format!("{error:#}").contains(why), "{bad}: {error:#}");
        }
    }

    /// The public guide docs/bridge.md: its config blocks must match the
    /// code (skipped only when the file isn't there, e.g. a crate tarball).
    fn bridge_docs() -> Option<String> {
        std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../docs/bridge.md"
        ))
        .ok()
    }

    /// The `[upstream.sieve]` block of docs/bridge.md lists every key, each
    /// with its default (a key added to `RawSieve` must be documented).
    #[test]
    fn docs_sieve_block_lists_every_key_with_its_default() {
        let Some(docs) = bridge_docs() else {
            return;
        };
        let block = docs
            .split("```toml\n[upstream.sieve]\n")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
            .unwrap();
        let uncommented: String = block
            .lines()
            .flat_map(|line| [line.strip_prefix("# ").unwrap_or(line), "\n"])
            .collect();
        let keys: Vec<&str> = uncommented
            .lines()
            .filter_map(|line| line.split_once(" = "))
            .map(|(key, _)| key.trim())
            .collect();
        assert_eq!(
            keys,
            [
                "host",
                "port",
                "tls",
                "tls_verify",
                "max_script_size",
                "max_scripts",
                "poll_seconds"
            ]
        );
        let parse = |sieve: &str| {
            Config::parse(
                &example(&format!("{UPSTREAM}[upstream.sieve]\n{sieve}")),
                Path::new("/"),
                false,
            )
            .unwrap()
            .upstream
            .sieve
            .unwrap()
        };
        let documented = parse(&uncommented);
        let defaults = parse("");
        assert_eq!(documented.server.host, defaults.server.host);
        assert_eq!(documented.server.port, defaults.server.port);
        assert_eq!(documented.server.tls, defaults.server.tls);
        assert_eq!(documented.server.certificate, defaults.server.certificate);
        assert_eq!(documented.max_script_size, defaults.max_script_size);
        assert_eq!(documented.max_scripts, defaults.max_scripts);
        assert_eq!(documented.poll, defaults.poll);
    }

    #[test]
    fn max_objects_in_get_fits_filter_lists() {
        let parse = |limits: &str| {
            Config::parse(
                &example(&format!("{UPSTREAM}[limits]\n{limits}\n")),
                Path::new("/"),
                false,
            )
        };
        assert!(parse(&format!("max_objects_in_get = {MAX_FILTER_LIST}")).is_ok());
        let error = parse(&format!("max_objects_in_get = {}", MAX_FILTER_LIST + 1)).unwrap_err();
        assert!(
            format!("{error:#}").contains("max_objects_in_get"),
            "{error:#}"
        );
    }

    #[test]
    fn dav_server() {
        let parse =
            |dav: &str| Config::parse(&example(&format!("{UPSTREAM}{dav}")), Path::new("/"), false);
        let config =
            parse("[upstream.dav]\nurl = \"https://dav.example.com/remote.php/dav\"\n").unwrap();
        assert_eq!(
            config.upstream.dav.as_ref().map(|d| d.url.as_str()),
            Some("https://dav.example.com/remote.php/dav")
        );
        let dav = config.upstream.dav.unwrap();
        assert!(dav.calendars && dav.contacts);
        assert_eq!(dav.certificate, CertificateCheck::Verify);
        assert_eq!(dav.scheduling, Scheduling::Auto);
        assert_eq!((dav.poll.as_secs(), dav.poll_idle.as_secs()), (60, 300));
        assert!(parse("").unwrap().upstream.dav.is_none());

        let full = parse(
            "[upstream.dav]\nurl = \"http://127.0.0.1:5232/\"\ncontacts = false\ntls_verify = false\n\
             allowed_hosts = [\"https://files.example.com\"]\nscheduling = \"bridge\"\npoll_seconds = 30\n",
        )
        .unwrap()
        .upstream
        .dav
        .unwrap();
        assert!(!full.contacts);
        assert_eq!(full.certificate, CertificateCheck::InsecureLoopbackOnly);
        assert_eq!(full.scheduling, Scheduling::Bridge);
        assert_eq!(full.allowed_hosts.len(), 1);
        assert_eq!(full.poll.as_secs(), 30);

        for (bad, why) in [
            ("url = \"http://dav.example.com/\"", "loopback"),
            (
                "url = \"https://dav.example.com/\"\ntls_verify = false",
                "loopback",
            ),
            ("url = \"https://alice:pw@dav.example.com/\"", "user"),
            ("url = \"ftp://dav.example.com/\"", "http"),
            ("url = \"https://dav.example.com/?x=1\"", "query"),
            (
                "url = \"https://dav.example.com/\"\nallowed_hosts = [\"https://x.example/path\"]",
                "origin",
            ),
            (
                "url = \"https://dav.example.com/\"\nallowed_hosts = [\"http://x.example\"]",
                "https",
            ),
            (
                "url = \"https://dav.example.com/\"\nscheduling = \"never\"",
                "scheduling",
            ),
            (
                "url = \"https://dav.example.com/\"\npoll_seconds = 1",
                "poll",
            ),
            (
                "url = \"https://dav.example.com/\"\ncalendars = false\ncontacts = false",
                "both off",
            ),
            ("url = \"https://dav.example.com/\"\nunknown = 1", "unknown"),
        ] {
            let error = parse(&format!("[upstream.dav]\n{bad}\n")).unwrap_err();
            assert!(format!("{error:#}").contains(why), "{bad}: {error:#}");
        }
    }

    #[test]
    fn dav_alerts() {
        let parse = |extra: &str| {
            Config::parse(
                &example(&format!(
                    "{UPSTREAM}[upstream.dav]\nurl = \"https://dav.example.com/\"\n{extra}"
                )),
                Path::new("/"),
                false,
            )
            .map(|config| config.upstream.dav.unwrap().alerts)
        };
        // Without the section: what the bridge always did.
        let defaults = parse("").unwrap();
        assert_eq!(
            defaults,
            AlertSettings {
                enabled: true,
                horizon: Duration::from_hours(24),
                grace: Duration::from_mins(10),
                lead: Duration::from_hours(8 * 24),
                max_events: 2000,
                max_per_event: 64,
                max_repeats: 10,
            }
        );
        let groupware = nebula_groupware::alerts::AlertConfig::default();
        assert_eq!(defaults.horizon, groupware.horizon);
        assert_eq!(defaults.grace, groupware.grace);
        assert_eq!(defaults.lead, groupware.max_lead);
        assert_eq!(defaults.max_events, groupware.max_events);
        assert_eq!(defaults.max_per_event, groupware.max_per_event);
        assert_eq!(defaults.max_repeats, groupware.max_repeats);
        // The block of docs/bridge.md, as written there: the defaults.
        if let Some(docs) = bridge_docs() {
            let block = docs
                .split("```toml\n[upstream.dav.alerts]\n")
                .nth(1)
                .and_then(|rest| rest.split("```").next())
                .unwrap();
            assert_eq!(
                parse(&format!("[upstream.dav.alerts]\n{block}")).unwrap(),
                defaults
            );
        }
        // Partly set: the rest keeps its default.
        let some =
            parse("[upstream.dav.alerts]\nenabled = false\ngrace_minutes = 5\nmax_repeats = 0\n")
                .unwrap();
        assert!(!some.enabled);
        assert_eq!(some.grace, Duration::from_mins(5));
        assert_eq!(some.max_repeats, 0);
        assert_eq!(some.horizon, Duration::from_hours(24));
        for (bad, why) in [
            ("horizon_hours = 1", "horizon_hours"),
            ("horizon_hours = 169", "horizon_hours"),
            ("grace_minutes = 0", "grace_minutes"),
            ("grace_minutes = 1441", "grace_minutes"),
            ("lead_days = 0", "lead_days"),
            ("lead_days = 367", "lead_days"),
            ("max_events = 0", "max_events"),
            ("max_per_event = 1001", "max_per_event"),
            ("max_repeats = 101", "max_repeats"),
            ("max_repeats = -1", ""),
            ("grace_minutes = \"10\"", ""),
            ("unknown = 1", "unknown"),
        ] {
            let error = parse(&format!("[upstream.dav.alerts]\n{bad}\n")).unwrap_err();
            assert!(format!("{error:#}").contains(why), "{bad}: {error:#}");
        }
    }

    #[test]
    fn push_calendar_alerts() {
        let parse = |push: &str| {
            Config::parse(
                &example(&format!("{UPSTREAM}{push}")),
                Path::new("/"),
                false,
            )
            .map(|config| config.push.unwrap().calendar_alerts)
        };
        assert!(parse("[push]\ncontact = \"mailto:a@b.c\"\n").unwrap());
        assert!(!parse("[push]\ncontact = \"mailto:a@b.c\"\ncalendar_alerts = false\n").unwrap());
        assert!(parse("[push]\ncontact = \"mailto:a@b.c\"\ncalendar_alerts = 1\n").is_err());
    }

    #[test]
    fn dav_icloud_example() {
        // The iCloud example of docs/bridge.md, as written there: each URL
        // gets Apple's per-account hosts of its own kind, built in.
        let icloud = Config::parse(
            &example(
                "[upstream.imap]\nhost = \"imap.mail.me.com\" # port 993, implicit TLS\n\n\
                 [upstream.smtp]\nhost = \"smtp.mail.me.com\"\ntls = \"starttls\" # port 587\n\n\
                 [upstream.dav]\nurl = \"https://caldav.icloud.com/\" # calendars\n\
                 contacts_url = \"https://contacts.icloud.com/\" # address books\n",
            ),
            Path::new("/"),
            false,
        )
        .unwrap();
        assert_eq!(icloud.upstream.imap.port, 993);
        assert_eq!(icloud.upstream.smtp.port, 587);
        let dav = icloud.upstream.dav.unwrap();
        let service = |url: &url::Url| {
            nebula_dav::DavServer::new(url.as_str())
                .unwrap()
                .account_hosts()
                .map(nebula_dav::AccountHosts::service)
        };
        assert_eq!(service(&dav.url), Some("caldav"));
        assert_eq!(
            dav.contacts_url.as_ref().and_then(service),
            Some("contacts")
        );
        // Any other server has none (Fastmail included).
        let fastmail = url::Url::parse("https://caldav.fastmail.com/").unwrap();
        assert_eq!(service(&fastmail), None);
    }

    #[test]
    fn dav_contacts_url() {
        let parse = |dav: &str| {
            Config::parse(
                &example(&format!("{UPSTREAM}[upstream.dav]\n{dav}\n")),
                Path::new("/"),
                false,
            )
        };
        // Fastmail: calendars and address books on two hosts.
        let fastmail = parse(
            "url = \"https://caldav.fastmail.com/\"\ncontacts_url = \"https://carddav.fastmail.com/\"",
        )
        .unwrap()
        .upstream
        .dav
        .unwrap();
        assert_eq!(fastmail.url.as_str(), "https://caldav.fastmail.com/");
        assert_eq!(
            fastmail.contacts_url.as_ref().map(url::Url::as_str),
            Some("https://carddav.fastmail.com/")
        );
        // The Fastmail example of docs/bridge.md, as written there.
        let documented = Config::parse(
            &example(
                "[upstream.imap]\nhost = \"imap.fastmail.com\"\n\n[upstream.smtp]\nhost = \"smtp.fastmail.com\"\n\n\
                 [upstream.dav]\nurl = \"https://caldav.fastmail.com/\"\ncontacts_url = \"https://carddav.fastmail.com/\"\n",
            ),
            Path::new("/"),
            false,
        )
        .unwrap();
        assert_eq!(documented.upstream.imap.host, "imap.fastmail.com");
        assert!(documented.upstream.dav.unwrap().contacts_url.is_some());
        // Optional: existing configurations are unchanged.
        let single = parse("url = \"https://dav.example.com/\"")
            .unwrap()
            .upstream
            .dav
            .unwrap();
        assert!(single.contacts_url.is_none());
        // Loopback development servers (Radicale + Baïkal), also without
        // certificate checks when both are loopback.
        let local = parse(
            "url = \"http://127.0.0.1:5232/\"\ncontacts_url = \"http://localhost:5233/dav.php/\"\ntls_verify = false",
        )
        .unwrap()
        .upstream
        .dav
        .unwrap();
        assert_eq!(local.certificate, CertificateCheck::InsecureLoopbackOnly);
        // A path is kept (where discovery starts).
        assert_eq!(
            local.contacts_url.map(|u| u.path().to_owned()).as_deref(),
            Some("/dav.php/")
        );

        let url = "url = \"https://caldav.example.com/\"\n";
        for (bad, why) in [
            ("contacts_url = \"http://carddav.example.com/\"", "loopback"),
            // An IP that only looks like loopback in another form is still
            // checked by its normalised host, not as text.
            (
                "contacts_url = \"http://127.0.0.1.example.com/\"",
                "loopback",
            ),
            (
                "contacts_url = \"https://alice:pw@carddav.example.com/\"",
                "user",
            ),
            (
                "contacts_url = \"https://carddav.example.com/?x=1\"",
                "query",
            ),
            (
                "contacts_url = \"https://carddav.example.com/#x\"",
                "fragment",
            ),
            ("contacts_url = \"ftp://carddav.example.com/\"", "http"),
            ("contacts_url = \"file:///etc/passwd\"", "http"),
            ("contacts_url = \"carddav.example.com\"", "URL"),
            ("contacts_url = \"\"", "URL"),
            ("contacts_url = \"https:///\"", "URL"),
            ("contacts_url = 5", "contacts_url"),
            ("contacts_url = \"https://caldav.example.com/\"", "same"),
            (
                "contacts_url = \"https://carddav.example.com/\"\ncontacts = false",
                "contacts = false",
            ),
            // Certificate checks can only be off with both on loopback.
            (
                "contacts_url = \"https://carddav.example.com/\"\ntls_verify = false",
                "loopback",
            ),
            // A server that gets the login is never an allowed host too.
            (
                "contacts_url = \"https://carddav.example.com/\"\nallowed_hosts = [\"https://carddav.example.com\"]",
                "already a configured server",
            ),
            (
                "contacts_url = \"https://carddav.example.com/\"\nallowed_hosts = [\"https://caldav.example.com:443\"]",
                "already a configured server",
            ),
        ] {
            let error = parse(&format!("{url}{bad}")).unwrap_err();
            assert!(format!("{error:#}").contains(why), "{bad}: {error:#}");
        }
        // Loopback url, public contacts_url: tls_verify = false refused.
        let error = parse(
            "url = \"http://127.0.0.1:5232/\"\ncontacts_url = \"https://carddav.example.com/\"\ntls_verify = false",
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("loopback"), "{error:#}");
    }

    #[test]
    fn insecure_tls_only_on_loopback() {
        let insecure = |host: &str| {
            format!(
                "[upstream.imap]\nhost = \"{host}\"\ntls_verify = false\n[upstream.smtp]\nhost = \"{host}\"\ntls_verify = false\n"
            )
        };
        for host in ["127.0.0.1", "localhost", "::1", "[::1]"] {
            assert!(
                Config::parse(&example(&insecure(host)), Path::new("/"), false).is_ok(),
                "{host}"
            );
        }
        for host in ["imap.example.com", "192.168.1.10", "10.0.0.1"] {
            let error =
                Config::parse(&example(&insecure(host)), Path::new("/"), false).unwrap_err();
            assert!(format!("{error:#}").contains("loopback"), "{host}");
        }
    }

    #[test]
    fn rejects_bad_values() {
        let cases = [
            (
                "https://mail.example.com/auth/callback",
                "http://mail.example.com/auth/callback",
            ),
            ("https://jmap.example.com\"", "http://jmap.example.com\""),
            ("app_origins = [", "unknown_key = 1\napp_origins = ["),
            (HASH, "plain-secret"),
            (
                "host = \"imap.example.com\"",
                "host = \"imap.example.com/x\"",
            ),
        ];
        for (from, to) in cases {
            let text = example(UPSTREAM).replace(from, to);
            assert!(Config::parse(&text, Path::new("/"), false).is_err(), "{to}");
        }
        // Both or neither key source.
        assert!(Config::parse(&example(UPSTREAM), Path::new("/"), true).is_err());
        let no_keys = example(UPSTREAM).replace("master_key_file = \"master.key\"", "");
        assert!(Config::parse(&no_keys, Path::new("/"), false).is_err());
        assert!(Config::parse(&no_keys, Path::new("/"), true).is_ok());
    }

    #[test]
    fn master_keys_from_env_and_files() {
        let one = BASE64.encode(&[1u8; 32]);
        let two = BASE64.encode(&[2u8; 32]);
        let keys = parse_key_list(&SecretString::from(one.clone())).unwrap();
        assert_eq!(keys.len(), 1);
        let ring =
            keyring(parse_key_list(&SecretString::from(format!("1:{one}, 2:{two}"))).unwrap())
                .unwrap();
        assert_eq!(ring.current_version().get(), 2);
        assert!(parse_key_list(&SecretString::from("1:short")).is_err());
        assert!(
            keyring(parse_key_list(&SecretString::from(format!("1:{one},1:{two}"))).unwrap())
                .is_err()
        );

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("k1"), format!("{one}\n")).unwrap();
        std::fs::write(dir.path().join("k3"), &two).unwrap();
        let text = example(UPSTREAM).replace(
            "master_key_file = \"master.key\"",
            "[[secrets.keys]]\nversion = 1\nfile = \"k1\"\n[[secrets.keys]]\nversion = 3\nfile = \"k3\"",
        );
        let config = Config::parse(&text, dir.path(), false).unwrap();
        assert_eq!(config.keyring(None).unwrap().current_version().get(), 3);
    }
}
