// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Builds the JMAP backend (`nebula-jmap` over `nebula-mail` and
//! `nebula-smtp`, and `nebula-groupware` if `[upstream.dav]` is set)
//! from the `[upstream]` configuration.

use std::sync::Arc;

use anyhow::Context as _;
use nebula_jmap::{
    JmapBackend, JmapConfig, SentCopy as JmapSentCopy, SmtpSettings, WebPushSettings,
};
use nebula_mail::{CertificateCheck as ImapCertificates, ImapServer, Mail, MailConfig};
use nebula_smtp::{CertificateCheck as SmtpCertificates, Security, SmtpClient, SmtpServer};
use nebula_store::Store;
use nebula_webpush::{PushClient, VapidKey};
use net_guard::{Host, NetworkAccess, Policy, Port, PortPolicy, Resolver, SystemResolver, Target};

use nebula_dav::{AllowedOrigin, DavServer, Origin, TlsVerify};
use nebula_groupware::{Groupware, GroupwareConfig, Scheduling};
use nebula_jmap::{HoldSettings, SieveSettings};
use nebula_managesieve::{
    CertificateCheck as SieveCertificates, Connector as SieveConnector, SieveServer,
    Tls as SieveTls,
};

use crate::config::{
    CertificateCheck, MailServer, NetworkScope, PushConfig, Scheduling as DavScheduling, SentCopy,
    TlsMode, Upstream,
};

fn network(scope: NetworkScope) -> NetworkAccess {
    match scope {
        NetworkScope::Public => NetworkAccess::PublicOnly,
        NetworkScope::Private => NetworkAccess::PrivateAllowed,
    }
}

/// Where to connect. The admin chose the port, so it is the only one
/// allowed for this server.
fn target(server: &MailServer, what: &str) -> anyhow::Result<(Target, PortPolicy)> {
    let host: Host = server
        .host
        .parse()
        .with_context(|| format!("upstream.{what}.host"))?;
    let port = Port::try_from(server.port).with_context(|| format!("upstream.{what}.port"))?;
    Ok((Target::new(host, port), PortPolicy::only(vec![port])))
}

fn imap(upstream: &Upstream) -> anyhow::Result<ImapServer> {
    let (target, ports) = target(&upstream.imap, "imap")?;
    let tls = match upstream.imap.tls {
        TlsMode::Implicit => nebula_mail::TlsMode::Implicit,
        TlsMode::Starttls => nebula_mail::TlsMode::StartTls,
    };
    let certificates = match upstream.imap.certificate {
        CertificateCheck::Verify => ImapCertificates::Platform,
        CertificateCheck::InsecureLoopbackOnly => ImapCertificates::InsecureLoopbackOnly,
    };
    Ok(ImapServer::new(target.host().clone(), target.port(), tls)
        .with_certificates(certificates)
        .with_private_network(upstream.network == NetworkScope::Private)
        .with_ports(ports))
}

fn smtp(upstream: &Upstream) -> anyhow::Result<SmtpSettings> {
    let (target, ports) = target(&upstream.smtp, "smtp")?;
    let security = match upstream.smtp.tls {
        TlsMode::Implicit => Security::ImplicitTls,
        TlsMode::Starttls => Security::StartTls,
    };
    let certificates = match upstream.smtp.certificate {
        CertificateCheck::Verify => SmtpCertificates::Platform,
        CertificateCheck::InsecureLoopbackOnly => SmtpCertificates::InsecureLoopbackOnly,
    };
    let server = SmtpServer::new(
        upstream
            .smtp
            .host
            .parse()
            .context("upstream.smtp.host is not a TLS name")?,
        security,
    )
    .with_certificates(certificates);
    let client = SmtpClient::new(server).context("SMTP TLS setup")?;
    Ok(SmtpSettings::new(
        client,
        target,
        Policy::new(ports, network(upstream.network)),
    ))
}

/// The largest literal the ManageSieve server may send: the largest
/// script allowed plus a margin (a quarter more and 64 KiB), since
/// scripts other clients wrote may be a little bigger than ours. Never
/// more than the largest size allowed at all.
fn sieve_literal(max_script_size: usize) -> anyhow::Result<usize> {
    let ceiling = usize::try_from(crate::config::MAX_SIEVE_SCRIPT).context("script size")?;
    Ok(max_script_size
        .saturating_add(max_script_size / 4)
        .saturating_add(64 * 1024)
        .min(ceiling))
}

/// ManageSieve, if `[upstream.sieve]` is set: the same
/// network rules as IMAP; only the configured port.
fn sieve(
    upstream: &Upstream,
    resolver: &Arc<dyn Resolver>,
) -> anyhow::Result<Option<SieveSettings>> {
    let Some(sieve) = &upstream.sieve else {
        return Ok(None);
    };
    let (target, ports) = target(&sieve.server, "sieve")?;
    let tls = match sieve.server.tls {
        TlsMode::Implicit => SieveTls::Implicit,
        TlsMode::Starttls => SieveTls::StartTls,
    };
    let certificates = match sieve.server.certificate {
        CertificateCheck::Verify => SieveCertificates::Platform,
        CertificateCheck::InsecureLoopbackOnly => SieveCertificates::InsecureLoopbackOnly,
    };
    let mut limits = nebula_managesieve::Limits::default();
    let literal = sieve_literal(sieve.max_script_size)?;
    limits.frame.max_literal = literal;
    limits.frame.max_line_literals = literal;
    let mut server = SieveServer::new(target.host().clone(), target.port(), tls)
        .with_certificates(certificates)
        .with_private_network(upstream.network == NetworkScope::Private)
        .with_limits(limits);
    server.ports = ports;
    let connector =
        SieveConnector::new(server, Arc::clone(resolver)).context("ManageSieve TLS setup")?;
    let mut settings = SieveSettings::new(connector);
    settings.max_script_size = sieve.max_script_size;
    settings.max_scripts = Some(sieve.max_scripts);
    settings.poll_interval = Some(sieve.poll);
    Ok(Some(settings))
}

/// The CalDAV/CardDAV service, if `[upstream.dav]` is set. The admin's
/// `private_network` applies to it and to its allowed hosts.
fn groupware(
    upstream: &Upstream,
    store: &Store,
    resolver: &Arc<dyn Resolver>,
) -> anyhow::Result<Option<Groupware>> {
    let Some(dav) = &upstream.dav else {
        return Ok(None);
    };
    let access = network(upstream.network);
    let certificates = match dav.certificate {
        CertificateCheck::Verify => TlsVerify::Platform,
        CertificateCheck::InsecureLoopbackOnly => TlsVerify::InsecureLoopbackOnly,
    };
    // `url` and, if set, `contacts_url`: the same networks, certificate
    // check and allowed hosts; each gets the login on its own origin only.
    let server_at = |url: &url::Url, what: &str| -> anyhow::Result<DavServer> {
        let mut server = DavServer::new(url.as_str())
            .map_err(|error| anyhow::anyhow!("upstream.dav.{what}: {error}"))?
            .with_private_network(upstream.network == NetworkScope::Private)
            .with_certificates(certificates);
        for host in &dav.allowed_hosts {
            let origin = Origin::parse(host.as_str())
                .map_err(|error| anyhow::anyhow!("upstream.dav.allowed_hosts: {error}"))?;
            server = server.with_allowed_origin(AllowedOrigin {
                origin,
                network: access,
            });
        }
        Ok(server)
    };
    let server = server_at(&dav.url, "url")?;
    let contacts_server = dav
        .contacts_url
        .as_ref()
        .map(|url| server_at(url, "contacts_url"))
        .transpose()?;
    let mut config = GroupwareConfig::new(server);
    config.contacts_server = contacts_server;
    config.calendars = dav.calendars;
    config.contacts = dav.contacts;
    config.scheduling = match dav.scheduling {
        DavScheduling::Auto => Scheduling::Auto,
        DavScheduling::Bridge => Scheduling::Bridge,
        DavScheduling::Server => Scheduling::Server,
    };
    config.poll_with_push = dav.poll;
    config.poll_without_push = dav.poll_idle;
    let alerts = &mut config.alerts;
    alerts.enabled = dav.alerts.enabled;
    alerts.horizon = dav.alerts.horizon;
    alerts.grace = dav.alerts.grace;
    alerts.max_lead = dav.alerts.lead;
    alerts.max_events = dav.alerts.max_events;
    alerts.max_per_event = dav.alerts.max_per_event;
    alerts.max_repeats = dav.alerts.max_repeats;
    Ok(Some(Groupware::new(
        store.clone(),
        config,
        Arc::clone(resolver),
    )))
}

/// The backend and its mail service, on `store`; Web Push with this
/// configuration and VAPID key, if given.
pub(crate) fn build(
    upstream: &Upstream,
    store: &Store,
    hold: HoldSettings,
    push: Option<(PushConfig, Arc<VapidKey>)>,
) -> anyhow::Result<JmapBackend> {
    let resolver: Arc<dyn Resolver> =
        Arc::new(SystemResolver::new().context("DNS resolver setup")?);
    let mail = Mail::new(
        store.clone(),
        MailConfig::new(imap(upstream)?),
        Arc::clone(&resolver),
    )
    .context("IMAP setup")?;
    let mut config = JmapConfig::new(smtp(upstream)?);
    config.sent_copy = match upstream.sent_copy {
        SentCopy::Append => JmapSentCopy::Append,
        SentCopy::Server => JmapSentCopy::Server,
    };
    config.email_domain.clone_from(&upstream.email_domain);
    config.hold = hold;
    if let Some((push, vapid)) = push {
        let client = PushClient::new(vapid, push.contact, Arc::clone(&resolver))
            .context("Web Push TLS setup")?;
        let mut settings = WebPushSettings::new(client, push.policy);
        settings.max_subscriptions = push.max_subscriptions;
        settings.calendar_alerts = push.calendar_alerts;
        if let Some(dav) = &upstream.dav {
            settings.alert_ttl = dav.alerts.grace;
        }
        config.web_push = Some(settings);
    }
    config.sieve = sieve(upstream, &resolver)?;
    let groupware = groupware(upstream, store, &resolver)?;
    Ok(JmapBackend::with_groupware(
        store.clone(),
        mail,
        groupware,
        resolver,
        config,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ManageSieve literal limit follows the script size (with a
    /// margin), not always the 16 MiB ceiling.
    #[test]
    fn sieve_literal_follows_the_script_size() {
        assert_eq!(
            sieve_literal(1024 * 1024).unwrap(),
            1024 * 1024 + 256 * 1024 + 64 * 1024
        );
        let ceiling = usize::try_from(crate::config::MAX_SIEVE_SCRIPT).unwrap();
        assert_eq!(sieve_literal(ceiling).unwrap(), ceiling);
    }
}
