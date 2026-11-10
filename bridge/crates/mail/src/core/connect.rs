// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Opening a logged-in IMAP connection: `net-guard` → TLS → LOGIN.

use std::sync::Arc;

use nebula_imap::{AlertSink, Authenticated, Client, Connected, Credentials, TlsConnector};
use net_guard::Resolver;
use rustls_pki_types::ServerName;

use crate::{ImapServer, MailError};

/// Everything needed to connect to the configured server.
pub(crate) struct Connector {
    server: ImapServer,
    server_name: ServerName<'static>,
    tls: TlsConnector,
    resolver: Arc<dyn Resolver>,
}

impl std::fmt::Debug for Connector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connector")
            .field("server", &self.server)
            .finish_non_exhaustive()
    }
}

impl Connector {
    pub(crate) fn new(server: ImapServer, resolver: Arc<dyn Resolver>) -> Result<Self, MailError> {
        let tls = TlsConnector::new(server.verify()).map_err(MailError::Config)?;
        let server_name = server.server_name()?;
        Ok(Self {
            server,
            server_name,
            tls,
            resolver,
        })
    }

    /// Connects through `net-guard` (port policy, every resolved address
    /// checked, connection pinned to the validated IP) and logs in. The
    /// server's `[ALERT]`s, from the greeting on, go to `alerts`.
    pub(crate) async fn login(
        &self,
        credentials: &Credentials,
        alerts: Arc<dyn AlertSink>,
    ) -> Result<Client<Authenticated>, MailError> {
        let stream = net_guard::connect(
            &self.server.target(),
            &self.server.policy(),
            self.resolver.as_ref(),
        )
        .await?;
        let connected = Client::connect(
            stream.into_stream(),
            self.server_name.clone(),
            self.server.tls(),
            &self.tls,
            self.server.limits,
        )
        .await?;
        match connected {
            Connected::NotAuthenticated(mut client) => {
                client.set_alert_sink(alerts);
                client
                    .login(credentials)
                    .await
                    .map_err(|error| MailError::from(error.into_parts().0))
            }
            // The bridge checks passwords by logging in: a
            // server that greets with PREAUTH would accept any password.
            _ => Err(MailError::Unsupported("PREAUTH greeting")),
        }
    }
}
