//! Runtime key/value settings, persisted in the DB rather than a TOML file.
//!
//! wshm-pro runs as **stateless K8s pods**, so config edited at runtime (e.g.
//! the review "grand domains" list + prompt) cannot live on the pod filesystem
//! — `~/.wshm/global.toml` is seeded read-only from a ConfigMap and any local
//! write is lost on restart and not shared across replicas. This K/V table is
//! the shared source of truth every pod reads from.
//!
//! Values are opaque strings (callers store JSON for structured settings). The
//! SQLite table is single-repo (one row per key); the Postgres backend scopes
//! the same keys per-repo. See `DatabaseBackend::{get,set}_app_setting`.

use anyhow::Result;
use rusqlite::{params, OptionalExtension};

use crate::db::Database;

/// Setting key: the configured review domains as a JSON array of `DomainDef`.
pub const REVIEW_DOMAINS_KEY: &str = "review_domains";
/// Setting key: an optional custom review prompt fragment (plain string).
pub const REVIEW_PROMPT_KEY: &str = "review_prompt";
/// Setting key: how many top subjects `discover` proposes (stringified usize).
pub const REVIEW_DOMAINS_LIMIT_KEY: &str = "review_domains_limit";
/// Setting key: configured AI review skills, as a JSON array of `SkillDef`.
pub const SKILLS_KEY: &str = "skills";
/// Setting key: names of the `[[skills]]` from TOML already seeded into
/// `SKILLS_KEY` (JSON array of strings). Lets a skill deleted in the
/// dashboard stay deleted instead of being re-seeded on the next start.
pub const SKILLS_SEEDED_KEY: &str = "skills_toml_seeded";
/// Setting key: Azure DevOps work item types treated as issues (JSON array
/// of strings); overrides `[azure_devops] work_item_types`.
pub const AZURE_WORK_ITEM_TYPES_KEY: &str = "azure_work_item_types";

impl Database {
    /// Read a setting value, or `None` if unset.
    pub fn get_app_setting(&self, key: &str) -> Result<Option<String>> {
        self.with_conn(|conn| {
            let v = conn
                .query_row(
                    "SELECT value FROM app_settings WHERE key = ?1",
                    params![key],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            Ok(v)
        })
    }

    /// Upsert a setting value.
    pub fn set_app_setting(&self, key: &str, value: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO app_settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
            Ok(())
        })
    }

    /// Atomically insert a setting unless the key already exists. Returns
    /// `true` when this call created it.
    pub fn set_app_setting_if_absent(&self, key: &str, value: &str) -> Result<bool> {
        self.with_conn(|conn| {
            let n = conn.execute(
                "INSERT INTO app_settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO NOTHING",
                params![key, value],
            )?;
            Ok(n == 1)
        })
    }

    /// Atomic compare-and-swap: set `key` to `new` only if its current value
    /// is exactly `expected`. Returns `true` when the row was updated.
    pub fn compare_and_set_app_setting(
        &self,
        key: &str,
        expected: &str,
        new: &str,
    ) -> Result<bool> {
        self.with_conn(|conn| {
            let n = conn.execute(
                "UPDATE app_settings SET value = ?3 WHERE key = ?1 AND value = ?2",
                params![key, expected, new],
            )?;
            Ok(n == 1)
        })
    }
}
