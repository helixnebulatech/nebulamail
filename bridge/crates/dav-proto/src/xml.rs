// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Reading XML strictly, and writing it with our own escaping.

use roxmltree::{Document, Node, ParsingOptions};

use crate::{EncodeError, Namespace, ParseError};

/// Limits for one XML body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XmlLimits {
    /// Largest body, in bytes, checked before parsing
    /// (16 MiB for a multistatus).
    pub max_bytes: usize,
    /// Most XML nodes (elements, text, comments, processing
    /// instructions; not attributes): bounds the memory of the parsed
    /// tree (see [`MAX_ATTRIBUTES_PER_ELEMENT`] and
    /// [`MAX_NAMESPACES_IN_SCOPE`] for the other limits of the markup).
    ///
    /// Known limit: roxmltree 0.21.2 reserves, before parsing, one node
    /// per `<` and one attribute per `=` of the whole text (values, CDATA
    /// and comments included). That is not tied to `max_nodes`: `=` is
    /// never escaped, so legitimate `calendar-data`, `address-data` or
    /// `calendar-timezone` (quoted-printable vCards, iCalendar
    /// parameters) can hold millions, and refusing the body for them
    /// would stop a whole collection or home. The reservation is bounded
    /// by `max_bytes` instead (at most one per byte) and is virtual
    /// memory: only the slots of nodes really parsed (at most
    /// `max_nodes`) are written.
    pub max_nodes: u32,
    /// Deepest element nesting. A multistatus needs about 8.
    pub max_depth: usize,
    /// Longest text of one property (`calendar-data`, `address-data`,
    /// `calendar-timezone`…): 4 MiB, the vCard limit.
    pub max_value_bytes: usize,
}

impl Default for XmlLimits {
    fn default() -> Self {
        Self {
            max_bytes: 16 * 1024 * 1024,
            max_nodes: 1_000_000,
            max_depth: 32,
            max_value_bytes: 4 * 1024 * 1024,
        }
    }
}

impl XmlLimits {
    /// Limits for small bodies (`DAV:error` of a failed request).
    #[must_use]
    pub const fn small() -> Self {
        Self {
            max_bytes: 64 * 1024,
            max_nodes: 10_000,
            max_depth: 16,
            max_value_bytes: 16 * 1024,
        }
    }
}

/// Most attributes on one element, `xmlns` declarations included.
/// roxmltree compares every attribute with the earlier ones of its
/// element (quadratic); real DAV bodies have fewer than 15 on the root.
pub const MAX_ATTRIBUTES_PER_ELEMENT: usize = 32;

/// Most `xmlns` declarations in scope at once (on the open elements).
/// roxmltree copies the declarations in scope into every element that
/// declares one, checking each for a duplicate (quadratic).
pub const MAX_NAMESPACES_IN_SCOPE: usize = 32;

/// Skips `<?…?>` and `<!--…-->` from the start of `text`.
fn skip_misc(text: &str) -> Option<&str> {
    if let Some(rest) = text.strip_prefix("<?") {
        return rest.find("?>").and_then(|end| rest.get(end + 2..));
    }
    if let Some(rest) = text.strip_prefix("<!--") {
        return rest.find("-->").and_then(|end| rest.get(end + 3..));
    }
    None
}

/// The `encoding` of an XML declaration, if any.
fn declared_encoding(declaration: &str) -> Option<&str> {
    let at = declaration.find("encoding")?;
    let rest = declaration.get(at + "encoding".len()..)?.trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let value = rest.get(1..)?;
    value.find(quote).and_then(|end| value.get(..end))
}

/// The prolog, before the root element: whitespace, the XML declaration,
/// comments and processing instructions. A DTD (`<!DOCTYPE`) can only be
/// here, so this finds every DTD, also the empty ones that roxmltree's
/// `allow_dtd: false` lets through.
fn check_prolog(text: &str) -> Result<(), ParseError> {
    if let Some(rest) = text.strip_prefix("<?xml")
        && rest.starts_with(|c: char| c.is_ascii_whitespace())
    {
        let declaration = rest.split("?>").next().unwrap_or_default();
        if let Some(encoding) = declared_encoding(declaration)
            && !encoding.eq_ignore_ascii_case("utf-8")
            && !encoding.eq_ignore_ascii_case("utf8")
        {
            return Err(ParseError::Encoding);
        }
    }
    let mut rest = text;
    loop {
        rest = rest.trim_start_matches([' ', '\t', '\r', '\n']);
        if rest.starts_with("<!") && !rest.starts_with("<!--") {
            return Err(ParseError::Dtd);
        }
        match skip_misc(rest) {
            Some(after) => rest = after,
            None => return Ok(()),
        }
    }
}

/// The position of `needle` in `bytes` from `from`, if any.
fn find(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    bytes
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|at| from + at)
}

const fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
}

/// What one start tag holds, as far as [`check_markup`] cares.
enum Tag {
    /// `<name …>` with this many `xmlns` declarations; `end` is after `>`.
    Open { namespaces: usize, end: usize },
    /// `<name …/>`.
    Empty { namespaces: usize, end: usize },
    /// Something the pre-pass does not read; roxmltree decides. `end` is
    /// where to look for the next `<`.
    Other { end: usize },
}

/// Reads the start tag at `start` (a `<` followed by a name), counting
/// attributes and `xmlns` declarations.
fn read_start_tag(bytes: &[u8], start: usize) -> Result<Tag, ParseError> {
    let byte = |at: usize| bytes.get(at).copied();
    let mut at = start + 1;
    while byte(at).is_some_and(|b| !is_space(b) && !matches!(b, b'/' | b'>' | b'<' | b'=')) {
        at += 1;
    }
    if at == start + 1 {
        return Ok(Tag::Other { end: at });
    }
    let mut attributes = 0_usize;
    let mut namespaces = 0_usize;
    loop {
        while byte(at).is_some_and(is_space) {
            at += 1;
        }
        match byte(at) {
            Some(b'>') => {
                return Ok(Tag::Open {
                    namespaces,
                    end: at + 1,
                });
            }
            Some(b'/') if byte(at + 1) == Some(b'>') => {
                return Ok(Tag::Empty {
                    namespaces,
                    end: at + 2,
                });
            }
            _ => {}
        }
        let name_start = at;
        while byte(at).is_some_and(|b| !is_space(b) && !matches!(b, b'/' | b'>' | b'<' | b'=')) {
            at += 1;
        }
        let name = bytes.get(name_start..at).unwrap_or_default();
        while byte(at).is_some_and(is_space) {
            at += 1;
        }
        if name.is_empty() || byte(at) != Some(b'=') {
            return Ok(Tag::Other {
                end: at.max(start + 1),
            });
        }
        at += 1;
        while byte(at).is_some_and(is_space) {
            at += 1;
        }
        let Some(quote) = byte(at).filter(|b| matches!(b, b'"' | b'\'')) else {
            return Ok(Tag::Other { end: at });
        };
        let Some(close) = find(bytes, at + 1, &[quote]) else {
            // An unclosed value: roxmltree stops here with an error.
            return Ok(Tag::Other { end: bytes.len() });
        };
        at = close + 1;
        attributes += 1;
        if attributes > MAX_ATTRIBUTES_PER_ELEMENT {
            return Err(ParseError::TooManyAttributes);
        }
        if name == b"xmlns" || name.starts_with(b"xmlns:") {
            namespaces += 1;
        }
    }
}

/// A linear pass over the markup before roxmltree, which is quadratic in
/// the attributes of one element and in the namespaces in scope, and
/// checks depth only on the finished tree. Limits only: anything it does
/// not read (a stray `<`, a broken tag) is left to roxmltree, which then
/// fails; it never refuses well-formed XML within the limits. Memory is a
/// stack bounded by `max_depth`.
pub(crate) fn check_markup(text: &str, limits: &XmlLimits) -> Result<(), ParseError> {
    let bytes = text.as_bytes();
    // No global count of `<` or `=`: they may be in values (see
    // `XmlLimits::max_nodes`).
    // Declarations of each open element, and their sum.
    let mut open: Vec<usize> = Vec::with_capacity(limits.max_depth.min(64));
    let mut in_scope = 0_usize;
    let mut at = 0;
    while let Some(start) = find(bytes, at, b"<") {
        let rest = bytes.get(start..).unwrap_or_default();
        let skip_to = |end: &[u8], from: usize| find(bytes, from, end).map(|e| e + end.len());
        let next = if rest.starts_with(b"<!--") {
            skip_to(b"-->", start + 4)
        } else if rest.starts_with(b"<![CDATA[") {
            skip_to(b"]]>", start + 9)
        } else if rest.starts_with(b"<?") {
            skip_to(b"?>", start + 2)
        } else if rest.starts_with(b"<!") {
            Some(start + 2)
        } else if rest.starts_with(b"</") {
            if let Some(namespaces) = open.pop() {
                in_scope = in_scope.saturating_sub(namespaces);
            }
            skip_to(b">", start + 2)
        } else {
            match read_start_tag(bytes, start)? {
                Tag::Open { namespaces, end } => {
                    if open.len() >= limits.max_depth {
                        return Err(ParseError::TooDeep);
                    }
                    in_scope += namespaces;
                    if in_scope > MAX_NAMESPACES_IN_SCOPE {
                        return Err(ParseError::TooManyNamespaces);
                    }
                    open.push(namespaces);
                    Some(end)
                }
                Tag::Empty { namespaces, end } => {
                    if open.len() >= limits.max_depth {
                        return Err(ParseError::TooDeep);
                    }
                    if in_scope + namespaces > MAX_NAMESPACES_IN_SCOPE {
                        return Err(ParseError::TooManyNamespaces);
                    }
                    Some(end)
                }
                Tag::Other { end } => Some(end),
            }
        };
        // Unclosed comment, CDATA or tag: roxmltree fails there.
        let Some(next) = next else {
            return Ok(());
        };
        at = next;
    }
    Ok(())
}

/// Parses a body with every check: size before parsing,
/// UTF-8, no DTD, a linear pre-pass on the markup ([`check_markup`]:
/// attributes, namespaces and depth), node
/// limit, then depth again on the tree (roxmltree 0.21.2 parses nested
/// elements without recursion).
pub(crate) fn parse_document<'a>(
    body: &'a [u8],
    limits: &XmlLimits,
) -> Result<Document<'a>, ParseError> {
    if body.len() > limits.max_bytes {
        return Err(ParseError::TooLarge);
    }
    let text = std::str::from_utf8(body).map_err(|_| ParseError::Encoding)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    check_prolog(text)?;
    check_markup(text, limits)?;
    let options = ParsingOptions {
        allow_dtd: false,
        nodes_limit: limits.max_nodes,
        ..ParsingOptions::default()
    };
    let document = Document::parse_with_options(text, options).map_err(|error| match error {
        roxmltree::Error::DtdDetected | roxmltree::Error::EntityReferenceLoop(_) => ParseError::Dtd,
        roxmltree::Error::NodesLimitReached => ParseError::TooManyNodes,
        other => {
            // The position only: the message may quote names from the body.
            let position = other.pos();
            ParseError::Xml {
                row: position.row,
                col: position.col,
            }
        }
    })?;
    check_depth(&document, limits.max_depth)?;
    Ok(document)
}

/// Depth of every element in one pass, without recursion.
fn check_depth(document: &Document<'_>, max_depth: usize) -> Result<(), ParseError> {
    let mut depths: Vec<usize> = vec![0; document.descendants().count()];
    for node in document.descendants() {
        let index = node.id().get_usize();
        let depth = match node.parent() {
            Some(parent) => depths.get(parent.id().get_usize()).copied().unwrap_or(0) + 1,
            None => 0,
        };
        if depth > max_depth {
            return Err(ParseError::TooDeep);
        }
        if let Some(slot) = depths.get_mut(index) {
            *slot = depth;
        }
    }
    Ok(())
}

/// The element is `namespace:local` (by URI, never by prefix).
pub(crate) fn is(node: Node<'_, '_>, namespace: Namespace, local: &str) -> bool {
    node.is_element()
        && node.tag_name().name() == local
        && node.tag_name().namespace() == Some(namespace.uri())
}

pub(crate) fn element_children<'a, 'input>(
    node: Node<'a, 'input>,
) -> impl Iterator<Item = Node<'a, 'input>> {
    node.children().filter(Node::is_element)
}

pub(crate) fn child<'a, 'input>(
    node: Node<'a, 'input>,
    namespace: Namespace,
    local: &str,
) -> Option<Node<'a, 'input>> {
    node.children().find(|c| is(*c, namespace, local))
}

/// The text of an element that has only text (and CDATA) inside.
/// `None` if it has child elements; an error past `max` bytes.
pub(crate) fn text_of(node: Node<'_, '_>, max: usize) -> Result<Option<String>, ParseError> {
    let mut text = String::new();
    for c in node.children() {
        if c.is_element() {
            return Ok(None);
        }
        if let Some(part) = c.text().filter(|_| c.is_text()) {
            if text.len() + part.len() > max {
                return Err(ParseError::ValueTooLarge);
            }
            text.push_str(part);
        }
    }
    Ok(Some(text))
}

/// XML parsers turn CRLF into LF (XML 1.0 §2.11); iCalendar and vCard need
/// CRLF back (RFC 4791 §9.6, as libdav does). A lone CR stays as it is.
pub(crate) fn crlf(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 32);
    let mut previous = '\0';
    for c in text.chars() {
        if c == '\n' && previous != '\r' {
            out.push('\r');
        }
        out.push(c);
        previous = c;
    }
    out
}

/// Characters XML 1.0 can carry (§2.2): tab, LF, CR and everything from
/// U+0020 except U+FFFE and U+FFFF (Rust strings have no surrogates).
const fn is_xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{FFFD}' | '\u{10000}'..)
}

/// Writes an XML body. Element names come only from our own constants;
/// text and attribute values are escaped and checked.
#[derive(Debug)]
pub(crate) struct Writer {
    out: String,
    open: Vec<(Namespace, &'static str)>,
    /// Where the root's start tag ends.
    root_end: usize,
}

impl Writer {
    /// Starts a document with `root`, declaring every namespace.
    pub(crate) fn new(namespace: Namespace, local: &'static str) -> Self {
        let mut out = String::with_capacity(512);
        out.push_str(r#"<?xml version="1.0" encoding="utf-8"?>"#);
        out.push('<');
        push_name(&mut out, namespace, local);
        for ns in Namespace::ALL {
            out.push_str(" xmlns:");
            out.push_str(ns.prefix());
            out.push_str("=\"");
            out.push_str(ns.uri());
            out.push('"');
        }
        out.push('>');
        Self {
            root_end: out.len(),
            out,
            open: vec![(namespace, local)],
        }
    }

    pub(crate) fn start(&mut self, namespace: Namespace, local: &'static str) -> &mut Self {
        self.out.push('<');
        push_name(&mut self.out, namespace, local);
        self.out.push('>');
        self.open.push((namespace, local));
        self
    }

    /// `<name attr="value"/>`.
    pub(crate) fn empty_with(
        &mut self,
        namespace: Namespace,
        local: &'static str,
        attributes: &[(&'static str, &str)],
    ) -> Result<&mut Self, EncodeError> {
        self.out.push('<');
        push_name(&mut self.out, namespace, local);
        for (name, value) in attributes {
            self.out.push(' ');
            self.out.push_str(name);
            self.out.push_str("=\"");
            escape_into(&mut self.out, value)?;
            self.out.push('"');
        }
        self.out.push_str("/>");
        Ok(self)
    }

    /// `<name/>` for a checked name from elsewhere (a privilege written
    /// back): our prefix for our namespaces, else declared on the element.
    pub(crate) fn empty_qualified(
        &mut self,
        name: &crate::QualifiedName,
    ) -> Result<&mut Self, EncodeError> {
        self.out.push('<');
        if let Some(namespace) = Namespace::from_uri(name.namespace()) {
            push_name(&mut self.out, namespace, name.local());
        } else {
            self.out.push_str("X:");
            self.out.push_str(name.local());
            self.out.push_str(" xmlns:X=\"");
            escape_into(&mut self.out, name.namespace())?;
            self.out.push('"');
        }
        self.out.push_str("/>");
        Ok(self)
    }

    pub(crate) fn empty(&mut self, namespace: Namespace, local: &'static str) -> &mut Self {
        self.out.push('<');
        push_name(&mut self.out, namespace, local);
        self.out.push_str("/>");
        self
    }

    /// `<name>text</name>`.
    pub(crate) fn text_element(
        &mut self,
        namespace: Namespace,
        local: &'static str,
        text: &str,
    ) -> Result<&mut Self, EncodeError> {
        self.out.push('<');
        push_name(&mut self.out, namespace, local);
        self.out.push('>');
        escape_into(&mut self.out, text)?;
        self.out.push_str("</");
        push_name(&mut self.out, namespace, local);
        self.out.push('>');
        Ok(self)
    }

    pub(crate) fn end(&mut self) -> &mut Self {
        if let Some((namespace, local)) = self.open.pop() {
            self.out.push_str("</");
            push_name(&mut self.out, namespace, local);
            self.out.push('>');
        }
        self
    }

    /// The document as `<root …/>`, if nothing was written inside the
    /// root (some servers want an empty element, not `<root></root>`);
    /// else as [`Self::finish`].
    pub(crate) fn finish_empty(mut self) -> String {
        if self.out.len() == self.root_end {
            self.out.pop();
            self.out.push_str("/>");
            return self.out;
        }
        self.finish()
    }

    pub(crate) fn finish(mut self) -> String {
        while !self.open.is_empty() {
            self.end();
        }
        self.out
    }
}

fn push_name(out: &mut String, namespace: Namespace, local: &str) {
    out.push_str(namespace.prefix());
    out.push(':');
    out.push_str(local);
}

/// Escapes text or an attribute value. `>` and both quotes are escaped
/// too, so the result is safe in either place; CR is written as `&#13;`
/// so that it survives the XML line-end normalisation.
pub(crate) fn escape_into(out: &mut String, text: &str) -> Result<(), EncodeError> {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\r' => out.push_str("&#13;"),
            c if is_xml_char(c) => out.push(c),
            _ => return Err(EncodeError::XmlCharacter),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{MAX_ATTRIBUTES_PER_ELEMENT, XmlLimits, crlf, escape_into, parse_document};
    use crate::{EncodeError, ParseError};

    /// `piece(i)` for each `i` of `range`, one after the other.
    fn joined(range: std::ops::Range<usize>, piece: impl Fn(usize) -> String) -> String {
        let mut out = String::new();
        for i in range {
            out.push_str(&piece(i));
        }
        out
    }

    fn parse(text: &str) -> Result<(), ParseError> {
        parse_document(text.as_bytes(), &XmlLimits::default()).map(|_| ())
    }

    #[test]
    fn every_dtd_is_refused() {
        let cases = [
            r#"<!DOCTYPE d><d:multistatus xmlns:d="DAV:"/>"#,
            r#"<?xml version="1.0"?><!DOCTYPE d []><d/>"#,
            "<?xml version=\"1.0\"?>\n<!-- hi -->\n<!DOCTYPE d SYSTEM \"file:///etc/passwd\"><d/>",
            r"<!doctype d><d/>",
            r#"<?xml version="1.0"?><!DOCTYPE lolz [<!ENTITY lol "lol"><!ENTITY lol2 "&lol;&lol;">]><d>&lol2;</d>"#,
            "\u{feff}<!DOCTYPE d><d/>",
        ];
        for case in cases {
            assert_eq!(parse(case), Err(ParseError::Dtd), "{case}");
        }
        // Undeclared entities are plain errors.
        assert!(matches!(parse("<d>&xxe;</d>"), Err(ParseError::Xml { .. })));
        // A DOCTYPE inside CDATA is just text.
        assert_eq!(parse("<d><![CDATA[<!DOCTYPE x>]]></d>"), Ok(()));
    }

    #[test]
    fn utf8_only() {
        assert_eq!(
            parse_document(b"<d>\xff</d>", &XmlLimits::default()).map(|_| ()),
            Err(ParseError::Encoding)
        );
        assert_eq!(
            parse(r#"<?xml version="1.0" encoding="ISO-8859-1"?><d/>"#),
            Err(ParseError::Encoding)
        );
        assert_eq!(
            parse(r"<?xml version='1.0' encoding='UTF-16'?><d/>"),
            Err(ParseError::Encoding)
        );
        assert_eq!(
            parse(r#"<?xml version="1.0" encoding="UTF-8"?><d/>"#),
            Ok(())
        );
        assert_eq!(parse("\u{feff}<d/>"), Ok(()));
    }

    #[test]
    fn limits() {
        let small = XmlLimits {
            max_bytes: 10,
            ..XmlLimits::default()
        };
        assert_eq!(
            parse_document(b"<d>0123456789</d>", &small).map(|_| ()),
            Err(ParseError::TooLarge)
        );
        let deep = format!("{}{}", "<a>".repeat(40), "</a>".repeat(40));
        assert_eq!(parse(&deep), Err(ParseError::TooDeep));
        let ok = format!("{}{}", "<a>".repeat(30), "</a>".repeat(30));
        assert_eq!(parse(&ok), Ok(()));
        let few = XmlLimits {
            max_nodes: 100,
            ..XmlLimits::default()
        };
        let wide = format!("<a>{}</a>", "<b/>".repeat(200));
        assert_eq!(
            parse_document(wide.as_bytes(), &few).map(|_| ()),
            Err(ParseError::TooManyNodes)
        );
    }

    #[test]
    fn very_deep_nesting_does_not_overflow_the_stack() {
        let deep = format!("{}{}", "<a>".repeat(200_000), "</a>".repeat(200_000));
        assert_eq!(parse(&deep), Err(ParseError::TooDeep));
    }

    #[test]
    fn too_many_attributes() {
        let attributes: String = joined(0..30_000, |i| format!(" xmlns:a{i}=\"u\""));
        assert_eq!(
            parse(&format!("<d{attributes}/>")),
            Err(ParseError::TooManyAttributes)
        );
        let attributes: String = joined(0..100, |i| format!(" a{i}=\"\""));
        assert_eq!(
            parse(&format!("<d><e{attributes}/></d>")),
            Err(ParseError::TooManyAttributes)
        );
        let attributes: String = joined(0..MAX_ATTRIBUTES_PER_ELEMENT, |i| format!(" a{i} = '>'"));
        assert_eq!(parse(&format!("<d><e{attributes}/></d>")), Ok(()));
    }

    #[test]
    fn too_many_namespaces_in_scope() {
        let root: String = joined(0..20, |i| format!(" xmlns:r{i}=\"u{i}\""));
        // Siblings each declare one: 21 in scope at once, fine.
        let siblings = "<e xmlns:q=\"v\"/>".repeat(100);
        assert_eq!(parse(&format!("<d{root}>{siblings}</d>")), Ok(()));
        // Nested, the sum passes 32.
        let nested: String = joined(0..13, |i| format!("<e xmlns:q{i}=\"v\">"));
        let closing = "</e>".repeat(13);
        assert_eq!(
            parse(&format!("<d{root}>{nested}{closing}</d>")),
            Err(ParseError::TooManyNamespaces)
        );
        let nested: String = joined(0..12, |i| format!("<e xmlns:q{i}=\"v\">"));
        let closing = "</e>".repeat(12);
        assert_eq!(parse(&format!("<d{root}>{nested}{closing}</d>")), Ok(()));
    }

    #[test]
    fn namespace_bombs_stop_before_parsing() {
        // The same prefix again and again (roxmltree does not copy it).
        let same = "<a xmlns:b=\"\">".repeat(10_000);
        assert_eq!(parse(&same), Err(ParseError::TooDeep));
        // Different prefixes: what roxmltree copies grows with each level.
        let different: String = joined(0..10_000, |i| format!("<a xmlns:b{i}=\"\">"));
        let started = std::time::Instant::now();
        assert!(matches!(
            parse(&different),
            Err(ParseError::TooDeep | ParseError::TooManyNamespaces)
        ));
        let deep = XmlLimits {
            max_depth: 100_000,
            ..XmlLimits::default()
        };
        assert_eq!(
            parse_document(different.as_bytes(), &deep).map(|_| ()),
            Err(ParseError::TooManyNamespaces)
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn only_markup_counts() {
        let many: String = joined(0..100, |i| format!(" a{i}=\"1\""));
        let cases = [
            format!("<d><!-- <a{many}> --></d>"),
            format!("<d><![CDATA[<a{many}>]]></d>"),
            format!("<?pi <a{many}> ?><d/>"),
            r#"<d a="x>y/>z" b='"'><e c="/>"/></d>"#.to_owned(),
            "<d>a=b c=d &lt;e f=g&gt;</d>".to_owned(),
            format!("<d>{}</d>", "<e/>".repeat(1000)),
        ];
        for case in cases {
            assert_eq!(parse(&case), Ok(()), "{case}");
        }
    }

    #[test]
    fn signs_in_values_never_refuse_the_body() {
        // A value full of `=` or a CDATA full of `<`: far more than
        // `max_nodes`, but text, not markup.
        let few = XmlLimits {
            max_nodes: 100,
            ..XmlLimits::default()
        };
        let cases = [
            format!("<d><!--{}--></d>", "<".repeat(3000)),
            format!("<d><![CDATA[{}]]></d>", "<".repeat(3000)),
            format!("<d>{}</d>", "=".repeat(3000)),
            format!("<d a=\"{}\"/>", "=".repeat(3000)),
        ];
        for case in cases {
            assert_eq!(parse_document(case.as_bytes(), &few).map(|_| ()), Ok(()));
        }
        // The node limit still holds for markup.
        let wide = format!("<d>{}</d>", "<e/>".repeat(300));
        assert_eq!(
            parse_document(wide.as_bytes(), &few).map(|_| ()),
            Err(ParseError::TooManyNodes)
        );
    }

    #[test]
    fn unclosed_depth_is_refused_before_parsing() {
        let open = "<a>".repeat(1000);
        assert_eq!(parse(&open), Err(ParseError::TooDeep));
    }

    #[test]
    fn escaping() {
        let mut out = String::new();
        escape_into(&mut out, "a<b>&\"'\r\n\tc").unwrap();
        assert_eq!(out, "a&lt;b&gt;&amp;&quot;&apos;&#13;\n\tc");
        for bad in ["\u{0}", "\u{8}", "\u{1b}", "\u{fffe}", "\u{ffff}"] {
            assert_eq!(
                escape_into(&mut String::new(), bad),
                Err(EncodeError::XmlCharacter)
            );
        }
    }

    #[test]
    fn line_ends() {
        assert_eq!(crlf("a\nb\r\nc\rd\n"), "a\r\nb\r\nc\rd\r\n");
    }
}
