// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Email previews (RFC 8621 `preview`: up to 256 characters of body text).

/// Longest preview, in characters (RFC 8621 §4.1.4).
pub(crate) const MAX_PREVIEW_CHARS: usize = 256;

/// Whitespace collapsed to single spaces, trimmed, cut to 256 characters.
pub(crate) fn clean(text: &str) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    joined.chars().take(MAX_PREVIEW_CHARS).collect()
}

/// The preview of a whole message: the first text part (or the HTML part
/// as text). `None` if the message has no text at all.
#[must_use]
pub fn from_message(raw: &[u8]) -> Option<String> {
    let message = super::parsed::ParsedMessage::parse(raw)?;
    // A generous cut first (bytes, before collapsing whitespace).
    let text = message.message().body_preview(4096)?;
    let preview = clean(&text);
    (!preview.is_empty()).then_some(preview)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_and_cuts() {
        assert_eq!(clean("  Hola,\r\n\r\n  mundo\t!  "), "Hola, mundo !");
        let long = "palabra ".repeat(100);
        let preview = clean(&long);
        assert_eq!(preview.chars().count(), MAX_PREVIEW_CHARS);
        assert!(clean("ñ".repeat(300).as_str()).chars().all(|c| c == 'ñ'));
        assert_eq!(clean(""), "");
    }

    #[test]
    fn previews_text_and_html() {
        let text = b"Subject: x\r\nContent-Type: text/plain\r\n\r\nHola\r\nmundo\r\n";
        assert_eq!(from_message(text).as_deref(), Some("Hola mundo"));
        let html = b"Subject: x\r\nContent-Type: text/html\r\n\r\n<p>Hola <b>mundo</b></p>";
        assert_eq!(from_message(html).as_deref(), Some("Hola mundo"));
        let empty = b"Subject: x\r\n\r\n";
        assert_eq!(from_message(empty), None);
    }
}
