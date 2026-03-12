use anyhow::Result;
use rusqlite::params;

use crate::db::Database;

pub struct SyncEntry {
    pub table_name: String,
    pub last_synced_at: String,
    pub etag: Option<String>,
}

impl Database {
    pub fn get_sync_entry(&self, table_name: &str) -> Result<Option<SyncEntry>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT table_name, last_synced_at, etag FROM sync_log WHERE table_name = ?1",
            )?;

            let result = stmt.query_row(params![table_name], |row| {
                Ok(SyncEntry {
                    table_name: row.get(0)?,
                    last_synced_at: row.get(1)?,
                    etag: row.get(2)?,
                })
            });

            match result {
                Ok(entry) => Ok(Some(entry)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e.into()),
            }
        })
    }

    pub fn update_sync_entry(
        &self,
        table_name: &str,
        last_synced_at: &str,
        etag: Option<&str>,
    ) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO sync_log (table_name, last_synced_at, etag)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(table_name) DO UPDATE SET
                    last_synced_at = excluded.last_synced_at,
                    etag = excluded.etag",
                params![table_name, last_synced_at, etag],
            )?;
            Ok(())
        })
    }
}
