// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `nebula-bridge discover`: shows the admin what a domain's DNS and
//! autoconfiguration offer, as configuration to review and paste.
//! Nothing is written and no password is sent: where the users'
//! passwords go stays the admin's decision.

use std::fmt::Write as _;
use std::sync::Arc;

use anyhow::{Context as _, bail};
use nebula_autoconfig::candidate::{Endpoint, Security, Service};
use nebula_autoconfig::check::{Checked, check};
use nebula_autoconfig::http::GuardedFetch;
use nebula_autoconfig::srv::SystemSrv;
use nebula_autoconfig::{Discovery, Options, Report};
use net_guard::{DomainName, Host, NetworkAccess, Resolver, SystemResolver};

/// The command line of `discover`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Arguments {
    domain: DomainName,
    options: Options,
}

impl Arguments {
    pub(crate) fn parse(args: &[String]) -> anyhow::Result<Self> {
        let mut domain = None;
        let mut options = Options::default();
        for arg in args {
            match arg.as_str() {
                "--private-network" => options.network = NetworkAccess::PrivateAllowed,
                "--no-ispdb" => options.ispdb = false,
                flag if flag.starts_with('-') => bail!("unknown option {flag}"),
                name if domain.is_none() => {
                    let host: Host = name.parse().context("not a domain name")?;
                    let Host::Domain(name) = host else {
                        bail!("give a domain name, not an address");
                    };
                    domain = Some(name);
                }
                _ => bail!("only one domain"),
            }
        }
        let domain = domain.context("which domain?")?;
        Ok(Self { domain, options })
    }
}

fn config_block(endpoint: &Endpoint) -> Option<String> {
    let section = match endpoint.service {
        Service::Imap => "imap",
        Service::Smtp => "smtp",
        Service::Sieve => "sieve",
        // `url` starts discovery; both go in one section (below).
        _ => return None,
    };
    let tls = match endpoint.security {
        Security::ImplicitTls => "implicit",
        _ => "starttls",
    };
    Some(format!(
        "[upstream.{section}]\nhost = \"{}\"\nport = {}\ntls = \"{tls}\"\n",
        endpoint.host, endpoint.port
    ))
}

fn dav_block(report: &Report) -> Option<String> {
    let calendars = report.get(Service::CalDav);
    let contacts = report.get(Service::CardDav);
    let url = |endpoint: &Endpoint| format!("https://{}/", endpoint.host);
    let mut block = String::from("[upstream.dav]\n");
    match (calendars, contacts) {
        (None, None) => return None,
        (Some(one), None) | (None, Some(one)) => {
            let _ = writeln!(block, "url = \"{}\"", url(one));
        }
        (Some(calendars), Some(contacts)) => {
            let _ = writeln!(block, "url = \"{}\"", url(calendars));
            if calendars.host != contacts.host {
                let _ = writeln!(block, "contacts_url = \"{}\"", url(contacts));
            }
        }
    }
    Some(block)
}

/// What to print: the checks and refusals as comments, then the config.
pub(crate) fn render(domain: &str, report: &Report, checks: &[(Endpoint, Checked)]) -> String {
    let mut out = format!(
        "# Found for {domain}. Check these hosts before using them: the users' passwords\n\
         # will go there. DNS and these files are not authenticated.\n"
    );
    for (endpoint, checked) in checks {
        let state = match checked {
            Checked::Works => "works".to_owned(),
            Checked::Answers => "answers (STARTTLS not tried)".to_owned(),
            Checked::Failed(why) => format!("FAILED: {why}"),
            _ => "unknown".to_owned(),
        };
        let _ = writeln!(
            out,
            "# {}: {}:{} (from {}): {state}",
            endpoint.service, endpoint.host, endpoint.port, endpoint.source
        );
    }
    for refused in &report.refused {
        let _ = writeln!(
            out,
            "# Refused {} {:?} (from {}): {}",
            refused.service,
            refused.host.escape_debug().to_string(),
            refused.source,
            refused.reason
        );
    }
    if report.found.is_empty() {
        out.push_str("# Nothing usable found.\n");
    }
    for (endpoint, checked) in checks {
        // A guess that isn't there is no finding.
        let guess_failed = endpoint.source == nebula_autoconfig::candidate::Source::SameHost
            && matches!(checked, Checked::Failed(_));
        if !guess_failed && let Some(block) = config_block(endpoint) {
            out.push('\n');
            out.push_str(&block);
        }
    }
    if let Some(block) = dav_block(report) {
        out.push('\n');
        out.push_str(&block);
    }
    out
}

/// Runs discovery and returns the text to print.
pub(crate) async fn run(arguments: Arguments) -> anyhow::Result<String> {
    let resolver: Arc<dyn Resolver> =
        Arc::new(SystemResolver::new().context("DNS resolver setup")?);
    let srv = Arc::new(SystemSrv::new().context("DNS resolver setup")?);
    let fetch = Arc::new(GuardedFetch::new(Arc::clone(&resolver)).context("TLS setup")?);
    let discovery = Discovery::new(Arc::clone(&resolver), srv, fetch, arguments.options);
    let report = discovery.run(&arguments.domain).await;
    let mut checks = Vec::with_capacity(report.found.len());
    for endpoint in &report.found {
        let checked = check(endpoint, &resolver, arguments.options.network).await;
        checks.push((endpoint.clone(), checked));
    }
    Ok(render(arguments.domain.as_str(), &report, &checks))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> anyhow::Result<Arguments> {
        Arguments::parse(&list.iter().map(ToString::to_string).collect::<Vec<_>>())
    }

    #[test]
    fn arguments() {
        let parsed = args(&["example.com", "--no-ispdb", "--private-network"]).unwrap();
        assert_eq!(parsed.domain.as_str(), "example.com");
        assert!(!parsed.options.ispdb);
        assert_eq!(parsed.options.network, NetworkAccess::PrivateAllowed);
        assert!(args(&[]).is_err());
        assert!(args(&["10.0.0.1"]).is_err());
        assert!(args(&["a.example", "b.example"]).is_err());
        assert!(args(&["example.com", "--nope"]).is_err());
    }
}
