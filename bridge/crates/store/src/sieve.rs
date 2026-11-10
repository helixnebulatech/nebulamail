// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Sieve scripts: ManageSieve knows scripts by name, JMAP
//! by id. Each listing from the server is compared with the rows here in
//! one transaction: new names get a new id, missing ones are destroyed,
//! a changed active flag or content is an update, and every difference
//! goes to the change log (`SieveScript/changes`). Renames made through
//! the bridge keep the id.

use std::collections::HashMap;

use diesel::{ExpressionMethods as _, QueryDsl as _, RunQueryDsl as _, SqliteConnection};

use crate::record::record_in;
use crate::schema::sieve_scripts;
use crate::{AccountId, Change, ChangeKind, DataType, Seq, SieveScriptId, Store, StoreError};

/// One script as the server lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedSieveScript {
    /// Its name.
    pub name: String,
    /// It is the active script.
    pub active: bool,
    /// A hash of its content (opaque to the store).
    pub content_hash: String,
}

/// A stored script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SieveScriptRow {
    /// Its id.
    pub id: SieveScriptId,
    /// Its name.
    pub name: String,
    /// It is the active script.
    pub active: bool,
    /// The hash of its content.
    pub content_hash: String,
}

type Row = (i64, String, i32, String);

fn rows(
    conn: &mut SqliteConnection,
    account: AccountId,
) -> Result<Vec<SieveScriptRow>, StoreError> {
    let found: Vec<Row> = sieve_scripts::table
        .filter(sieve_scripts::account_id.eq(account.get()))
        .order(sieve_scripts::id.asc())
        .select((
            sieve_scripts::id,
            sieve_scripts::name,
            sieve_scripts::active,
            sieve_scripts::content_hash,
        ))
        .load(conn)?;
    found
        .into_iter()
        .map(|(id, name, active, content_hash)| {
            Ok(SieveScriptRow {
                id: SieveScriptId::from_raw(id).ok_or(StoreError::Corrupt("sieve script id"))?,
                name,
                active: active != 0,
                content_hash,
            })
        })
        .collect()
}

impl Store {
    /// Compares the server's listing with the stored scripts, records the
    /// differences and returns the scripts (in id order) and the state.
    ///
    /// # Errors
    /// On database errors.
    pub async fn sync_sieve_scripts(
        &self,
        account: AccountId,
        listed: Vec<ListedSieveScript>,
    ) -> Result<(Vec<SieveScriptRow>, Seq), StoreError> {
        self.write(move |conn| {
            let mut stored: HashMap<String, SieveScriptRow> = rows(conn, account)?
                .into_iter()
                .map(|row| (row.name.clone(), row))
                .collect();
            let mut changes = Vec::new();
            for script in listed {
                let active = i32::from(script.active);
                if let Some(row) = stored.remove(&script.name) {
                    {
                        if row.active != script.active || row.content_hash != script.content_hash {
                            diesel::update(sieve_scripts::table.find(row.id.get()))
                                .set((
                                    sieve_scripts::active.eq(active),
                                    sieve_scripts::content_hash.eq(&script.content_hash),
                                ))
                                .execute(conn)?;
                            changes.push(Change {
                                object: row.id.get(),
                                kind: ChangeKind::Updated,
                            });
                        }
                    }
                } else {
                    {
                        let id: i64 = diesel::insert_into(sieve_scripts::table)
                            .values((
                                sieve_scripts::account_id.eq(account.get()),
                                sieve_scripts::name.eq(&script.name),
                                sieve_scripts::active.eq(active),
                                sieve_scripts::content_hash.eq(&script.content_hash),
                            ))
                            .returning(sieve_scripts::id)
                            .get_result(conn)?;
                        changes.push(Change {
                            object: id,
                            kind: ChangeKind::Created,
                        });
                    }
                }
            }
            for gone in stored.into_values() {
                diesel::delete(sieve_scripts::table.find(gone.id.get())).execute(conn)?;
                changes.push(Change {
                    object: gone.id.get(),
                    kind: ChangeKind::Destroyed,
                });
            }
            let seq = record_in(conn, account, DataType::SieveScript, &changes)?;
            Ok((rows(conn, account)?, seq))
        })
        .await
    }

    /// The stored scripts of an account, as last synced.
    ///
    /// # Errors
    /// On database errors.
    pub async fn sieve_scripts(
        &self,
        account: AccountId,
    ) -> Result<Vec<SieveScriptRow>, StoreError> {
        self.read(move |conn| rows(conn, account)).await
    }

    /// The server renamed script `id` (through the bridge): the row keeps
    /// its id. Any other row with the new name was stale and is destroyed.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] if the script isn't stored; database errors.
    pub async fn rename_sieve_script(
        &self,
        account: AccountId,
        id: SieveScriptId,
        name: String,
    ) -> Result<Seq, StoreError> {
        self.write(move |conn| {
            let scripts = rows(conn, account)?;
            if !scripts.iter().any(|row| row.id == id) {
                return Err(StoreError::NotFound("sieve script"));
            }
            let mut changes = Vec::new();
            for stale in scripts
                .iter()
                .filter(|row| row.name == name && row.id != id)
            {
                diesel::delete(sieve_scripts::table.find(stale.id.get())).execute(conn)?;
                changes.push(Change {
                    object: stale.id.get(),
                    kind: ChangeKind::Destroyed,
                });
            }
            diesel::update(sieve_scripts::table.find(id.get()))
                .set(sieve_scripts::name.eq(&name))
                .execute(conn)?;
            changes.push(Change {
                object: id.get(),
                kind: ChangeKind::Updated,
            });
            record_in(conn, account, DataType::SieveScript, &changes)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChangesSince;
    use crate::account::tests::alice;
    use crate::db::test_support::store;

    fn listed(name: &str, active: bool, hash: &str) -> ListedSieveScript {
        ListedSieveScript {
            name: name.to_owned(),
            active,
            content_hash: hash.to_owned(),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sync_keeps_ids_and_logs_changes() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        let (first, s1) = store
            .sync_sieve_scripts(
                account,
                vec![listed("a", true, "1"), listed("b", false, "2")],
            )
            .await
            .unwrap();
        assert_eq!(first.len(), 2);
        let a = first.iter().find(|r| r.name == "a").unwrap().id;
        let b = first.iter().find(|r| r.name == "b").unwrap().id;
        // Same listing: no new state.
        let (_, same) = store
            .sync_sieve_scripts(
                account,
                vec![listed("a", true, "1"), listed("b", false, "2")],
            )
            .await
            .unwrap();
        assert_eq!(same, s1);
        // b's content changes, a is deactivated, c appears.
        let (now, s2) = store
            .sync_sieve_scripts(
                account,
                vec![
                    listed("a", false, "1"),
                    listed("b", false, "3"),
                    listed("c", true, "4"),
                ],
            )
            .await
            .unwrap();
        assert!(now.iter().any(|r| r.id == a && !r.active));
        let ChangesSince::Changes(changes) = store
            .changes_since(account, DataType::SieveScript, s1, 100)
            .await
            .unwrap()
        else {
            panic!("cannot calculate");
        };
        assert_eq!(changes.new_state, s2);
        assert_eq!(changes.updated.len(), 2);
        assert_eq!(changes.created.len(), 1);
        // Renamed through the bridge: same id; then b disappears.
        store
            .rename_sieve_script(account, a, "z".to_owned())
            .await
            .unwrap();
        let (now, _) = store
            .sync_sieve_scripts(
                account,
                vec![listed("z", false, "1"), listed("c", true, "4")],
            )
            .await
            .unwrap();
        assert!(now.iter().any(|r| r.id == a && r.name == "z"));
        assert!(!now.iter().any(|r| r.id == b));
        assert_eq!(store.sieve_scripts(account).await.unwrap(), now);
        assert!(
            store
                .rename_sieve_script(account, b, "y".to_owned())
                .await
                .is_err()
        );
    }
}
