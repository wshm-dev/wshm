use anyhow::Result;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::db::Database;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookEventRow {
    pub id: i64,
    pub event_type: String,
    pub action: String,
    pub number: Option<u64>,
    pub payload: String,
    pub status: String,
    pub error: Option<String>,
    pub received_at: String,
    pub processed_at: Option<String>,
}

impl Database {
    pub fn insert_webhook_event(
        &self,
        event_type: &str,
        action: &str,
        number: Option<u64>,
        payload: &str,
    ) -> Result<i64> {
        self.with_conn(|conn| {
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO webhook_events (event_type, action, number, payload, status, received_at)
                 VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
                params![event_type, action, number, payload, now],
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    pub fn update_event_status(&self, id: i64, status: &str, error: Option<&str>) -> Result<()> {
        self.with_conn(|conn| {
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE webhook_events SET status = ?1, error = ?2, processed_at = ?3 WHERE id = ?4",
                params![status, error, now, id],
            )?;
            Ok(())
        })
    }

    pub fn pending_event_count(&self) -> Result<u64> {
        self.with_conn(|conn| {
            let count: u64 = conn.query_row(
                "SELECT COUNT(*) FROM webhook_events WHERE status = 'pending'",
                [],
                |row| row.get(0),
            )?;
            Ok(count)
        })
    }

    /// Delete processed events older than `days` days.
    pub fn cleanup_old_events(&self, days: u32) -> Result<u64> {
        self.with_conn(|conn| {
            let cutoff = chrono::Utc::now() - chrono::Duration::days(days as i64);
            let cutoff_str = cutoff.to_rfc3339();
            let deleted = conn.execute(
                "DELETE FROM webhook_events WHERE status IN ('done', 'failed') AND received_at < ?1",
                params![cutoff_str],
            )?;
            Ok(deleted as u64)
        })
    }

    pub fn get_pending_events(&self) -> Result<Vec<WebhookEventRow>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, event_type, action, number, payload, status, error, received_at, processed_at
                 FROM webhook_events WHERE status = 'pending' ORDER BY id ASC",
            )?;

            let events = stmt
                .query_map([], |row| {
                    Ok(WebhookEventRow {
                        id: row.get(0)?,
                        event_type: row.get(1)?,
                        action: row.get(2)?,
                        number: row.get(3)?,
                        payload: row.get(4)?,
                        status: row.get(5)?,
                        error: row.get(6)?,
                        received_at: row.get(7)?,
                        processed_at: row.get(8)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;

            Ok(events)
        })
    }
}
