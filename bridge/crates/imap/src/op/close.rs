// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! CLOSE, UNSELECT and LOGOUT.

use nebula_imap_proto::command::{Access, Close, Logout, Unselect};
use nebula_imap_proto::response::{Condition, Untagged};
use nebula_imap_proto::types::Capability;

use crate::core::client::{Authenticated, Client, Selected, sealed::State};
use crate::core::connection::{StrayContinuation, expect_ok};
use crate::{Error, StateError};

impl Client<Selected> {
    /// `CLOSE`: deselects and **expunges** messages marked `\Deleted`
    /// (unless the mailbox is read-only).
    ///
    /// # Errors
    ///
    /// On NO the mailbox is still selected and comes back in the error.
    pub async fn close(mut self) -> Result<Client<Authenticated>, StateError<Client<Selected>>> {
        match self.run_simple(&Close).await {
            Ok(_) => Ok(self.into_state(Authenticated)),
            Err(error) => Err(StateError::new(error, self)),
        }
    }

    /// Deselects without expunging: `UNSELECT`, or `CLOSE` when the mailbox
    /// was opened read-only (then CLOSE expunges nothing).
    ///
    /// # Errors
    ///
    /// [`Error::Unsupported`] for a read-write mailbox on a server without
    /// UNSELECT; on NO the mailbox is still selected.
    pub async fn unselect(mut self) -> Result<Client<Authenticated>, StateError<Client<Selected>>> {
        let result = if self.session.has(&Capability::Unselect) {
            self.run_simple(&Unselect).await
        } else if self.state.mailbox.access == Access::ReadOnly {
            self.run_simple(&Close).await
        } else {
            Err(Error::Unsupported("UNSELECT"))
        };
        match result {
            Ok(_) => Ok(self.into_state(Authenticated)),
            Err(error) => Err(StateError::new(error, self)),
        }
    }
}

impl<S: State> Client<S> {
    /// `LOGOUT`: ends the session and closes the connection.
    ///
    /// # Errors
    ///
    /// Connection errors (the connection is gone either way).
    pub async fn logout(mut self) -> Result<(), Error> {
        let result = self
            .run_with(&Logout, StrayContinuation::Reject, |data| match data {
                // The BYE is expected here.
                Untagged::Status(status) if status.condition == Condition::Bye => None,
                other => Some(other),
            })
            .await;
        match result {
            Ok(tagged) => expect_ok("LOGOUT", tagged).map(|_| ()),
            // Some servers close the stream right after BYE.
            Err(Error::Bye(_) | Error::ConnectionClosed) => Ok(()),
            Err(error) => Err(error),
        }
    }
}
