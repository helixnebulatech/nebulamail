// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

use diesel_async::pooled_connection::deadpool::PoolError;

/// Errors from the store. None of them carries user data.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    /// The database file could not be opened.
    #[error("cannot open the database: {0}")]
    Connection(#[from] diesel::ConnectionError),
    /// No connection could be taken from the pool.
    #[error("database pool: {0}")]
    Pool(#[from] PoolError),
    /// A query failed.
    #[error("database query failed: {0}")]
    Query(#[from] diesel::result::Error),
    /// The schema migrations failed.
    #[error("database migration failed: {0}")]
    Migration(String),
    /// A blocking database task panicked or was cancelled.
    #[error("database task failed")]
    Task,
    /// The database file could not be created (the error carries no path).
    #[error("cannot create the database file: {0}")]
    Create(#[source] std::io::Error),
    /// The database path is not valid UTF-8.
    #[error("the database path must be valid UTF-8")]
    Path,
    /// A row holds a value this version can't read (e.g. an unknown enum).
    #[error("invalid value in the database: {0}")]
    Corrupt(&'static str),
    /// A value given to the store breaks its rules.
    #[error("invalid {0}")]
    Invalid(&'static str),
    /// The account already has as many of these as allowed.
    #[error("too many {0}")]
    OverQuota(&'static str),
    /// The object does not exist (or belongs to another account).
    #[error("{0} not found")]
    NotFound(&'static str),
    /// The system clock is before 1970 or after year 292 billion.
    #[error("the system clock is out of range")]
    Clock,
}
