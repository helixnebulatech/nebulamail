// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `SearchSnippet/get` (RFC 8621 §5): the subject and preview with the
//! search terms in `<mark>`. Everything else is HTML-escaped, so a subject
//! like `<img onerror=…>` comes out as text.
//!
//! Matching is a case-insensitive substring search of each term, like the
//! IMAP `SEARCH` that found the email. For body terms, the first bytes of
//! each email's text part are fetched (not the whole message, see
//! `MailAccount::body_texts`) and the snippet is the fragment around the
//! first match; if the text can't be had or doesn't match (the hit was in
//! an attachment, say), the stored preview is tried instead.

use nebula_jmap_proto::MethodResponse;
use nebula_jmap_proto::method::Filter;
use nebula_jmap_proto::object::email::EmailFilterCondition;
use nebula_jmap_proto::object::{
    SearchSnippet, SearchSnippetGetArguments, SearchSnippetGetResponse,
};

use std::collections::HashMap;
use std::num::NonZeroU32;

use super::query::MAX_TEXT_TERMS;
use crate::core::context::Ctx;
use crate::core::ids;
use crate::error::CallError;

/// Most distinct terms kept for the subject, and for the body. Every term
/// costs one pass over each text, so they are capped; the rest are
/// ignored (a snippet is best effort, and the search itself is capped by
/// `Filter::parse` and `Email/query`).
const MAX_SNIPPET_TERMS: usize = 32;
/// Longest term kept, in characters: a longer one can't fit in a snippet
/// ([`SNIPPET_CHARS`]).
const MAX_TERM_CHARS: usize = SNIPPET_CHARS;

/// Search terms for the subject and for the body, already [`fold`]ed,
/// without repeats and capped ([`Terms::add`]).
#[derive(Debug, Default, PartialEq, Eq)]
struct Terms {
    subject: Vec<String>,
    body: Vec<String>,
}

impl Terms {
    fn add(list: &mut Vec<String>, text: &str) {
        for word in words(text) {
            if list.len() >= MAX_SNIPPET_TERMS {
                return;
            }
            if word.chars().count() <= MAX_TERM_CHARS && !list.contains(&word) {
                list.push(word);
            }
        }
    }
}

/// `text` in lower case, one character at a time (`char::to_lowercase`,
/// so a final `Σ` is `σ` as anywhere else). Texts and terms both go
/// through here, so they always compare alike.
fn fold(text: &str) -> String {
    text.chars().flat_map(char::to_lowercase).collect()
}

/// A text in lower case ([`fold`]) with, for each character of the
/// original, where its lower-case form starts; the last entry is the end.
/// One lower-case character may be longer than the original (`İ` is `i̇`),
/// so offsets don't line up by themselves.
struct Folded {
    lower: String,
    starts: Vec<usize>,
}

impl Folded {
    fn new(text: &str) -> Self {
        let mut lower = String::with_capacity(text.len());
        let mut starts = Vec::with_capacity(text.len().saturating_add(1));
        for c in text.chars() {
            starts.push(lower.len());
            lower.extend(c.to_lowercase());
        }
        starts.push(lower.len());
        Self { lower, starts }
    }

    /// The character at byte `offset` of `lower`, if one starts there.
    fn char_at(&self, offset: usize) -> Option<usize> {
        self.starts.binary_search(&offset).ok()
    }

    /// Matches of `term` (lower case) as ranges of characters of the
    /// original, without overlaps (like `str::match_indices`): after one,
    /// the search goes on from its end. A match that starts or ends inside
    /// the lower-case form of one character doesn't count; after
    /// [`MAX_REJECTED`] of those the search gives up, so a term costs
    /// O(text + `MAX_REJECTED` × term) and never O(text × term).
    fn matches<'a>(&'a self, term: &'a str) -> impl Iterator<Item = (usize, usize)> + 'a {
        let mut from = 0_usize;
        let mut rejected = 0_usize;
        std::iter::from_fn(move || {
            if term.is_empty() {
                return None;
            }
            loop {
                let found = from.checked_add(self.lower.get(from..)?.find(term)?)?;
                let after = found.saturating_add(term.len());
                let start = self.char_at(found);
                let end = self.char_at(after);
                if let (Some(start), Some(end)) = (start, end) {
                    from = after;
                    return Some((start, end));
                }
                rejected = rejected.saturating_add(1);
                if rejected > MAX_REJECTED {
                    return None;
                }
                // Not a match: try again from the next character.
                from = found.saturating_add(
                    self.lower
                        .get(found..)?
                        .chars()
                        .next()
                        .map_or(1, char::len_utf8),
                );
            }
        })
    }
}

/// Matches cut inside a character (`İ` is `i̇` in lower case) looked at for
/// one term in one text before giving up; each costs up to one term's
/// length in comparisons. Real text has a handful at most.
const MAX_REJECTED: usize = 64;

fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split_whitespace()
        .take(MAX_TEXT_TERMS)
        .map(|w| fold(w.trim_matches('"')))
        .filter(|w| !w.is_empty())
}

fn collect(filter: &Filter<EmailFilterCondition>, terms: &mut Terms, negated: bool) {
    match filter {
        Filter::Condition(condition) if !negated => {
            if let Some(text) = &condition.text {
                Terms::add(&mut terms.subject, text);
                Terms::add(&mut terms.body, text);
            }
            if let Some(text) = &condition.subject {
                Terms::add(&mut terms.subject, text);
            }
            if let Some(text) = &condition.body {
                Terms::add(&mut terms.body, text);
            }
        }
        Filter::Operator(operator) => {
            let negated = negated || operator.operator == nebula_jmap_proto::method::Operator::Not;
            for child in &operator.conditions {
                collect(child, terms, negated);
            }
        }
        _ => {}
    }
}

fn escape(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
}

/// `text` escaped with every match of `terms` in `<mark>`, or `None` if
/// nothing matches.
fn highlight(text: &str, terms: &[String]) -> Option<String> {
    let folded = Folded::new(text);
    let count = folded.starts.len().saturating_sub(1);
    let mut marked = vec![false; count];
    let mut any = false;
    for term in terms {
        for (start, end) in folded.matches(term) {
            any = true;
            for flag in marked.iter_mut().take(end).skip(start) {
                *flag = true;
            }
        }
    }
    if !any {
        return None;
    }
    let mut out = String::with_capacity(text.len().saturating_add(32));
    let mut open = false;
    for (c, mark) in text.chars().zip(&marked) {
        if *mark && !open {
            out.push_str("<mark>");
            open = true;
        } else if !*mark && open {
            out.push_str("</mark>");
            open = false;
        }
        escape(c.encode_utf8(&mut [0; 4]), &mut out);
    }
    if open {
        out.push_str("</mark>");
    }
    Some(out)
}

/// Bytes of each text part read for snippets.
const BODY_BYTES: u32 = 16 * 1024;
/// Characters of context kept before the first match.
const BEFORE: usize = 60;
/// Longest snippet, in characters (like `preview`).
const SNIPPET_CHARS: usize = 256;

/// Index (in chars) of the first match of any term in `text`.
fn first_match(text: &str, terms: &[String]) -> Option<usize> {
    let folded = Folded::new(text);
    terms
        .iter()
        .filter_map(|term| folded.matches(term).next().map(|(start, _)| start))
        .min()
}

/// At most the first `max` bytes of `text`, cut at a character boundary.
fn cut(text: &str, max: usize) -> &str {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    text.get(..end).unwrap_or_default()
}

/// The fragment of `text` around the first match, highlighted. Only the
/// first [`BODY_BYTES`] are looked at, the same whether the text came
/// from the server or from the cache (which may hold all of it).
fn body_snippet(text: &str, terms: &[String]) -> Option<String> {
    let text = cut(text, usize::try_from(BODY_BYTES).unwrap_or(usize::MAX));
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let at = first_match(&collapsed, terms)?;
    let chars: Vec<char> = collapsed.chars().collect();
    let mut start = at.saturating_sub(BEFORE);
    // Start at a word, not in the middle of one.
    if start > 0 {
        let next_space = chars
            .iter()
            .skip(start)
            .take(at.saturating_sub(start))
            .position(|c| *c == ' ');
        if let Some(offset) = next_space {
            start = start.saturating_add(offset).saturating_add(1);
        }
    }
    let fragment: String = chars.iter().skip(start).take(SNIPPET_CHARS).collect();
    highlight(&fragment, terms)
}

/// Longest subject looked at, in bytes (a header can be up to 64 KiB).
const SUBJECT_BYTES: usize = 16 * 1024;

/// The snippets of `emails`. Pure CPU work (one linear pass per term and
/// text), run off the async workers by [`get`].
fn snippets(
    emails: &[nebula_store::Email],
    texts: &HashMap<nebula_store::EmailId, String>,
    terms: &Terms,
) -> Result<Vec<SearchSnippet>, CallError> {
    let mut list = Vec::with_capacity(emails.len());
    for email in emails {
        list.push(SearchSnippet {
            email_id: ids::email_id(email.id)?,
            subject: email
                .meta
                .subject
                .as_deref()
                .and_then(|s| highlight(cut(s, SUBJECT_BYTES), &terms.subject)),
            preview: texts
                .get(&email.id)
                .and_then(|text| body_snippet(text, &terms.body))
                .or_else(|| {
                    email
                        .meta
                        .preview
                        .as_deref()
                        .and_then(|p| highlight(cut(p, SUBJECT_BYTES), &terms.body))
                }),
        });
    }
    Ok(list)
}

pub(crate) async fn get(
    ctx: &Ctx<'_>,
    args: SearchSnippetGetArguments,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let mut terms = Terms::default();
    if let Some(filter) = &args.filter {
        collect(filter, &mut terms, false);
    }
    let rows: Vec<_> = args.email_ids.iter().filter_map(ids::store_email).collect();
    let emails = ctx.store().emails(ctx.id(), rows).await?;
    let texts = body_texts(ctx, &terms, &emails).await;
    let found: Vec<_> = emails.iter().map(|e| e.id).collect();
    // Up to 500 emails × 32 terms × 32 KiB of text: off the async workers.
    let list = tokio::task::spawn_blocking(move || snippets(&emails, &texts, &terms))
        .await
        .map_err(|_| CallError::from(crate::error::Bug("snippet task")))??;
    let not_found = args
        .email_ids
        .into_iter()
        .filter(|id| ids::store_email(id).is_none_or(|row| !found.contains(&row)))
        .collect();
    Ok(MethodResponse::SearchSnippetGet(SearchSnippetGetResponse {
        account_id: ctx.jmap_id(),
        list,
        not_found,
    }))
}

/// The body texts, only if there are body terms. A failure (server
/// down...) falls back to the previews.
async fn body_texts(
    ctx: &Ctx<'_>,
    terms: &Terms,
    emails: &[nebula_store::Email],
) -> HashMap<nebula_store::EmailId, String> {
    if terms.body.is_empty() || emails.is_empty() {
        return HashMap::new();
    }
    let Some(bytes) = NonZeroU32::new(BODY_BYTES) else {
        return HashMap::new();
    };
    let ids: Vec<_> = emails.iter().map(|e| e.id).collect();
    let Ok(mail) = ctx.mail() else {
        return HashMap::new();
    };
    mail.body_texts(&ids, bytes).await.unwrap_or_else(|error| {
        tracing::warn!(error = %error, "snippet bodies could not be fetched");
        HashMap::new()
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn marks_and_escapes() {
        let terms = vec!["informe".to_owned()];
        assert_eq!(
            highlight("El Informe <b>anual</b>", &terms).as_deref(),
            Some("El <mark>Informe</mark> &lt;b&gt;anual&lt;/b&gt;")
        );
        assert_eq!(highlight("nada", &terms), None);
        assert_eq!(
            highlight("ÑAÑA ña", &["ña".to_owned()]).as_deref(),
            Some("<mark>ÑAÑA</mark> <mark>ña</mark>")
        );
    }

    #[test]
    fn fragment_around_the_match() {
        let terms = vec!["factura".to_owned()];
        let text = format!(
            "{}la factura de <octubre>\r\n\r\nfin",
            "palabra ".repeat(40)
        );
        let snippet = body_snippet(&text, &terms).unwrap();
        assert!(snippet.starts_with("palabra "), "{snippet:?}");
        assert!(snippet.contains("la <mark>factura</mark> de &lt;octubre&gt; fin"));
        assert!(snippet.chars().count() < 120, "{snippet:?}");
        let short = body_snippet("Factura", &terms).unwrap();
        assert_eq!(short, "<mark>Factura</mark>");
        assert_eq!(body_snippet("nada", &terms), None);
        let long = format!("factura {}", "x".repeat(1000));
        assert!(body_snippet(&long, &terms).unwrap().chars().count() <= 256 + 13);
    }

    #[test]
    fn terms_from_filters() {
        let filter = Filter::parse(json!({"operator": "AND", "conditions": [
            {"text": "hola mundo"}, {"subject": "acta"},
            {"operator": "NOT", "conditions": [{"text": "spam"}]}
        ]}))
        .unwrap();
        let mut terms = Terms::default();
        collect(&filter, &mut terms, false);
        assert_eq!(terms.subject, ["hola", "mundo", "acta"]);
        assert_eq!(terms.body, ["hola", "mundo"]);
    }

    #[test]
    fn terms_are_capped_and_deduplicated() {
        let mut terms = Terms::default();
        for n in 0..10_000 {
            Terms::add(&mut terms.body, &format!("hola t{n}"));
        }
        assert_eq!(terms.body.len(), MAX_SNIPPET_TERMS);
        assert_eq!(terms.body.iter().filter(|t| *t == "hola").count(), 1);
        let mut terms = Terms::default();
        Terms::add(&mut terms.body, &format!("{} corto", "x".repeat(300)));
        assert_eq!(terms.body, ["corto"]);
    }

    #[test]
    fn lower_case_is_per_character() {
        // A final capital sigma is σ, as in the middle of a word, for the
        // text and for the terms alike.
        assert_eq!(fold("ΟΔΟΣ"), "οδοσ");
        assert!(highlight("ΟΔΟΣ", &["οδοσ".to_owned()]).is_some());
        assert_eq!(highlight("ΟΔΟΣ", &["οδος".to_owned()]), None);
        let mut terms = Terms::default();
        Terms::add(&mut terms.subject, "ΟΔΟΣ");
        assert_eq!(
            highlight("la ΟΔΟΣ", &terms.subject).as_deref(),
            Some("la <mark>ΟΔΟΣ</mark>")
        );
        // İ is two characters in lower case: matches don't cut it.
        assert_eq!(
            highlight("İstanbul", &[fold("İstanbul")]).as_deref(),
            Some("<mark>İstanbul</mark>")
        );
        assert_eq!(highlight("İ", &["i".to_owned()]), None);
        assert_eq!(
            highlight("aİb İ", &[fold("İ")]).as_deref(),
            Some("a<mark>İ</mark>b <mark>İ</mark>")
        );
        // Matches don't overlap (like `str::match_indices`).
        assert_eq!(
            highlight("aaa", &["aa".to_owned()]).as_deref(),
            Some("<mark>aa</mark>a")
        );
        assert_eq!(
            highlight("aaaa", &["aa".to_owned()]).as_deref(),
            Some("<mark>aaaa</mark>")
        );
    }

    #[test]
    fn only_the_first_body_bytes_are_searched() {
        let terms = vec!["factura".to_owned()];
        let late = format!("{}factura", "x ".repeat(1024 * 512));
        assert_eq!(body_snippet(&late, &terms), None);
        let early = format!("factura {}", "x ".repeat(1024 * 512));
        assert!(body_snippet(&early, &terms).is_some());
        assert_eq!(cut("añb", 2), "a");
        assert_eq!(cut("añb", 3), "añ");
        assert_eq!(cut("a", 10), "a");
    }

    /// A repeated subject and repeated terms used to cost O(n × m) per
    /// term (overlapping search): 'a' × 65536 with 32 terms of ~250 'a'
    /// took seconds of CPU per email.
    #[test]
    fn repeated_text_is_linear() {
        let subject = "a".repeat(65_536);
        let terms: Vec<String> = (0..32).map(|k| "a".repeat(256 - k)).collect();
        let started = std::time::Instant::now();
        let marked = highlight(cut(&subject, SUBJECT_BYTES), &terms).unwrap();
        assert!(marked.starts_with("<mark>"));
        let snippet = body_snippet(&subject, &terms);
        assert!(snippet.is_some());
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    /// Matches cut inside `İ` (lower case `i̇`) used to restart one
    /// character later every time: after [`MAX_REJECTED`] the term gives up.
    #[test]
    fn rejected_matches_are_capped() {
        let text = "İ".repeat(8 * 1024);
        let term = format!("{}i", fold(&"İ".repeat(200)));
        let folded = Folded::new(&text);
        assert_eq!(folded.matches(&term).next(), None);
        let started = std::time::Instant::now();
        assert_eq!(first_match(&text, &[term]), None);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        // A good match after a few rejected ones is still found.
        assert_eq!(
            highlight("İ i", &["i".to_owned()]).as_deref(),
            Some("İ <mark>i</mark>")
        );
    }
}
