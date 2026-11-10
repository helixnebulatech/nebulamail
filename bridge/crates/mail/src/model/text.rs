// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The body text of a message without downloading all of it: pick the text
//! part from BODYSTRUCTURE, fetch only its first bytes, decode them.

use std::fmt::Write as _;

use nebula_imap::proto::response::{BodyStructure, SinglePart, TransferEncoding};

/// The part that holds the body text: its IMAP path (`[1]`, `[1, 2]`…),
/// and what is needed to decode it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextPart {
    pub(crate) path: Vec<u32>,
    /// `text/plain` or `text/html`.
    content_type: String,
    charset: Option<String>,
    encoding: &'static str,
}

fn encoding_name(encoding: &TransferEncoding) -> Option<&'static str> {
    Some(match encoding {
        TransferEncoding::SevenBit => "7bit",
        TransferEncoding::EightBit => "8bit",
        TransferEncoding::Binary => "binary",
        TransferEncoding::Base64 => "base64",
        TransferEncoding::QuotedPrintable => "quoted-printable",
        // Unknown encodings: the bytes can't be decoded.
        _ => return None,
    })
}

fn is_attachment(part: &SinglePart) -> bool {
    part.common
        .disposition
        .as_ref()
        .is_some_and(|d| d.kind == "attachment")
}

fn walk<'a>(
    structure: &'a BodyStructure,
    path: &mut Vec<u32>,
    plain: &mut Option<(Vec<u32>, &'a SinglePart)>,
    html: &mut Option<(Vec<u32>, &'a SinglePart)>,
) {
    match structure {
        BodyStructure::Multipart(multipart) => {
            for (index, child) in multipart.parts.iter().enumerate() {
                let Ok(number) = u32::try_from(index.saturating_add(1)) else {
                    return;
                };
                path.push(number);
                walk(child, path, plain, html);
                path.pop();
            }
        }
        BodyStructure::Single(part) => {
            if part.common.media_type != "text" || is_attachment(part) {
                return;
            }
            let slot = match part.common.subtype.as_str() {
                "plain" => &mut *plain,
                "html" => &mut *html,
                _ => return,
            };
            if slot.is_none() {
                // A message that is not multipart has its body at `[1]`.
                let at = if path.is_empty() {
                    vec![1]
                } else {
                    path.clone()
                };
                *slot = Some((at, part));
            }
        }
        _ => {}
    }
}

/// The first `text/plain` part that is not an attachment, else the first
/// `text/html` one. Nested `message/rfc822` parts are not looked into.
pub(crate) fn text_part(structure: &BodyStructure) -> Option<TextPart> {
    let (mut plain, mut html) = (None, None);
    walk(structure, &mut Vec::new(), &mut plain, &mut html);
    let (path, part) = plain.or(html)?;
    Some(TextPart {
        path,
        content_type: format!("text/{}", part.common.subtype),
        charset: part
            .common
            .params
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("charset"))
            .map(|(_, value)| value.clone()),
        encoding: encoding_name(&part.encoding)?,
    })
}

/// Decodes the first bytes of a part (`truncated`: cut by the partial
/// fetch) into text; HTML becomes plain text.
pub(crate) fn decode(part: &TextPart, data: &[u8], truncated: bool) -> Option<String> {
    let data = if truncated {
        safe_end(data, part.encoding)
    } else {
        data
    };
    let mut header = format!("Content-Type: {}", part.content_type);
    if let Some(charset) = &part.charset {
        // The charset comes from the server; only a token can go here.
        if charset
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
        {
            let _ = write!(header, "; charset={charset}");
        }
    }
    let _ = write!(
        header,
        "\r\nContent-Transfer-Encoding: {}\r\n\r\n",
        part.encoding
    );
    let mut message = header.into_bytes();
    message.extend_from_slice(data);
    let parsed = super::parsed::ParsedMessage::parse(&message)?;
    let text = parsed.message().body_text(0)?;
    let mut text = text.into_owned();
    if truncated {
        // A character cut in two (in any charset) decodes as U+FFFD.
        let kept = text.trim_end_matches('\u{FFFD}').len();
        text.truncate(kept);
    }
    Some(text)
}

/// The start of a cut part that decodes cleanly. A cut in the middle of a
/// base64 group or a `=XX` escape would garble the end: stop after the
/// last full line (with its `\n`, so a soft break `=\r\n` stays whole).
/// A part that is one long line is cut by its encoding instead of being
/// dropped.
fn safe_end<'a>(data: &'a [u8], encoding: &str) -> &'a [u8] {
    if let Some(pos) = data.iter().rposition(|b| *b == b'\n') {
        return data.get(..=pos).unwrap_or_default();
    }
    let end = match encoding {
        "base64" => {
            // Up to the last symbol that closes a group of four.
            let mut symbols = 0_usize;
            let mut end = 0;
            for (index, byte) in data.iter().enumerate() {
                if byte.is_ascii_alphanumeric() || b"+/=".contains(byte) {
                    symbols = symbols.saturating_add(1);
                    if symbols.is_multiple_of(4) {
                        end = index.saturating_add(1);
                    }
                }
            }
            end
        }
        "quoted-printable" => {
            // Drops a final `=` or `=X` (an escape cut in two).
            let tail = data.len().saturating_sub(2);
            data.iter()
                .skip(tail)
                .position(|b| *b == b'=')
                .map_or(data.len(), |at| tail.saturating_add(at))
        }
        _ => data.len(),
    };
    data.get(..end).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use nebula_imap::proto::response::{Disposition, Multipart, PartCommon};

    use super::*;

    fn single(subtype: &str, encoding: TransferEncoding, attachment: bool) -> BodyStructure {
        BodyStructure::Single(Box::new(SinglePart {
            common: PartCommon {
                media_type: if subtype == "png" { "image" } else { "text" }.to_owned(),
                subtype: subtype.to_owned(),
                params: vec![("charset".to_owned(), "iso-8859-1".to_owned())],
                disposition: attachment.then(|| Disposition {
                    kind: "attachment".to_owned(),
                    params: Vec::new(),
                }),
                language: Vec::new(),
                location: None,
            },
            id: None,
            description: None,
            md5: None,
            encoding,
            size: 10,
            lines: None,
            message: None,
        }))
    }

    fn multipart(subtype: &str, parts: Vec<BodyStructure>) -> BodyStructure {
        BodyStructure::Multipart(Multipart {
            common: PartCommon {
                media_type: "multipart".to_owned(),
                subtype: subtype.to_owned(),
                params: Vec::new(),
                disposition: None,
                language: Vec::new(),
                location: None,
            },
            parts,
        })
    }

    #[test]
    fn picks_the_text_part() {
        let simple = single("plain", TransferEncoding::SevenBit, false);
        assert_eq!(text_part(&simple).unwrap().path, [1]);

        let mixed = multipart(
            "mixed",
            vec![
                multipart(
                    "alternative",
                    vec![
                        single("html", TransferEncoding::QuotedPrintable, false),
                        single("plain", TransferEncoding::Base64, false),
                    ],
                ),
                single("plain", TransferEncoding::SevenBit, true),
                single("png", TransferEncoding::Base64, true),
            ],
        );
        let part = text_part(&mixed).unwrap();
        assert_eq!(part.path, [1, 2]);
        assert_eq!(part.encoding, "base64");

        let html_only = multipart(
            "mixed",
            vec![
                single("png", TransferEncoding::Base64, false),
                single("html", TransferEncoding::SevenBit, false),
            ],
        );
        assert_eq!(text_part(&html_only).unwrap().path, [2]);

        let attachments = multipart(
            "mixed",
            vec![single("plain", TransferEncoding::SevenBit, true)],
        );
        assert_eq!(text_part(&attachments), None);
        let unknown = single("plain", TransferEncoding::Other("x-uuencode".into()), false);
        assert_eq!(text_part(&unknown), None);
    }

    #[test]
    fn decodes_partial_data() {
        let part = |encoding, subtype: &str, charset: Option<&str>| TextPart {
            path: vec![1],
            content_type: format!("text/{subtype}"),
            charset: charset.map(str::to_owned),
            encoding,
        };
        // "Hola mundo\r\n" twice, base64 in two lines, the second one cut.
        let data = b"SG9sYSBtdW5kbw0K\r\nSG9sYSBtdW5k";
        let text = decode(&part("base64", "plain", None), data, true).unwrap();
        assert_eq!(text.trim(), "Hola mundo");

        let latin1 = decode(
            &part("quoted-printable", "plain", Some("iso-8859-1")),
            b"Ma=F1ana\r\n",
            false,
        )
        .unwrap();
        assert_eq!(latin1.trim(), "Mañana");

        let html = decode(
            &part("7bit", "html", None),
            b"<p>Hola <b>mundo</b></p>",
            false,
        )
        .unwrap();
        assert!(html.contains("Hola") && !html.contains('<'), "{html:?}");

        // One long line cut in the middle still gives its start.
        let long = decode(
            &part("8bit", "plain", None),
            "Año nuevo ñ".as_bytes().get(..12).unwrap(),
            true,
        )
        .unwrap();
        assert_eq!(long, "Año nuevo ");
        let base64 = decode(&part("base64", "plain", None), b"SG9sYSBtdW5kbw", true).unwrap();
        assert_eq!(base64, "Hola mund");
        let qp = decode(
            &part("quoted-printable", "plain", Some("utf-8")),
            b"Ma=C3=B1ana =C",
            true,
        )
        .unwrap();
        assert_eq!(qp, "Mañana");
        // A cut right after a soft line break does not end in `=`.
        let soft = decode(
            &part("quoted-printable", "plain", None),
            b"Hola =\r\nmun",
            true,
        )
        .unwrap();
        assert_eq!(soft, "Hola ");

        // A hostile charset can't add header lines.
        let evil = decode(
            &part("7bit", "plain", Some("utf-8\r\nContent-Type: text/html")),
            b"<b>x</b>",
            false,
        )
        .unwrap();
        assert_eq!(evil.trim(), "<b>x</b>");
    }
}
