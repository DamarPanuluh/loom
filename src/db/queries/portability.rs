//! Graph portability: deterministic JSON export / import.
//!
//! The grafeo file in `.loom/` is binary and per-machine; the export is the
//! graph's *travel format* — committed to git, diffable in PRs (intent and
//! verdict changes reviewable next to code changes), and rebuildable anywhere
//! with `loom import`. Schema-driven: the property lists come from
//! `db::schema`, so export/import can't drift from the vocabulary.

use anyhow::Result;
use serde_json::{json, Map, Value as J};

use crate::db::schema::{self, edge, label, prop, EDGE_TYPES, NODE_LABELS};
use crate::db::LoomDb;

use super::row::col_map;

/// Properties exported per node label: the required set plus additive extras
/// that aren't in the required table (kept out of it so older graphs stay
/// doctor-clean until their next sync).
fn node_props(lbl: &str) -> Vec<&'static str> {
    let mut props: Vec<&'static str> =
        schema::required_node_props(lbl).iter().map(|(p, _)| *p).collect();
    if lbl == label::CODE_FILE {
        props.push(prop::IMPORTS);
        props.push(prop::CONTENT_HASH);
    }
    props
}

fn edge_props(etype: &str) -> Vec<&'static str> {
    schema::required_edge_props(etype).iter().map(|(p, _)| *p).collect()
}

/// Endpoint node labels per edge type (from → to).
fn endpoints(etype: &str) -> (&'static str, &'static str) {
    match etype {
        edge::IMPLEMENTS => (label::INTENT, label::CODE_FILE),
        edge::GOVERNS => (label::QUALITY_RULE, label::INTENT),
        edge::VALIDATES => (label::VALIDATION, label::INTENT),
        _ => (label::INTENT, label::INTENT), // RELATES_TO, HIERARCHY
    }
}

/// Numeric edge properties (everything else is a string).
fn is_numeric(p: &str) -> bool {
    p == prop::CONFIDENCE || p == prop::PRIORITY_SCORE
}

fn grafeo_to_json(v: &grafeo::Value, numeric: bool) -> J {
    // Render through the shared row helpers so NULL becomes ""/0.0 uniformly.
    if numeric {
        json!(super::row::f64_val(v))
    } else {
        json!(super::row::str_val(v))
    }
}

/// Export the whole graph as deterministic JSON (arrays sorted by id; map keys
/// sorted by serde_json). Same graph → byte-identical export.
pub fn export_graph(db: &dyn LoomDb) -> Result<J> {
    let mut nodes = Map::new();
    for &lbl in NODE_LABELS {
        let props = node_props(lbl);
        let select = props.iter().map(|p| format!("n.{p}")).collect::<Vec<_>>().join(", ");
        let result = db.execute(&format!("MATCH (n:{lbl}) RETURN {select}"))?;
        let cols = col_map(&result);
        let mut items: Vec<J> = Vec::new();
        for row in result.rows() {
            let mut obj = Map::new();
            for p in &props {
                let v = super::row::get(row, &cols, &format!("n.{p}"));
                obj.insert((*p).to_string(), grafeo_to_json(v, false));
            }
            items.push(J::Object(obj));
        }
        items.sort_by_key(|i| i["id"].as_str().unwrap_or_default().to_string());
        nodes.insert(lbl.to_string(), J::Array(items));
    }

    let mut edges = Map::new();
    for &etype in EDGE_TYPES {
        let props = edge_props(etype);
        let (la, lb) = endpoints(etype);
        let select = props.iter().map(|p| format!("r.{p}")).collect::<Vec<_>>().join(", ");
        let result = db.execute(&format!(
            "MATCH (a:{la})-[r:{etype}]->(b:{lb}) RETURN a.id AS __from, b.id AS __to, {select}"
        ))?;
        let cols = col_map(&result);
        let mut items: Vec<J> = Vec::new();
        for row in result.rows() {
            let mut obj = Map::new();
            obj.insert("from".into(), grafeo_to_json(super::row::get(row, &cols, "__from"), false));
            obj.insert("to".into(), grafeo_to_json(super::row::get(row, &cols, "__to"), false));
            for p in &props {
                let v = super::row::get(row, &cols, &format!("r.{p}"));
                obj.insert((*p).to_string(), grafeo_to_json(v, is_numeric(p)));
            }
            items.push(J::Object(obj));
        }
        items.sort_by_key(|i| {
            format!(
                "{}|{}|{}",
                i["from"].as_str().unwrap_or_default(),
                i["to"].as_str().unwrap_or_default(),
                i["id"].as_str().unwrap_or_default()
            )
        });
        edges.insert(etype.to_string(), J::Array(items));
    }

    // The graph's identity travels with it — other looms reference this
    // (graph_id) and readers know whose testimony the export is (custody).
    let meta = super::meta::get_meta(db)?;
    let (gid, gname, custody) = meta
        .map(|m| (m.graph_id, m.graph_name, m.custody))
        .unwrap_or_default();

    Ok(json!({
        "loom_export": 1,
        "schema_version": schema::SCHEMA_VERSION,
        "graph_id": gid,
        "graph_name": gname,
        "custody": custody,
        "nodes": nodes,
        "edges": edges,
    }))
}

/// Counts of what an import rebuilt.
#[derive(Debug, Default)]
pub struct ImportReport {
    pub nodes: usize,
    pub edges: usize,
}

/// Rebuild a graph from an export. The target graph must be content-empty
/// (fresh `loom init`) — import is restoration, not merge.
pub fn import_graph(db: &dyn LoomDb, data: &J) -> Result<ImportReport> {
    if data.get("loom_export").and_then(J::as_i64) != Some(1) {
        anyhow::bail!("Not a loom export (missing/unknown `loom_export` marker).");
    }
    let ver = data.get("schema_version").and_then(J::as_str).unwrap_or("");
    if ver != schema::SCHEMA_VERSION {
        anyhow::bail!(
            "Export schema version '{}' does not match this loom ('{}').",
            ver, schema::SCHEMA_VERSION
        );
    }
    for &lbl in NODE_LABELS {
        let r = db.execute(&format!("MATCH (n:{lbl}) RETURN count(n) AS c"))?;
        let n = r.rows().first().map(|row| super::row::i64_val(&row[0])).unwrap_or(0);
        if n > 0 {
            anyhow::bail!(
                "Graph already contains {lbl} nodes — import only restores into a fresh `loom init`."
            );
        }
    }

    // A restore IS the exported graph — adopt its identity + custody (the
    // fresh init's identity was a placeholder). Older exports without one
    // keep the fresh identity.
    let gid = data.get("graph_id").and_then(J::as_str).unwrap_or("");
    if !gid.is_empty() {
        super::meta::set_identity(
            db,
            gid,
            data.get("graph_name").and_then(J::as_str).unwrap_or(""),
            data.get("custody").and_then(J::as_str).unwrap_or("owned"),
        )?;
    }

    let mut report = ImportReport::default();
    let empty = Map::new();

    // Nodes first.
    for &lbl in NODE_LABELS {
        let items = data["nodes"].get(lbl).and_then(J::as_array).cloned().unwrap_or_default();
        for item in items {
            let obj = item.as_object().unwrap_or(&empty);
            let assigns = node_props(lbl)
                .iter()
                .map(|p| {
                    let v = obj.get(*p).and_then(J::as_str).unwrap_or("");
                    format!("{p}: '{}'", schema::esc(v))
                })
                .collect::<Vec<_>>()
                .join(", ");
            db.execute(&format!("INSERT (:{lbl} {{{assigns}}})"))?;
            report.nodes += 1;
        }
    }

    // Then edges, endpoint-matched.
    for &etype in EDGE_TYPES {
        let (la, lb) = endpoints(etype);
        let items = data["edges"].get(etype).and_then(J::as_array).cloned().unwrap_or_default();
        for item in items {
            let obj = item.as_object().unwrap_or(&empty);
            let from = obj.get("from").and_then(J::as_str).unwrap_or("");
            let to = obj.get("to").and_then(J::as_str).unwrap_or("");
            let assigns = edge_props(etype)
                .iter()
                .map(|p| {
                    if is_numeric(p) {
                        format!("{p}: {}", obj.get(*p).and_then(J::as_f64).unwrap_or(0.0))
                    } else {
                        let v = obj.get(*p).and_then(J::as_str).unwrap_or("");
                        format!("{p}: '{}'", schema::esc(v))
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            db.execute(&format!(
                "MATCH (a:{la} {{id: '{from}'}}), (b:{lb} {{id: '{to}'}}) \
                 INSERT (a)-[:{etype} {{{assigns}}}]->(b)",
                from = schema::esc(from),
                to = schema::esc(to),
            ))?;
            report.edges += 1;
        }
    }
    Ok(report)
}
