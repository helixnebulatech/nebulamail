// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `[ALERT]` texts (RFC 3501 §7.1, RFC 9051 §7.1): "The human-readable
//! text contains a special alert that MUST be presented to the user in a
//! fashion that calls the user's attention to the message." The client
//! hands each one to its [`AlertSink`]; the caller shows them.

use std::fmt;
use std::sync::Arc;

/// The longest alert kept, in characters: longer ones are cut (with `…`).
pub const MAX_ALERT_CHARS: usize = 500;

/// The text of an `[ALERT]`, made safe to show: untrusted server text
/// with control and invisible formatting characters (line breaks, bidi
/// overrides…) turned into spaces, runs of spaces collapsed, at most
/// [`MAX_ALERT_CHARS`] characters, never empty. Still plain text: whoever
/// shows it must escape it for its medium (HTML…).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ServerAlert(Box<str>);

impl ServerAlert {
    /// Cleans `text`; `None` if nothing visible is left.
    #[must_use]
    pub fn new(text: &str) -> Option<Self> {
        let mut out = String::with_capacity(text.len().min(MAX_ALERT_CHARS * 4));
        let mut chars = 0_usize;
        let mut pending_space = false;
        for c in text.chars() {
            if is_hidden(c) || c.is_whitespace() {
                pending_space = !out.is_empty();
                continue;
            }
            let needed = usize::from(pending_space) + 1;
            if chars + needed > MAX_ALERT_CHARS {
                // Room for the ellipsis: drop the last character if full.
                if chars == MAX_ALERT_CHARS {
                    out.pop();
                }
                out.push('…');
                return Some(Self(out.into()));
            }
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.push(c);
            chars += needed;
        }
        (!out.is_empty()).then(|| Self(out.into()))
    }

    /// The cleaned text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Controls (C0, DEL, C1) and invisible formatting characters that could
/// hide or reorder text: bidi embeddings, overrides and isolates, zero
/// width characters, line and paragraph separators, BOM.
fn is_hidden(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{200B}'..='\u{200F}'
                | '\u{2028}'..='\u{202E}'
                | '\u{2060}'..='\u{2069}'
                | '\u{FEFF}'
                | '\u{061C}'
        )
}

impl fmt::Debug for ServerAlert {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Server text may hold personal data: only its size is printed.
        write!(f, "ServerAlert({} bytes)", self.0.len())
    }
}

impl fmt::Display for ServerAlert {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Receives the `[ALERT]`s of a connection as they arrive: in untagged
/// and tagged status responses and in the greeting after TLS.
pub trait AlertSink: Send + Sync {
    /// One alert.
    fn alert(&self, alert: ServerAlert);
}

/// Where a client sends its alerts.
#[derive(Clone, Default)]
pub(crate) struct Alerts {
    sink: Option<Arc<dyn AlertSink>>,
    /// Kept until a sink is set (the greeting comes before), at most
    /// [`MAX_PENDING`].
    pending: Vec<ServerAlert>,
}

const MAX_PENDING: usize = 4;

impl Alerts {
    pub(crate) fn push(&mut self, text: &str) {
        let Some(alert) = ServerAlert::new(text) else {
            return;
        };
        tracing::info!(len = alert.as_str().len(), "IMAP server alert");
        match &self.sink {
            Some(sink) => sink.alert(alert),
            None if self.pending.len() < MAX_PENDING => self.pending.push(alert),
            None => {}
        }
    }

    pub(crate) fn set_sink(&mut self, sink: Arc<dyn AlertSink>) {
        for alert in self.pending.drain(..) {
            sink.alert(alert);
        }
        self.sink = Some(sink);
    }
}

impl fmt::Debug for Alerts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Alerts")
            .field("sink", &self.sink.is_some())
            .field("pending", &self.pending.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_untrusted_text() {
        assert_eq!(
            ServerAlert::new("  Quota\r\n at\t95%\u{202E}!  ")
                .unwrap()
                .as_str(),
            "Quota at 95% !"
        );
        assert!(ServerAlert::new(" \u{200B}\r\n ").is_none());
        assert!(ServerAlert::new("").is_none());
        let long = "a".repeat(MAX_ALERT_CHARS + 10);
        let cut = ServerAlert::new(&long).unwrap();
        assert_eq!(cut.as_str().chars().count(), MAX_ALERT_CHARS);
        assert!(cut.as_str().ends_with('…'));
        let exact = "é".repeat(MAX_ALERT_CHARS);
        assert_eq!(ServerAlert::new(&exact).unwrap().as_str(), exact);
        assert_eq!(
            format!("{cut:?}"),
            format!("ServerAlert({} bytes)", cut.as_str().len())
        );
    }

    #[test]
    fn pending_until_a_sink_is_set() {
        use std::sync::Mutex;
        #[derive(Default)]
        struct Collect(Mutex<Vec<String>>);
        impl AlertSink for Collect {
            fn alert(&self, alert: ServerAlert) {
                self.0.lock().unwrap().push(alert.as_str().to_owned());
            }
        }
        let mut alerts = Alerts::default();
        for n in 0..6 {
            alerts.push(&format!("early {n}"));
        }
        let sink = Arc::new(Collect::default());
        alerts.set_sink(sink.clone());
        alerts.push("late");
        assert_eq!(
            *sink.0.lock().unwrap(),
            ["early 0", "early 1", "early 2", "early 3", "late"]
        );
    }
}
