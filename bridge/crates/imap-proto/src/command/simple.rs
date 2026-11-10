// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Commands without arguments.

use zeroize::Zeroizing;

use super::RawLine;
use super::command;

macro_rules! no_args {
    ($(#[$doc:meta])* $type:ident, $name:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
        pub struct $type;

        command!($type, $name, |_command, writer| {
            writer.raw($name.as_bytes());
            Ok(())
        });
    };
}

no_args!(
    /// `CAPABILITY`
    Capability,
    "CAPABILITY"
);
no_args!(
    /// `NOOP`: also polls for changes.
    Noop,
    "NOOP"
);
no_args!(
    /// `LOGOUT`
    Logout,
    "LOGOUT"
);
no_args!(
    /// `STARTTLS`
    StartTls,
    "STARTTLS"
);
no_args!(
    /// `NAMESPACE` (RFC 2342).
    Namespace,
    "NAMESPACE"
);
no_args!(
    /// `CLOSE`: deselects and expunges `\Deleted` messages.
    Close,
    "CLOSE"
);
no_args!(
    /// `UNSELECT` (RFC 3691): deselects without expunging.
    Unselect,
    "UNSELECT"
);
no_args!(
    /// `IDLE` (RFC 2177). After the server's `+`, send [`idle_done`] to stop.
    Idle,
    "IDLE"
);

/// The `DONE` line that ends IDLE.
#[must_use]
pub fn idle_done() -> RawLine {
    RawLine::new(Zeroizing::new(b"DONE\r\n".to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::test_util::basic;

    #[test]
    fn encodes() {
        assert_eq!(basic(&Capability), "A1 CAPABILITY\r\n");
        assert_eq!(basic(&Idle), "A1 IDLE\r\n");
        assert_eq!(basic(&Unselect), "A1 UNSELECT\r\n");
        assert_eq!(idle_done().bytes(), b"DONE\r\n");
    }
}
