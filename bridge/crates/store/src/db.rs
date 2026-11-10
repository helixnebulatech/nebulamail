// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

use std::path::Path;

use diesel::{Connection as _, SqliteConnection};
use diesel_async::pooled_connection::deadpool::{Object, Pool};
use diesel_async::pooled_connection::{AsyncDieselConnectionManager, ManagerConfig};
use diesel_async::sync_connection_wrapper::SyncConnectionWrapper;
use diesel_async::{AsyncConnection as _, SimpleAsyncConnection as _};
use diesel_migrations::{EmbeddedMigrations, MigrationHarness as _, embed_migrations};
use futures::FutureExt as _;
use futures::future::BoxFuture;

use crate::StoreError;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

/// Settings for every connection. `foreign_keys`, `busy_timeout` and
/// `secure_delete` are per connection in SQLite, so they go here and not
/// in a migration.
///
/// `secure_delete`: what is deleted is overwritten with zeros, not left in
/// free pages of the file. A destroyed push subscription's URL and keys
/// MUST be securely erased from storage (RFC 8620 §7.2); the same goes for
/// everything else we delete (sealed mail, tokens). Old copies can stay in
/// the WAL until its frames are reused or a checkpoint truncates it.
const CONNECTION_PRAGMAS: &str = "
    PRAGMA foreign_keys = ON;
    PRAGMA busy_timeout = 5000;
    PRAGMA synchronous = NORMAL;
    PRAGMA secure_delete = ON;
";

/// SQLite allows one writer at a time; a few connections are enough for
/// concurrent reads (WAL mode).
const POOL_SIZE: usize = 8;

pub(crate) type Conn = SyncConnectionWrapper<SqliteConnection>;

/// The bridge database. Cheap to clone: clones share the connection pool.
#[derive(Clone)]
pub struct Store {
    pool: Pool<Conn>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").finish_non_exhaustive()
    }
}

impl Store {
    /// Opens (or creates) the database at `path` and brings its schema up to
    /// date.
    ///
    /// A new file is created readable and writable only by the bridge (0600
    /// on unix, whatever the umask), and SQLite gives its `-wal`, `-shm` and
    /// `-journal` files the same mode. The database holds sealed secrets,
    /// token hashes and, in clear, the mail metadata cache. SQLite's special
    /// names (`:memory:` and `file:` URIs) are passed through untouched; the
    /// bridge always gives a plain path.
    ///
    /// # Errors
    /// If the file can't be created or opened, the path isn't UTF-8 or a
    /// migration fails.
    pub async fn open(path: &Path) -> Result<Self, StoreError> {
        let url = path.to_str().ok_or(StoreError::Path)?.to_owned();
        if !is_special_name(&url) {
            create_private(path)?;
        }
        migrate(url.clone()).await?;

        let mut config = ManagerConfig::default();
        config.custom_setup = Box::new(establish);
        let manager = AsyncDieselConnectionManager::<Conn>::new_with_config(url, config);
        let pool = Pool::builder(manager)
            .max_size(POOL_SIZE)
            .build()
            .map_err(|_| StoreError::Task)?;
        Ok(Self { pool })
    }

    /// A connection from the pool.
    pub(crate) async fn conn(&self) -> Result<Object<Conn>, StoreError> {
        Ok(self.pool.get().await?)
    }

    /// Runs `work` in one write transaction on a blocking thread.
    ///
    /// `BEGIN IMMEDIATE` takes the write lock at the start: a deferred
    /// transaction that reads first and writes later can fail with
    /// `SQLITE_BUSY` when another writer got in between, and the busy timeout
    /// does not help then. The async transactions on a pooled connection
    /// (oauth.rs, changes.rs) use `immediate_transaction` for the same reason.
    pub(crate) async fn write<T, F>(&self, work: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&mut SqliteConnection) -> Result<T, StoreError> + Send + 'static,
    {
        let mut conn = self.conn().await?;
        conn.spawn_blocking(move |conn| Ok(conn.immediate_transaction(work)))
            .await?
    }

    /// Runs `work` in one read transaction (a consistent snapshot) on a
    /// blocking thread.
    pub(crate) async fn read<T, F>(&self, work: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&mut SqliteConnection) -> Result<T, StoreError> + Send + 'static,
    {
        let mut conn = self.conn().await?;
        conn.spawn_blocking(move |conn| Ok(conn.transaction(work)))
            .await?
    }
}

/// SQLite names that are not a file path: an in-memory database or a URI.
fn is_special_name(url: &str) -> bool {
    url == ":memory:" || url.starts_with("file:")
}

/// Creates the database file with mode 0600 if it does not exist yet, so
/// SQLite never makes it (and its companions) with the umask's mode.
#[cfg(unix)]
fn create_private(path: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::OpenOptionsExt as _;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(StoreError::Create(error)),
    }
}

#[cfg(not(unix))]
fn create_private(_path: &Path) -> Result<(), StoreError> {
    Ok(())
}

fn establish(url: &str) -> BoxFuture<'_, diesel::ConnectionResult<Conn>> {
    async move {
        let mut conn = Conn::establish(url).await?;
        conn.batch_execute(CONNECTION_PRAGMAS)
            .await
            .map_err(diesel::ConnectionError::CouldntSetupConfiguration)?;
        Ok(conn)
    }
    .boxed()
}

/// Runs the pending migrations on a blocking thread (diesel's migrations are
/// synchronous) and switches the file to WAL mode, which is persistent.
async fn migrate(url: String) -> Result<(), StoreError> {
    tokio::task::spawn_blocking(move || {
        let mut conn = SqliteConnection::establish(&url)?;
        diesel::connection::SimpleConnection::batch_execute(
            &mut conn,
            "PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;",
        )?;
        conn.run_pending_migrations(MIGRATIONS)
            .map_err(|e| StoreError::Migration(e.to_string()))?;
        Ok(())
    })
    .await
    .map_err(|_| StoreError::Task)?
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::Store;

    /// A store in a fresh temporary directory (deleted when the guard drops).
    pub(crate) async fn store() -> (Store, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("test.db")).await.unwrap();
        (store, dir)
    }
}

#[cfg(test)]
mod tests {
    use diesel::sql_types::BigInt;
    use diesel_async::RunQueryDsl as _;

    use super::test_support::store;

    #[derive(diesel::QueryableByName)]
    struct Value {
        #[diesel(sql_type = BigInt)]
        value: i64,
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn connections_enforce_foreign_keys() {
        let (store, _dir) = store().await;
        let mut conn = store.conn().await.unwrap();
        let row: Value = diesel::sql_query("SELECT foreign_keys AS value FROM pragma_foreign_keys")
            .get_result(&mut conn)
            .await
            .unwrap();
        assert_eq!(row.value, 1);
    }

    /// Deleted bytes don't stay in the file (`secure_delete`, RFC 8620
    /// §7.2 for push subscriptions).
    #[tokio::test(flavor = "multi_thread")]
    async fn deleted_data_is_overwritten() {
        let (store, dir) = store().await;
        let mut conn = store.conn().await.unwrap();
        let marker = "nebula-erase-me-".repeat(64);
        for statement in [
            "CREATE TABLE erase_probe (data BLOB)".to_owned(),
            format!("INSERT INTO erase_probe VALUES (CAST('{marker}' AS BLOB))"),
            "PRAGMA wal_checkpoint(TRUNCATE)".to_owned(),
            "DELETE FROM erase_probe".to_owned(),
            "PRAGMA wal_checkpoint(TRUNCATE)".to_owned(),
        ] {
            diesel::sql_query(statement)
                .execute(&mut conn)
                .await
                .unwrap();
        }
        let mut found = false;
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let bytes = std::fs::read(entry.unwrap().path()).unwrap();
            found |= bytes
                .windows(marker.len())
                .any(|window| window == marker.as_bytes());
        }
        assert!(!found, "the deleted row is still in the database files");
        let row: Value =
            diesel::sql_query("SELECT secure_delete AS value FROM pragma_secure_delete")
                .get_result(&mut conn)
                .await
                .unwrap();
        assert_eq!(row.value, 1);
    }

    /// Runs the migrations before the one whose name contains `before`.
    fn migrate_until(conn: &mut diesel::SqliteConnection, before: &str) {
        use diesel_migrations::MigrationHarness as _;
        diesel::connection::SimpleConnection::batch_execute(conn, "PRAGMA foreign_keys = ON;")
            .unwrap();
        loop {
            let pending = conn.pending_migrations(super::MIGRATIONS).unwrap();
            let next = <[_]>::first(&pending).unwrap();
            if next.name().to_string().contains(before) {
                return;
            }
            conn.run_migration(next.as_ref()).unwrap();
        }
    }

    fn count(conn: &mut diesel::SqliteConnection, sql: &str) -> i64 {
        diesel::RunQueryDsl::get_result::<Value>(diesel::sql_query(sql), conn)
            .unwrap()
            .value
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn account_ids_are_never_reused() {
        use diesel::Connection as _;
        use diesel::connection::SimpleConnection as _;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let url = path.to_str().unwrap().to_owned();
        {
            // A database from before the migration, with data that
            // references the accounts (and a deleted last account).
            let mut conn = diesel::SqliteConnection::establish(&url).unwrap();
            migrate_until(&mut conn, "accounts_autoincrement");
            conn.batch_execute(
                "INSERT INTO accounts (id, login, email, wrapped_dek, created_at, last_login_at)
                     VALUES (1, 'alice', 'a@x', x'00', 1, 1), (2, 'bob', 'b@x', x'00', 1, 1);
                 INSERT INTO account_secrets (account_id, field, sealed) VALUES (1, 1, x'01');
                 INSERT INTO mailboxes (account_id, imap_name, name, subscribed, selectable)
                     VALUES (1, 'INBOX', 'INBOX', 1, 1);",
            )
            .unwrap();
        }
        let store = super::Store::open(&path).await.unwrap();
        let mut conn = diesel::SqliteConnection::establish(&url).unwrap();
        conn.batch_execute("PRAGMA foreign_keys = ON;").unwrap();
        // Nothing that referenced the accounts was lost.
        assert_eq!(
            count(&mut conn, "SELECT COUNT(*) AS value FROM account_secrets"),
            1
        );
        assert_eq!(
            count(&mut conn, "SELECT COUNT(*) AS value FROM mailboxes"),
            1
        );
        assert_eq!(
            count(
                &mut conn,
                "SELECT COUNT(*) AS value FROM pragma_foreign_key_check"
            ),
            0
        );
        // Deleting the last account doesn't free its id, and cascades still
        // work.
        conn.batch_execute("DELETE FROM accounts WHERE id = 2;")
            .unwrap();
        let carol = store
            .get_or_create_account(
                crate::NewAccount {
                    login: "carol",
                    email: "c@x",
                    wrapped_dek: b"k",
                },
                crate::UnixTime::from_secs(5),
            )
            .await
            .unwrap();
        assert_eq!(carol.id.get(), 3);
        conn.batch_execute("DELETE FROM accounts WHERE id = 1;")
            .unwrap();
        assert_eq!(
            count(&mut conn, "SELECT COUNT(*) AS value FROM mailboxes"),
            0
        );
    }

    /// Recurring calendar resources with an end lose their etag, and their
    /// collection its token, so a sync indexes them again; nothing else.
    #[tokio::test(flavor = "multi_thread")]
    async fn closed_series_are_fetched_again() {
        use diesel::Connection as _;
        use diesel::connection::SimpleConnection as _;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let url = path.to_str().unwrap().to_owned();
        {
            let mut conn = diesel::SqliteConnection::establish(&url).unwrap();
            migrate_until(&mut conn, "reindex_calendar_ends");
            conn.batch_execute(
                "INSERT INTO accounts (id, login, email, wrapped_dek, created_at, last_login_at)
                     VALUES (1, 'alice', 'a@x', x'00', 1, 1);
                 INSERT INTO dav_collections (id, account_id, kind, href, supports_sync,
                         sync_token, ctag, synced_at, is_visible, is_subscribed, is_default,
                         publishable, is_subscription)
                     VALUES (1, 1, 1, 'http://h/c/', 1, 't1', 'c1', 5, 1, 1, 1, 0, 0),
                            (2, 1, 1, 'http://h/d/', 1, 't2', 'c2', 5, 1, 1, 0, 0, 0),
                            (3, 1, 2, 'http://h/b/', 1, 't3', 'c3', 5, 1, 1, 1, 0, 0);
                 INSERT INTO dav_resources (account_id, collection_id, href, etag, data, size,
                         utc_end, recurring, invalid, updated_at)
                     VALUES (1, 1, 'http://h/c/series', 'e1', x'00', 1, 9, 1, 0, 1),
                            (1, 1, 'http://h/c/endless', 'e2', x'00', 1, NULL, 1, 0, 1),
                            (1, 2, 'http://h/d/single', 'e3', x'00', 1, 9, 0, 0, 1),
                            (1, 3, 'http://h/b/card', 'e4', x'00', 1, 9, 1, 0, 1);",
            )
            .unwrap();
        }
        drop(super::Store::open(&path).await.unwrap());
        let mut conn = diesel::SqliteConnection::establish(&url).unwrap();
        assert_eq!(
            count(
                &mut conn,
                "SELECT COUNT(*) AS value FROM dav_resources WHERE etag IS NULL"
            ),
            1
        );
        assert_eq!(
            count(
                &mut conn,
                "SELECT COUNT(*) AS value FROM dav_resources WHERE etag IS NULL AND href = 'http://h/c/series'"
            ),
            1
        );
        assert_eq!(
            count(
                &mut conn,
                "SELECT COUNT(*) AS value FROM dav_collections WHERE sync_token IS NULL AND ctag IS NULL"
            ),
            1
        );
        assert_eq!(
            count(
                &mut conn,
                "SELECT COUNT(*) AS value FROM dav_collections WHERE id = 1 AND sync_token IS NULL"
            ),
            1
        );
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn the_database_is_private_whatever_the_umask() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let store = super::Store::open(&path).await.unwrap();
        // A write, so the WAL file exists.
        store
            .get_or_create_account(
                crate::NewAccount {
                    login: "alice",
                    email: "a@x",
                    wrapped_dek: b"k",
                },
                crate::UnixTime::from_secs(5),
            )
            .await
            .unwrap();
        let wal = dir.path().join("test.db-wal");
        for file in [&path, &wal] {
            let mode = std::fs::metadata(file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{}", file.display());
        }
        drop(store);
        // An existing database opens again.
        super::Store::open(&path).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_missing_directory_is_a_create_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing").join("test.db");
        let error = super::Store::open(&path).await.unwrap_err();
        if cfg!(unix) {
            assert!(matches!(error, crate::StoreError::Create(_)), "{error}");
        }
    }

    #[test]
    fn special_names_are_not_files() {
        assert!(super::is_special_name(":memory:"));
        assert!(super::is_special_name("file:test.db?mode=memory"));
        assert!(!super::is_special_name("/var/lib/bridge/bridge.db"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn reopening_keeps_the_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        drop(super::Store::open(&path).await.unwrap());
        super::Store::open(&path).await.unwrap();
    }
}
