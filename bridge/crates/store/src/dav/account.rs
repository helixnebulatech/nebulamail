// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! What discovery found for an account.

use diesel::{
    ExpressionMethods as _, OptionalExtension as _, QueryDsl as _, Queryable, Selectable,
    SelectableHelper as _,
};
use diesel_async::RunQueryDsl as _;

use super::types::{DavAccount, DavHref};
use crate::schema::dav_accounts;
use crate::{AccountId, Store, StoreError, UnixTime};

#[derive(Queryable, Selectable)]
#[diesel(table_name = dav_accounts, check_for_backend(diesel::sqlite::Sqlite))]
struct DavAccountRow {
    principal: String,
    calendar_homes: String,
    addressbook_homes: String,
    schedule_outbox: Option<String>,
    user_addresses: String,
    auto_schedule: bool,
    discovered_at: i64,
}

fn hrefs_to_db(hrefs: &[DavHref]) -> Result<String, StoreError> {
    let texts: Vec<&str> = hrefs.iter().map(DavHref::as_str).collect();
    serde_json::to_string(&texts).map_err(|_| StoreError::Invalid("href list"))
}

fn hrefs_from_db(json: &str) -> Result<Vec<DavHref>, StoreError> {
    let texts: Vec<String> =
        serde_json::from_str(json).map_err(|_| StoreError::Corrupt("href list"))?;
    texts.into_iter().map(DavHref::from_db).collect()
}

impl TryFrom<DavAccountRow> for DavAccount {
    type Error = StoreError;

    fn try_from(row: DavAccountRow) -> Result<Self, StoreError> {
        Ok(Self {
            principal: DavHref::from_db(row.principal)?,
            calendar_homes: hrefs_from_db(&row.calendar_homes)?,
            addressbook_homes: hrefs_from_db(&row.addressbook_homes)?,
            schedule_outbox: row.schedule_outbox.map(DavHref::from_db).transpose()?,
            user_addresses: serde_json::from_str(&row.user_addresses)
                .map_err(|_| StoreError::Corrupt("user addresses"))?,
            auto_schedule: row.auto_schedule,
            discovered_at: UnixTime::from_secs(row.discovered_at),
        })
    }
}

impl Store {
    /// What the last discovery found for the account, if any.
    ///
    /// # Errors
    /// On database errors.
    pub async fn dav_account(&self, account: AccountId) -> Result<Option<DavAccount>, StoreError> {
        let mut conn = self.conn().await?;
        dav_accounts::table
            .find(account.get())
            .select(DavAccountRow::as_select())
            .first(&mut conn)
            .await
            .optional()?
            .map(DavAccount::try_from)
            .transpose()
    }

    /// Stores (or replaces) what discovery found.
    ///
    /// # Errors
    /// On database errors, including an unknown account.
    pub async fn put_dav_account(
        &self,
        account: AccountId,
        found: &DavAccount,
    ) -> Result<(), StoreError> {
        let calendar_homes = hrefs_to_db(&found.calendar_homes)?;
        let addressbook_homes = hrefs_to_db(&found.addressbook_homes)?;
        let user_addresses = serde_json::to_string(&found.user_addresses)
            .map_err(|_| StoreError::Invalid("user addresses"))?;
        let outbox = found.schedule_outbox.as_ref().map(DavHref::as_str);
        let values = (
            dav_accounts::principal.eq(found.principal.as_str()),
            dav_accounts::calendar_homes.eq(&calendar_homes),
            dav_accounts::addressbook_homes.eq(&addressbook_homes),
            dav_accounts::schedule_outbox.eq(outbox),
            dav_accounts::user_addresses.eq(&user_addresses),
            dav_accounts::auto_schedule.eq(found.auto_schedule),
            dav_accounts::discovered_at.eq(found.discovered_at.secs()),
        );
        let mut conn = self.conn().await?;
        diesel::insert_into(dav_accounts::table)
            .values((dav_accounts::account_id.eq(account.get()), values))
            .on_conflict(dav_accounts::account_id)
            .do_update()
            .set(values)
            .execute(&mut conn)
            .await?;
        Ok(())
    }
}
