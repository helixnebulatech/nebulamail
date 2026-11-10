// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! CAPABILITY and NOOP (valid in every state).

use nebula_imap_proto::command;
use nebula_imap_proto::response::{ResponseCode, Untagged};
use nebula_imap_proto::types::Capabilities;

use crate::Error;
use crate::core::client::{Client, sealed::State};
use crate::core::connection::{StrayContinuation, expect_ok};

impl<S: State> Client<S> {
    /// Asks the server for its capabilities again.
    ///
    /// # Errors
    ///
    /// Network or protocol errors.
    pub async fn refresh_capabilities(&mut self) -> Result<&Capabilities, Error> {
        let mut found = None;
        let tagged = self
            .run_with(
                &command::Capability,
                StrayContinuation::Reject,
                |data| match data {
                    Untagged::Capabilities(capabilities) => {
                        found = Some(capabilities);
                        None
                    }
                    other => Some(other),
                },
            )
            .await?;
        let tagged = expect_ok("CAPABILITY", tagged)?;
        let ((Some(capabilities), _) | (None, Some(ResponseCode::Capabilities(capabilities)))) =
            (found, tagged.code)
        else {
            return Err(Error::Protocol("CAPABILITY without capabilities"));
        };
        self.session.capabilities = capabilities;
        Ok(&self.session.capabilities)
    }

    /// `NOOP`: lets the server report changes, which end up in
    /// [`Client::take_events`]. Also keeps the connection alive.
    ///
    /// # Errors
    ///
    /// Network or protocol errors.
    pub async fn noop(&mut self) -> Result<(), Error> {
        self.run_simple(&command::Noop).await.map(|_| ())
    }
}
