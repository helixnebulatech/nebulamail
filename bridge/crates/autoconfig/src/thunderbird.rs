// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Thunderbird's autoconfig file (`clientConfig`), from the domain itself
//! or from the public database.

use net_guard::DomainName;

use crate::candidate::{Candidate, Security, Service, Source};

/// The two places a domain keeps its own file.
pub(crate) fn own_urls(domain: &DomainName) -> [String; 2] {
    let domain = domain.as_str();
    [
        format!("https://autoconfig.{domain}/mail/config-v1.1.xml"),
        format!("https://{domain}/.well-known/autoconfig/mail/config-v1.1.xml"),
    ]
}

pub(crate) fn ispdb_url(domain: &DomainName) -> String {
    format!(
        "https://autoconfig.thunderbird.net/v1.1/{}",
        domain.as_str()
    )
}

fn child<'a>(node: roxmltree::Node<'a, 'a>, name: &str) -> Option<&'a str> {
    node.children()
        .find(|item| item.has_tag_name(name))?
        .text()
        .map(str::trim)
}

/// Every IMAP and SMTP server the file names, in its order. Anything
/// else (POP3, a broken entry, a broken file) is skipped. roxmltree
/// refuses DTDs, so there are no entities to expand.
pub(crate) fn parse(xml: &str, domain: &DomainName, source: Source) -> Vec<Candidate> {
    let Ok(document) = roxmltree::Document::parse(xml) else {
        return Vec::new();
    };
    document
        .descendants()
        .filter_map(|node| {
            let service = match (node.tag_name().name(), node.attribute("type")) {
                ("incomingServer", Some("imap")) => Service::Imap,
                ("outgoingServer", Some("smtp")) => Service::Smtp,
                _ => return None,
            };
            let security = match child(node, "socketType")? {
                "SSL" => Some(Security::ImplicitTls),
                "STARTTLS" => Some(Security::StartTls),
                _ => None,
            };
            Some(Candidate {
                service,
                source,
                host: child(node, "hostname")?.replace("%EMAILDOMAIN%", domain.as_str()),
                port: child(node, "port")?.parse().ok()?,
                security,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"<?xml version="1.0"?>
<clientConfig version="1.1">
  <emailProvider id="example.com">
    <domain>example.com</domain>
    <incomingServer type="pop3">
      <hostname>pop.example.com</hostname><port>995</port><socketType>SSL</socketType>
    </incomingServer>
    <incomingServer type="imap">
      <hostname>imap.%EMAILDOMAIN%</hostname><port>993</port><socketType>SSL</socketType>
    </incomingServer>
    <incomingServer type="imap">
      <hostname>plain.example.com</hostname><port>143</port><socketType>plain</socketType>
    </incomingServer>
    <outgoingServer type="smtp">
      <hostname>smtp.example.com</hostname><port>587</port><socketType>STARTTLS</socketType>
    </outgoingServer>
  </emailProvider>
</clientConfig>"#;

    #[test]
    fn reads_imap_and_smtp() {
        let domain: DomainName = match "example.com".parse().unwrap() {
            net_guard::Host::Domain(name) => name,
            other => panic!("{other:?}"),
        };
        let found = parse(FILE, &domain, Source::Autoconfig);
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].host, "imap.example.com");
        assert_eq!(found[0].security, Some(Security::ImplicitTls));
        assert_eq!(found[1].security, None);
        assert_eq!(found[2].service, Service::Smtp);
        assert_eq!(found[2].security, Some(Security::StartTls));
    }

    #[test]
    fn dtds_and_garbage_give_nothing() {
        let domain = match "example.com".parse().unwrap() {
            net_guard::Host::Domain(name) => name,
            other => panic!("{other:?}"),
        };
        let bomb = r#"<!DOCTYPE a [<!ENTITY b "c">]><clientConfig>&b;</clientConfig>"#;
        assert!(parse(bomb, &domain, Source::Ispdb).is_empty());
        assert!(parse("not xml", &domain, Source::Ispdb).is_empty());
    }
}
