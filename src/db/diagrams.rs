use anyhow::Result;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::db::Database;

/// Cached PR diagram (`pr_diagrams`): the validated graph JSON for one PR,
/// keyed by the diff it was built from, plus where its interactive page was
/// published. Regenerated only when `diff_hash` or `gen_hash` changes.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PrDiagramRow {
    pub pr_number: u64,
    /// sha256 of the raw PR diff the graph was built from.
    pub diff_hash: String,
    pub head_sha: Option<String>,
    /// Fingerprint of the generation settings (kinds, text, language,
    /// prompt version): changing them regenerates the graph.
    pub gen_hash: String,
    pub graph_json: String,
    /// Publish target of the page (`local`, `github-pages`, `wshm`), if any.
    pub page_target: Option<String>,
    /// Object key / file path of the page under the target.
    pub page_key: Option<String>,
    pub page_url: Option<String>,
    /// Unguessable token of the `wshm` hosted page.
    pub page_token: Option<String>,
    /// Size of the last rendered page (bytes), for the dashboard. The page
    /// itself is never stored: `wshm` renders it on request from the graph.
    pub page_bytes: u64,
    /// Fingerprint of the page-rendering settings of a static page (viewer
    /// version, language, diff cap): a change republishes the file.
    pub page_hash: Option<String>,
    /// The repo was private when the page was published.
    pub is_private: bool,
    pub created_at: String,
    pub updated_at: String,
}

const COLUMNS: &str = "pr_number, diff_hash, head_sha, gen_hash, graph_json, page_target, \
     page_key, page_url, page_token, page_bytes, is_private, created_at, updated_at, page_hash";

fn row_to_diagram(row: &rusqlite::Row) -> rusqlite::Result<PrDiagramRow> {
    Ok(PrDiagramRow {
        pr_number: row.get(0)?,
        diff_hash: row.get(1)?,
        head_sha: row.get(2)?,
        gen_hash: row.get(3)?,
        graph_json: row.get(4)?,
        page_target: row.get(5)?,
        page_key: row.get(6)?,
        page_url: row.get(7)?,
        page_token: row.get(8)?,
        page_bytes: row.get::<_, i64>(9)?.max(0) as u64,
        is_private: row.get::<_, i64>(10)? != 0,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
        page_hash: row.get(13)?,
    })
}

impl Database {
    pub fn get_pr_diagram(&self, pr_number: u64) -> Result<Option<PrDiagramRow>> {
        self.with_conn(|conn| {
            Ok(conn
                .query_row(
                    &format!("SELECT {COLUMNS} FROM pr_diagrams WHERE pr_number = ?1"),
                    params![pr_number],
                    row_to_diagram,
                )
                .optional()?)
        })
    }

    pub fn list_pr_diagrams(&self) -> Result<Vec<PrDiagramRow>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM pr_diagrams"))?;
            let rows = stmt
                .query_map([], row_to_diagram)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn upsert_pr_diagram(&self, row: &PrDiagramRow) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO pr_diagrams (pr_number, diff_hash, head_sha, gen_hash, graph_json,
                    page_target, page_key, page_url, page_token, page_bytes, is_private,
                    created_at, updated_at, page_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                 ON CONFLICT(pr_number) DO UPDATE SET
                    diff_hash = excluded.diff_hash,
                    head_sha = excluded.head_sha,
                    gen_hash = excluded.gen_hash,
                    graph_json = excluded.graph_json,
                    page_target = excluded.page_target,
                    page_key = excluded.page_key,
                    page_url = excluded.page_url,
                    page_token = excluded.page_token,
                    page_bytes = excluded.page_bytes,
                    is_private = excluded.is_private,
                    updated_at = excluded.updated_at,
                    page_hash = excluded.page_hash",
                params![
                    row.pr_number,
                    row.diff_hash,
                    row.head_sha,
                    row.gen_hash,
                    row.graph_json,
                    row.page_target,
                    row.page_key,
                    row.page_url,
                    row.page_token,
                    row.page_bytes as i64,
                    row.is_private as i64,
                    row.created_at,
                    row.updated_at,
                    row.page_hash,
                ],
            )?;
            Ok(())
        })
    }

    pub fn delete_pr_diagram(&self, pr_number: u64) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM pr_diagrams WHERE pr_number = ?1",
                params![pr_number],
            )?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_diagram_roundtrip() {
        let db = Database::open_memory().unwrap();
        assert!(db.get_pr_diagram(7).unwrap().is_none());
        let mut row = PrDiagramRow {
            pr_number: 7,
            diff_hash: "d1".into(),
            gen_hash: "g1".into(),
            graph_json: "{}".into(),
            page_target: Some("wshm".into()),
            page_bytes: 1234,
            is_private: true,
            created_at: "t0".into(),
            updated_at: "t0".into(),
            ..Default::default()
        };
        db.upsert_pr_diagram(&row).unwrap();
        assert_eq!(db.get_pr_diagram(7).unwrap().unwrap(), row);
        row.diff_hash = "d2".into();
        row.updated_at = "t1".into();
        db.upsert_pr_diagram(&row).unwrap();
        let got = db.get_pr_diagram(7).unwrap().unwrap();
        assert_eq!(got.diff_hash, "d2");
        assert_eq!(got.created_at, "t0");
        let listed = db.list_pr_diagrams().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].page_bytes, 1234);
        db.delete_pr_diagram(7).unwrap();
        assert!(db.get_pr_diagram(7).unwrap().is_none());
    }
}
