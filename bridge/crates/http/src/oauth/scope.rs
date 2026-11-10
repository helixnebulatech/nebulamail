// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! OAuth scopes the bridge knows.

/// Access to the mail (JMAP).
pub(crate) const MAIL: &str = "urn:ietf:params:oauth:scope:mail";
/// A refresh token.
pub(crate) const OFFLINE_ACCESS: &str = "offline_access";
/// For the discovery document.
pub(crate) const SUPPORTED: [&str; 2] = [MAIL, OFFLINE_ACCESS];

/// A granted scope. Mail is always granted (it is the only thing the bridge
/// does); other requested scopes (the web app also asks for contacts,
/// calendars and files) are ignored, as RFC 6749 §3.3 allows. Stalwart 1.0
/// is stricter: it refuses unknown scope names and ones the client wasn't
/// registered for (`invalid_scope`).
///
/// A refresh token is issued whatever the scope: the web BFF needs one and
/// is the only (confidential) client. `offline_access` is only echoed back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Scope {
    offline_access: bool,
}

impl Scope {
    /// The scope granted for a requested `scope` parameter.
    pub(crate) fn granted(requested: Option<&str>) -> Self {
        let offline_access = requested
            .unwrap_or_default()
            .split(' ')
            .any(|scope| scope == OFFLINE_ACCESS);
        Self { offline_access }
    }

    /// The scope as stored and sent in token responses.
    pub(crate) fn as_str(self) -> &'static str {
        if self.offline_access {
            "urn:ietf:params:oauth:scope:mail offline_access"
        } else {
            MAIL
        }
    }
}

/// Whether a stored scope string grants mail access.
pub(crate) fn grants_mail(stored: &str) -> bool {
    stored.split(' ').any(|scope| scope == MAIL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mail_is_always_granted() {
        let web = Scope::granted(Some("urn:ietf:params:oauth:scope:mail offline_access"));
        assert_eq!(
            web.as_str(),
            "urn:ietf:params:oauth:scope:mail offline_access"
        );
        assert_eq!(Scope::granted(None).as_str(), MAIL);
        assert_eq!(Scope::granted(Some("openid email")).as_str(), MAIL);
        assert!(grants_mail(web.as_str()));
        assert!(!grants_mail("offline_access"));
    }
}
