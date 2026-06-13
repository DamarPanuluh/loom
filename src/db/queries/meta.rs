//! LoomMeta sentinel queries — schema version + freshness timestamps.

use anyhow::Result;
use serde::Serialize;

use crate::db::schema::{esc, label, prop};
use crate::db::LoomDb;

use super::row::{col_map, get, list_val, str_val};

/// Version + freshness + identity + custody of the graph, read from the
/// LoomMeta sentinel.
#[derive(Debug, Clone, Serialize)]
pub struct GraphMeta {
    pub version: String,
    pub created_at: String,
    /// RFC3339 of the last `loom sync`, or "" if never synced.
    pub last_synced: String,
    /// Stable identity (uuid) other looms reference; "" on pre-identity graphs
    /// until `loom init` backfills it.
    pub graph_id: String,
    /// Human name (defaults to the repo directory name at init).
    pub graph_name: String,
    /// "owned" | "observed" ("" on pre-identity graphs = owned).
    pub custody: String,
}

impl GraphMeta {
    /// True when this graph maps code its drivers do NOT own — build/fix
    /// lanes are disabled (findings, not fixes).
    pub fn observed(&self) -> bool {
        self.custody == "observed"
    }
}

pub fn get_meta(db: &dyn LoomDb) -> Result<Option<GraphMeta>> {
    let q = format!(
        "MATCH (m:{meta}) RETURN m.{v} AS v, m.{c} AS c, m.{s} AS s, \
         m.{gid} AS gid, m.{gname} AS gname, m.{cust} AS cust LIMIT 1",
        meta = label::META,
        v = prop::VERSION,
        c = prop::CREATED_AT,
        s = prop::LAST_SYNCED,
        gid = prop::GRAPH_ID,
        gname = prop::GRAPH_NAME,
        cust = prop::CUSTODY,
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    Ok(result.rows().first().map(|row| GraphMeta {
        version: str_val(get(row, &cols, "v")),
        created_at: str_val(get(row, &cols, "c")),
        last_synced: str_val(get(row, &cols, "s")),
        graph_id: str_val(get(row, &cols, "gid")),
        graph_name: str_val(get(row, &cols, "gname")),
        custody: str_val(get(row, &cols, "cust")),
    }))
}

/// Set/backfill the graph's identity + custody on the meta sentinel. Used by
/// `loom init` (fresh stamp or backfill on an older graph) and `loom import`
/// (a restore ADOPTS the exported graph's identity — it IS that graph).
pub fn set_identity(
    db: &dyn LoomDb,
    graph_id: &str,
    graph_name: &str,
    custody: &str,
) -> Result<()> {
    db.execute(&format!(
        "MATCH (m:{meta}) SET m.{gid} = '{id}', m.{gname} = '{name}', m.{cust} = '{custody}'",
        meta = label::META,
        gid = prop::GRAPH_ID,
        gname = prop::GRAPH_NAME,
        cust = prop::CUSTODY,
        id = esc(graph_id),
        name = esc(graph_name),
        custody = esc(custody),
    ))?;
    Ok(())
}

/// The custody gate: error when this graph is `observed` and the action
/// implies changing the code. Observed graphs map someone else's repo —
/// understanding, measuring, and proving still work; claiming you built or
/// fixed their code does not.
pub fn ensure_owned(db: &dyn LoomDb, action: &str) -> Result<()> {
    if let Some(m) = get_meta(db)? {
        if m.observed() {
            anyhow::bail!(
                "Custody gate: this graph OBSERVES '{}' — code its drivers don't own — so you \
                 cannot {action}. Record what you found instead: `loom edge explore … issue`, \
                 `loom rule verdict … --status failing`, or `loom note add --kind todo` \
                 (an upstream issue to hand to the owners).",
                if m.graph_name.is_empty() {
                    "this repo"
                } else {
                    &m.graph_name
                },
            );
        }
    }
    Ok(())
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

/// The declared architecture layer order, top layer first ([] = never declared).
/// This is the normative input `layering_violation` judges imports against:
/// a layer earlier in the list may depend on later ones, never the reverse.
pub fn get_layer_order(db: &dyn LoomDb) -> Result<Vec<String>> {
    let q = format!(
        "MATCH (m:{meta}) RETURN m.{p} AS o LIMIT 1",
        meta = label::META,
        p = prop::LAYER_ORDER,
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    Ok(result
        .rows()
        .first()
        .map(|row| list_val(get(row, &cols, "o")))
        .unwrap_or_default())
}

/// Declare (REPLACE) or clear (`&[]`) the architecture layer order. Atomic by
/// construction: the order is one list property on the meta sentinel —
/// there is no partial state to corrupt.
pub fn set_layer_order(db: &dyn LoomDb, order: &[String]) -> Result<()> {
    let mut p = std::collections::HashMap::new();
    p.insert("order".to_string(), super::row::list_param(order));
    db.execute_with_params(
        &format!(
            "MATCH (m:{meta}) SET m.{p} = $order",
            meta = label::META,
            p = prop::LAYER_ORDER,
        ),
        p,
    )?;
    Ok(())
}

/// The default per-target ROUTINE-transition ceiling when the graph hasn't set
/// one explicitly. Generous on purpose: it hard-bounds runaway churn (heavy
/// flappers stop accumulating dozens of flip-flop notes) while keeping enough
/// recent history that the align churn-count ranking barely shifts. Tune down
/// (toward 3) for a smaller store, or set 0 to disable.
pub const DEFAULT_TRANSITION_CAP: usize = 20;

/// The per-target ROUTINE-transition ceiling for this graph: the stored
/// `transition_cap`, or [`DEFAULT_TRANSITION_CAP`] when unset. `0` = off
/// (strict append-only — sync never compacts).
pub fn get_transition_cap(db: &dyn LoomDb) -> Result<usize> {
    let q = format!(
        "MATCH (m:{meta}) RETURN m.{p} AS c LIMIT 1",
        meta = label::META,
        p = prop::TRANSITION_CAP,
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    // Absent (older graphs) → the default; an explicit value (incl. 0) wins.
    Ok(result
        .rows()
        .first()
        .map(|row| {
            let raw = str_val(get(row, &cols, "c"));
            if raw.is_empty() {
                DEFAULT_TRANSITION_CAP
            } else {
                raw.parse::<usize>().unwrap_or(DEFAULT_TRANSITION_CAP)
            }
        })
        .unwrap_or(DEFAULT_TRANSITION_CAP))
}

/// Persist the per-target ROUTINE-transition ceiling. `0` disables compaction.
pub fn set_transition_cap(db: &dyn LoomDb, cap: usize) -> Result<()> {
    let cap_s = cap.to_string();
    db.execute_with_params(
        &format!(
            "MATCH (m:{meta}) SET m.{p} = $cap",
            meta = label::META,
            p = prop::TRANSITION_CAP,
        ),
        super::row::sparams(&[("cap", cap_s.as_str())]),
    )?;
    Ok(())
}

/// Legacy v5 property: product domains doubled as architecture layers.
/// New writes must use `layer_order`; this is read only for migration/import
/// compatibility.
pub fn get_legacy_domain_order(db: &dyn LoomDb) -> Result<Vec<String>> {
    let q = format!(
        "MATCH (m:{meta}) RETURN m.{p} AS o LIMIT 1",
        meta = label::META,
        p = prop::DOMAIN_ORDER,
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    Ok(result
        .rows()
        .first()
        .map(|row| list_val(get(row, &cols, "o")))
        .unwrap_or_default())
}
