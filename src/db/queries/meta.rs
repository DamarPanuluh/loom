//! LoomMeta sentinel queries — schema version + freshness timestamps.

use anyhow::Result;
use serde::Serialize;

use crate::db::schema::{esc, label, prop};
use crate::db::LoomDb;

use super::row::{col_map, get, str_val};

/// Version + freshness of the graph, read from the LoomMeta sentinel.
#[derive(Debug, Clone, Serialize)]
pub struct GraphMeta {
    pub version: String,
    pub created_at: String,
    /// RFC3339 of the last `loom sync`, or "" if never synced.
    pub last_synced: String,
}

pub fn get_meta(db: &dyn LoomDb) -> Result<Option<GraphMeta>> {
    let q = format!(
        "MATCH (m:{meta}) RETURN m.{v} AS v, m.{c} AS c, m.{s} AS s LIMIT 1",
        meta = label::META,
        v = prop::VERSION,
        c = prop::CREATED_AT,
        s = prop::LAST_SYNCED,
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    Ok(result.rows().first().map(|row| GraphMeta {
        version:     str_val(get(row, &cols, "v")),
        created_at:  str_val(get(row, &cols, "c")),
        last_synced: str_val(get(row, &cols, "s")),
    }))
}

/// Stamp the graph as reconciled against disk (called by `loom sync`).
pub fn set_last_synced(db: &dyn LoomDb, now: &str) -> Result<()> {
    db.execute(&format!(
        "MATCH (m:{meta}) SET m.{s} = '{now}'",
        meta = label::META,
        s = prop::LAST_SYNCED,
        now = esc(now),
    ))?;
    Ok(())
}
