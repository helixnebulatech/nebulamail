// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `Email/query` on the local metadata (RFC 8621 §4.4). Text search runs on
//! the IMAP server (`nebula-mail`) and comes back as an [`EmailFilter::Ids`]
//! condition.
//!
//! The SQL is built from the typed filter; every value is a bound
//! parameter, never text pasted into the query.

use diesel::sql_types::{BigInt, Text};
use diesel::sqlite::Sqlite;
use diesel::{RunQueryDsl as _, SqliteConnection};
use nebula_jmap_proto::Keyword;

use super::convert::{email_id, id_list, u64_to_db};
use crate::{AccountId, EmailId, MailboxId, Store, StoreError, UnixTime};

/// Deepest nesting of AND/OR/NOT (jmap-proto allows 16 too).
const MAX_DEPTH: usize = 16;

/// Most nodes (conditions and operators) in one filter: a backstop so a
/// wide filter can't hold the database for long. One JMAP condition
/// becomes an AND of up to about 14 nodes here, so this is well above
/// what jmap-proto's own limit lets through (256 nodes).
pub const MAX_FILTER_NODES: usize = 4096;

/// Most mailboxes in one `inMailboxOtherThan`.
pub const MAX_OTHER_THAN_MAILBOXES: usize = 1000;

/// A filter condition (RFC 8621 §4.4.1, without the text conditions).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EmailFilter {
    /// `inMailbox`.
    InMailbox(MailboxId),
    /// `inMailboxOtherThan`: in at least one mailbox not in the list.
    InMailboxOtherThan(Vec<MailboxId>),
    /// `hasKeyword`.
    HasKeyword(Keyword),
    /// `notKeyword`.
    NotKeyword(Keyword),
    /// `allInThreadHaveKeyword`.
    AllInThreadHaveKeyword(Keyword),
    /// `someInThreadHaveKeyword`.
    SomeInThreadHaveKeyword(Keyword),
    /// `noneInThreadHaveKeyword`.
    NoneInThreadHaveKeyword(Keyword),
    /// `before`: received before this time.
    Before(UnixTime),
    /// `after`: received at or after this time.
    After(UnixTime),
    /// `minSize`: at least this many bytes.
    MinSize(u64),
    /// `maxSize`: fewer than this many bytes.
    MaxSize(u64),
    /// `hasAttachment`.
    HasAttachment(bool),
    /// One of these emails (the result of a server-side text search).
    Ids(Vec<EmailId>),
    /// All conditions.
    And(Vec<EmailFilter>),
    /// At least one condition.
    Or(Vec<EmailFilter>),
    /// None of the conditions (JMAP `NOT`).
    Not(Vec<EmailFilter>),
}

/// What to sort by (RFC 8621 §4.4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SortKey {
    /// `receivedAt`.
    ReceivedAt,
    /// `sentAt` (emails without `Date` first when ascending).
    SentAt,
    /// `size`.
    Size,
    /// `subject`, without `Re:`-style prefixes, case-insensitive.
    Subject,
    /// `from`: the first sender's name, else address.
    From,
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Smallest first.
    Ascending,
    /// Largest first.
    Descending,
}

/// One sort criterion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmailSort {
    /// What to compare.
    pub key: SortKey,
    /// Which way.
    pub direction: Direction,
}

/// Where the page starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryStart {
    /// A position; negative counts from the end.
    Position(i64),
    /// Relative to an email in the results.
    Anchor {
        /// The anchor email.
        id: EmailId,
        /// Offset from it (negative: before it).
        offset: i64,
    },
}

/// An email query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailQuery {
    /// The filter (`None`: every email).
    pub filter: Option<EmailFilter>,
    /// The sort; empty means `receivedAt` descending.
    pub sort: Vec<EmailSort>,
    /// Keep only the first email of each thread.
    pub collapse_threads: bool,
    /// Where the page starts.
    pub start: QueryStart,
    /// At most this many ids.
    pub limit: Option<u64>,
}

/// A page of results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailQueryResult {
    /// The ids, in order.
    pub ids: Vec<EmailId>,
    /// Position of the first id in the full results.
    pub position: u64,
    /// How many results there are in total.
    pub total: u64,
}

enum Bind {
    Int(i64),
    Text(String),
}

/// SQL text with its parameters, in order.
#[derive(Default)]
struct Sql {
    text: String,
    binds: Vec<Bind>,
    /// Filter nodes seen so far ([`MAX_FILTER_NODES`]).
    nodes: usize,
}

impl Sql {
    fn push(&mut self, text: &str) {
        self.text.push_str(text);
    }

    fn int(&mut self, value: i64) {
        self.text.push('?');
        self.binds.push(Bind::Int(value));
    }

    fn text(&mut self, value: String) {
        self.text.push('?');
        self.binds.push(Bind::Text(value));
    }

    fn filter(&mut self, filter: &EmailFilter, depth: usize) -> Result<(), StoreError> {
        if depth > MAX_DEPTH {
            return Err(StoreError::Invalid("filter too deep"));
        }
        self.nodes = self.nodes.saturating_add(1);
        if self.nodes > MAX_FILTER_NODES {
            return Err(StoreError::Invalid("filter too large"));
        }
        let next = depth.saturating_add(1);
        match filter {
            EmailFilter::InMailbox(mailbox) => {
                self.push(
                    "EXISTS (SELECT 1 FROM email_locations l WHERE l.email_id = e.id \
                     AND l.mailbox_id = ",
                );
                self.int(mailbox.get());
                self.push(")");
            }
            EmailFilter::InMailboxOtherThan(list) => {
                if list.len() > MAX_OTHER_THAN_MAILBOXES {
                    return Err(StoreError::Invalid("filter mailbox list too long"));
                }
                self.push(
                    "EXISTS (SELECT 1 FROM email_locations l WHERE l.email_id = e.id \
                     AND l.mailbox_id NOT IN (SELECT value FROM json_each(",
                );
                self.text(id_list(list.iter().map(|id| id.get())));
                self.push(")))");
            }
            EmailFilter::HasKeyword(keyword) => self.has_keyword("e", keyword),
            EmailFilter::NotKeyword(keyword) => {
                self.push("NOT ");
                self.has_keyword("e", keyword);
            }
            EmailFilter::SomeInThreadHaveKeyword(keyword) => {
                self.thread_email("EXISTS");
                self.has_keyword("t", keyword);
                self.push(")");
            }
            EmailFilter::NoneInThreadHaveKeyword(keyword) => {
                self.thread_email("NOT EXISTS");
                self.has_keyword("t", keyword);
                self.push(")");
            }
            EmailFilter::AllInThreadHaveKeyword(keyword) => {
                self.thread_email("NOT EXISTS");
                self.push("NOT ");
                self.has_keyword("t", keyword);
                self.push(")");
            }
            EmailFilter::Before(time) => {
                self.push("e.received_at < ");
                self.int(time.secs());
            }
            EmailFilter::After(time) => {
                self.push("e.received_at >= ");
                self.int(time.secs());
            }
            EmailFilter::MinSize(size) => {
                self.push("e.size >= ");
                self.int(u64_to_db(*size, "size").unwrap_or(i64::MAX));
            }
            EmailFilter::MaxSize(size) => {
                self.push("e.size < ");
                self.int(u64_to_db(*size, "size").unwrap_or(i64::MAX));
            }
            EmailFilter::HasAttachment(value) => {
                self.push("e.has_attachment = ");
                self.int(i64::from(*value));
            }
            EmailFilter::Ids(ids) => {
                self.push("e.id IN (SELECT value FROM json_each(");
                self.text(id_list(ids.iter().map(|id| id.get())));
                self.push("))");
            }
            EmailFilter::And(list) => self.join(list, " AND ", "1", next)?,
            EmailFilter::Or(list) => self.join(list, " OR ", "0", next)?,
            EmailFilter::Not(list) => {
                self.push("NOT ");
                self.join(list, " OR ", "0", next)?;
            }
        }
        Ok(())
    }

    fn join(
        &mut self,
        list: &[EmailFilter],
        operator: &str,
        empty: &str,
        depth: usize,
    ) -> Result<(), StoreError> {
        if list.is_empty() {
            self.push(empty);
            return Ok(());
        }
        self.balanced(list, operator, depth)
    }

    /// `(a) op (b) op …` grouped as a balanced tree: SQLite parses a flat
    /// chain into an expression as deep as it is long, and gives up past
    /// a depth of 1000; halves keep it at about log2 of the width.
    fn balanced(
        &mut self,
        list: &[EmailFilter],
        operator: &str,
        depth: usize,
    ) -> Result<(), StoreError> {
        self.push("(");
        if let [item] = list {
            self.filter(item, depth)?;
        } else {
            let (left, right) = list.split_at(list.len() / 2);
            self.balanced(left, operator, depth)?;
            self.push(operator);
            self.balanced(right, operator, depth)?;
        }
        self.push(")");
        Ok(())
    }

    fn has_keyword(&mut self, table: &str, keyword: &Keyword) {
        self.push("EXISTS (SELECT 1 FROM email_keywords k WHERE k.email_id = ");
        self.push(table);
        self.push(".id AND k.keyword = ");
        self.text(keyword.as_str().to_owned());
        self.push(")");
    }

    /// `<quantifier> (SELECT 1 FROM emails t WHERE <same thread, visible> AND`
    /// — the caller adds the condition on `t` and the closing parenthesis.
    fn thread_email(&mut self, quantifier: &str) {
        self.push(quantifier);
        self.push(
            " (SELECT 1 FROM emails t WHERE t.thread_id = e.thread_id \
             AND EXISTS (SELECT 1 FROM email_locations tl WHERE tl.email_id = t.id) AND ",
        );
    }

    fn run<T>(self, conn: &mut SqliteConnection) -> Result<Vec<T>, StoreError>
    where
        T: diesel::QueryableByName<Sqlite> + 'static,
    {
        let mut query = diesel::sql_query(self.text).into_boxed::<Sqlite>();
        for bind in self.binds {
            query = match bind {
                Bind::Int(value) => query.bind::<BigInt, _>(value),
                Bind::Text(value) => query.bind::<Text, _>(value),
            };
        }
        Ok(query.load(conn)?)
    }
}

fn order_by(sort: &[EmailSort]) -> String {
    let default = [EmailSort {
        key: SortKey::ReceivedAt,
        direction: Direction::Descending,
    }];
    let sort = if sort.is_empty() { &default[..] } else { sort };
    let mut parts: Vec<String> = sort
        .iter()
        .map(|item| {
            let column = match item.key {
                SortKey::ReceivedAt => "received_at",
                SortKey::SentAt => "sent_at",
                SortKey::Size => "size",
                SortKey::Subject => "sort_subject",
                SortKey::From => "sort_from",
            };
            let direction = match item.direction {
                Direction::Ascending => "ASC",
                Direction::Descending => "DESC",
            };
            format!("{column} {direction}")
        })
        .collect();
    // Newest id last: a stable order for equal keys.
    parts.push("id DESC".to_owned());
    parts.join(", ")
}

/// `WITH matches AS (...), results AS (...)`: the filtered, sorted (and
/// maybe collapsed) results with their 1-based `pos`.
fn results_cte(account: AccountId, query: &EmailQuery) -> Result<Sql, StoreError> {
    let mut sql = Sql::default();
    sql.push(
        "WITH matches AS (SELECT e.id AS id, e.thread_id AS thread_id, \
         e.received_at AS received_at, e.sent_at AS sent_at, e.size AS size, \
         e.sort_subject AS sort_subject, e.sort_from AS sort_from \
         FROM emails e WHERE e.account_id = ",
    );
    sql.int(account.get());
    sql.push(" AND EXISTS (SELECT 1 FROM email_locations v WHERE v.email_id = e.id)");
    if let Some(filter) = &query.filter {
        sql.push(" AND (");
        sql.filter(filter, 0)?;
        sql.push(")");
    }
    sql.push("), ");
    let order = order_by(&query.sort);
    if query.collapse_threads {
        sql.push(&format!(
            "firsts AS (SELECT id, ROW_NUMBER() OVER (ORDER BY {order}) AS rank, \
             ROW_NUMBER() OVER (PARTITION BY thread_id ORDER BY {order}) AS in_thread \
             FROM matches), \
             results AS (SELECT id, ROW_NUMBER() OVER (ORDER BY rank) AS pos \
             FROM firsts WHERE in_thread = 1) "
        ));
    } else {
        sql.push(&format!(
            "results AS (SELECT id, ROW_NUMBER() OVER (ORDER BY {order}) AS pos FROM matches) "
        ));
    }
    Ok(sql)
}

#[derive(diesel::QueryableByName)]
struct CountRow {
    #[diesel(sql_type = BigInt)]
    count: i64,
}

#[derive(diesel::QueryableByName)]
struct PosRow {
    #[diesel(sql_type = BigInt)]
    pos: i64,
}

#[derive(diesel::QueryableByName)]
struct IdRow {
    #[diesel(sql_type = BigInt)]
    id: i64,
}

fn run_query(
    conn: &mut SqliteConnection,
    account: AccountId,
    query: &EmailQuery,
) -> Result<EmailQueryResult, StoreError> {
    let mut count = results_cte(account, query)?;
    count.push("SELECT COUNT(*) AS count FROM results");
    let total = count
        .run::<CountRow>(conn)?
        .first()
        .map_or(0, |row| row.count);

    let position = match query.start {
        QueryStart::Position(position) if position < 0 => total.saturating_add(position).max(0),
        QueryStart::Position(position) => position,
        QueryStart::Anchor { id, offset } => {
            let mut anchor = results_cte(account, query)?;
            anchor.push("SELECT pos FROM results WHERE id = ");
            anchor.int(id.get());
            let pos = anchor
                .run::<PosRow>(conn)?
                .first()
                .map(|row| row.pos)
                .ok_or(StoreError::NotFound("anchor"))?;
            // `pos` is 1-based.
            pos.saturating_sub(1).saturating_add(offset).max(0)
        }
    };

    let mut page = results_cte(account, query)?;
    page.push("SELECT id FROM results WHERE pos > ");
    page.int(position);
    page.push(" ORDER BY pos");
    if let Some(limit) = query.limit {
        page.push(" LIMIT ");
        page.int(u64_to_db(limit, "limit").unwrap_or(i64::MAX));
    }
    let ids = page
        .run::<IdRow>(conn)?
        .into_iter()
        .map(|row| email_id(row.id))
        .collect::<Result<_, _>>()?;
    Ok(EmailQueryResult {
        ids,
        position: u64::try_from(position).unwrap_or(0),
        total: u64::try_from(total).unwrap_or(0),
    })
}

impl Store {
    /// Runs an email query on the stored metadata.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] (`"anchor"`) if the anchor isn't in the
    /// results, [`StoreError::Invalid`] for a filter nested too deep.
    pub async fn query_emails(
        &self,
        account: AccountId,
        query: EmailQuery,
    ) -> Result<EmailQueryResult, StoreError> {
        self.read(move |conn| run_query(conn, account, &query))
            .await
    }
}
