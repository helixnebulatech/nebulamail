// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Autodiscover, its plain XML form (`POX`). Redirects to other hosts are
//! not followed, in HTTP or in the answer.

use net_guard::DomainName;

use crate::candidate::{Candidate, Security, Service, Source};
use crate::http::Fetch;

fn request(domain: &DomainName) -> String {
    // Autodiscover wants an address: one every domain must have
    // (RFC 5321 §4.5.1), so no user's address is told to anybody.
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<Autodiscover xmlns=\"http://schemas.microsoft.com/exchange/autodiscover/outlook/requestschema/2006\">\
<Request><EMailAddress>postmaster@{}</EMailAddress>\
<AcceptableResponseSchema>http://schemas.microsoft.com/exchange/autodiscover/outlook/responseschema/2006a</AcceptableResponseSchema>\
</Request></Autodiscover>",
        domain.as_str()
    )
}

fn child<'a>(node: roxmltree::Node<'a, 'a>, name: &str) -> Option<&'a str> {
    node.children()
        .find(|item| item.tag_name().name() == name)?
        .text()
        .map(str::trim)
}

fn security(protocol: roxmltree::Node<'_, '_>, port: u16) -> Option<Security> {
    match child(protocol, "Encryption")
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("ssl") => return Some(Security::ImplicitTls),
        Some("tls") => return Some(Security::StartTls),
        Some("none") => return None,
        _ => {}
    }
    if child(protocol, "SSL").is_some_and(|ssl| ssl.eq_ignore_ascii_case("off")) {
        return None;
    }
    match port {
        143 | 587 => Some(Security::StartTls),
        _ => Some(Security::ImplicitTls),
    }
}

pub(crate) fn parse(xml: &str) -> Vec<Candidate> {
    let Ok(document) = roxmltree::Document::parse(xml) else {
        return Vec::new();
    };
    document
        .descendants()
        .filter(|node| node.tag_name().name() == "Protocol")
        .filter_map(|protocol| {
            let service = match child(protocol, "Type")? {
                "IMAP" => Service::Imap,
                "SMTP" => Service::Smtp,
                _ => return None,
            };
            let port = child(protocol, "Port")?.parse().ok()?;
            Some(Candidate {
                service,
                source: Source::Autodiscover,
                host: child(protocol, "Server")?.to_owned(),
                port,
                security: security(protocol, port),
            })
        })
        .collect()
}

pub(crate) async fn ask(fetch: &dyn Fetch, domain: &DomainName) -> Vec<Candidate> {
    let url = format!(
        "https://autodiscover.{}/autodiscover/autodiscover.xml",
        domain.as_str()
    );
    match fetch.post_xml(&url, request(domain)).await {
        Ok(body) => parse(&body),
        Err(error) => {
            tracing::debug!(%error, "autoconfig: no Autodiscover");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_protocols() {
        let found = parse(
            r#"<?xml version="1.0"?>
<Autodiscover xmlns="http://schemas.microsoft.com/exchange/autodiscover/responseschema/2006">
 <Response xmlns="http://schemas.microsoft.com/exchange/autodiscover/outlook/responseschema/2006a">
  <Account><Protocol><Type>IMAP</Type><Server>imap.example.com</Server><Port>993</Port><SSL>on</SSL></Protocol>
  <Protocol><Type>SMTP</Type><Server>smtp.example.com</Server><Port>587</Port><Encryption>TLS</Encryption></Protocol>
  <Protocol><Type>IMAP</Type><Server>plain.example.com</Server><Port>143</Port><SSL>off</SSL></Protocol>
  <Protocol><Type>EXCH</Type><Server>x</Server><Port>1</Port></Protocol></Account>
 </Response></Autodiscover>"#,
        );
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].security, Some(Security::ImplicitTls));
        assert_eq!(found[1].security, Some(Security::StartTls));
        assert_eq!(found[2].security, None);
    }
}
