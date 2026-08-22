use crate::model::*;
use crate::Result;
use rusqlite::Connection;

/// Sentinel timestamp for derived rows. Derived data is recomputed by sync, so
/// its creation time is meaningless; a fixed sentinel keeps wipe+rebuild output
/// byte-identical (INV-2).
pub(crate) const DERIVED_TS: &str = "";

/// Deterministic FNV-1a 64-bit digest over the joined parts. Returns the bare
/// 16-hex digest (no prefix). Callers choose a plane prefix (`d` for derived
/// rows, `c` for debt clusters, `p` for promoted findings).
pub fn fnv_hex_digest(parts: &[&str]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h ^= 0x1f;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        for b in p.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

/// Deterministic, content-addressed id for derived data (FNV-1a 64-bit over the
/// joined parts). The same inputs always yield the same id, so a wiped-and-
/// rebuilt derived plane is byte-identical.
pub(crate) fn derived_id(parts: &[&str]) -> String {
    format!("d{}", fnv_hex_digest(parts))
}

/// Whether an id has the content-addressed shape used by derived nodes.
/// Centralized because dormant derived-Finding adjudications are the sole
/// intentional missing-subject reference in restore and audit.
pub(crate) fn is_derived_node_id(id: &str) -> bool {
    id.strip_prefix('d').is_some_and(|digest| {
        digest.len() == 16
            && digest
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    })
}

/// Generate a fresh 128-bit hex id and an RFC3339 timestamp in one query, using
/// SQLite's own functions (no external rng/clock crate).
pub(crate) fn id_and_now(conn: &Connection) -> Result<(String, String)> {
    conn.query_row(
        "SELECT lower(hex(randomblob(16))), strftime('%Y-%m-%dT%H:%M:%fZ','now')",
        [],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )
    .map_err(Into::into)
}

pub(crate) fn now(conn: &Connection) -> Result<String> {
    conn.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |r| {
        r.get::<_, String>(0)
    })
    .map_err(Into::into)
}

pub(crate) fn parse_named<T: std::str::FromStr>(
    row: &rusqlite::Row,
    col: &str,
) -> rusqlite::Result<T>
where
    T::Err: std::fmt::Display,
{
    let s: String = row.get(col)?;
    s.parse().map_err(|e: T::Err| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            e.to_string().into(),
        )
    })
}

pub(crate) fn parse_col<T: std::str::FromStr>(
    row: &rusqlite::Row,
    idx: usize,
) -> rusqlite::Result<T>
where
    T::Err: std::fmt::Display,
{
    let s: String = row.get(idx)?;
    s.parse().map_err(|e: T::Err| {
        rusqlite::Error::FromSqlConversionFailure(
            idx,
            rusqlite::types::Type::Text,
            e.to_string().into(),
        )
    })
}

/// Column list for node SELECTs. Order-independent (mappers read by name) but
/// kept as one constant so every query selects the full row.
pub(crate) const NODE_COLS: &str =
    "id,node_type,name,description,status,truth_class,body,created_at,updated_at";

/// Column list for edge SELECTs. Read from `edge_view`, never `edge`: the
/// verdict fields (`criterion`, `confidence`, `inspected_by`) are PROJECTIONS of
/// the edge's `verdict` fact, so an edge row cannot disagree with the fact that
/// justifies it — there is no column to write them into.
pub(crate) const EDGE_COLS: &str = "id,from_id,to_id,kind,truth_class,status,criterion,\
                         confidence,depends_on,inspected_by,created_at,updated_at";

pub(crate) fn row_to_node(r: &rusqlite::Row) -> rusqlite::Result<Node> {
    let body_str: String = r.get("body")?;
    Ok(Node {
        id: r.get("id")?,
        node_type: parse_named(r, "node_type")?,
        name: r.get("name")?,
        description: r.get("description")?,
        status: r.get("status")?,
        truth_class: parse_named(r, "truth_class")?,
        body: serde_json::from_str(&body_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
    })
}

pub(crate) fn row_to_edge(r: &rusqlite::Row) -> rusqlite::Result<Edge> {
    let depends_str: String = r.get("depends_on")?;
    Ok(Edge {
        id: r.get("id")?,
        from_id: r.get("from_id")?,
        to_id: r.get("to_id")?,
        kind: parse_named(r, "kind")?,
        truth_class: parse_named(r, "truth_class")?,
        status: parse_named(r, "status")?,
        criterion: r.get("criterion")?,
        confidence: r.get("confidence")?,
        depends_on: serde_json::from_str(&depends_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?,
        inspected_by: r.get("inspected_by")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_ids_require_lowercase_hex() {
        assert!(is_derived_node_id("d0123456789abcdef"));
        assert!(!is_derived_node_id("d0123456789abcdeF"));
        assert!(!is_derived_node_id("d0123456789ABCDE"));
    }
}
