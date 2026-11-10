// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

use lettre::transport::smtp::authentication::Mechanism;

use crate::core::config::Credentials;
use crate::core::session::{self, AuthenticatedSession, SecureSession, timed};
use crate::error::{self, SmtpError, Stage};

/// The mechanisms we use, in order of preference. Both send the password
/// itself, which is fine only because the session is already encrypted.
const MECHANISMS: [Mechanism; 2] = [Mechanism::Plain, Mechanism::Login];

impl SecureSession {
    /// Authenticates with `AUTH PLAIN` (or `AUTH LOGIN` if that is all the
    /// server offers).
    ///
    /// # Errors
    ///
    /// [`SmtpError::AuthUnavailable`] if neither is offered,
    /// [`SmtpError::Auth`] if the server refuses, and the network errors.
    pub async fn authenticate(
        mut self,
        credentials: &Credentials,
    ) -> Result<AuthenticatedSession, SmtpError> {
        if self
            .conn
            .server_info()
            .get_auth_mechanism(&MECHANISMS)
            .is_none()
        {
            session::quit(&mut self.conn).await;
            return Err(SmtpError::AuthUnavailable);
        }
        let result = {
            // lettre's copy of the password lives only in this block (and is
            // wiped when dropped: vendor/lettre/PATCHES.md).
            let credentials = credentials.to_lettre();
            timed(
                Stage::Auth,
                self.timeouts.command,
                self.conn.auth(&MECHANISMS, &credentials),
            )
            .await?
        };
        match result {
            Ok(_) => Ok(AuthenticatedSession {
                conn: self.conn,
                timeouts: self.timeouts,
                extensions: self.extensions,
                activity: self.activity,
                broken: false,
            }),
            Err(error) => {
                let error = match error::reply(&error) {
                    Some((code, text)) => SmtpError::Auth { code, text },
                    None => SmtpError::from_lettre(Stage::Auth, &error),
                };
                // A refused login leaves a working session: say goodbye.
                if matches!(error, SmtpError::Auth { .. }) {
                    session::quit(&mut self.conn).await;
                }
                Err(error)
            }
        }
    }
}
