//! Graph portability: deterministic JSON export / import.
//!
//! The grafeo file in `.loom/` is binary and per-machine; the export is the
//! graph's *travel format* — committed to git, diffable in PRs (intent and
//! verdict changes reviewable next to code changes), and rebuildable anywhere
//! with `loom import`. Schema-driven: the property lists come from
//! `db::schema`, so export/import can't drift from the vocabulary.

use anyhow::{Context, Result};
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
    if lbl == label::QUALITY_RULE {
        props.push(prop::INSPECTION_EFFORT);
    }
    if lbl == label::NOTE {
        props.push(prop::AUDIENCE);
    }
    props
}

/// Props that are ADDITIVE (absent on exports from older binaries): the import
/// reads them as "" instead of failing, so old committed exports keep
/// restoring. Everything else stays strictly required — A4's loud-rejection
/// guarantee is about corruption, not about honest schema growth.
fn is_optional_prop(p: &str) -> bool {
    matches!(
        p,
        x if x == prop::IMPORTS
            || x == prop::CONTENT_HASH
            || x == prop::INSPECTION_EFFORT
            || x == prop::AUDIENCE
    )
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
        edge::TARGETS => (label::HYPOTHESIS, label::INTENT),
        _ => (label::INTENT, label::INTENT), // RELATES_TO, HIERARCHY
    }
}

/// Sections ADDITIVE to schema v3 (the hypothesis plane): exports from older
/// binaries don't have them, so import reads a missing section as empty —
/// same growth contract as `is_optional_prop`, at section granularity.
fn is_additive_node_section(lbl: &str) -> bool {
    lbl == label::HYPOTHESIS
}

fn is_additive_edge_section(etype: &str) -> bool {
    etype == edge::TARGETS
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

fn object_field<'a>(data: &'a J, field: &str) -> Result<&'a Map<String, J>> {
    data.get(field)
        .with_context(|| format!("Export is missing `{field}` object"))?
        .as_object()
        .with_context(|| format!("Export `{field}` is not an object"))
}

fn array_in_object<'a>(
    obj: &'a Map<String, J>,
    section: &str,
    key: &str,
) -> Result<&'a Vec<J>> {
    obj.get(key)
        .with_context(|| format!("Export is missing `{section}.{key}` array"))?
        .as_array()
        .with_context(|| format!("Export `{section}.{key}` is not an array"))
}

fn item_object<'a>(item: &'a J, ctx: &str) -> Result<&'a Map<String, J>> {
    item.as_object()
        .with_context(|| format!("Export `{ctx}` item is not an object"))
}

fn required_str<'a>(obj: &'a Map<String, J>, key: &str, ctx: &str) -> Result<&'a str> {
    obj.get(key)
        .with_context(|| format!("Export `{ctx}` is missing string field `{key}`"))?
        .as_str()
        .with_context(|| format!("Export `{ctx}.{key}` is not a string"))
}

fn required_f64(obj: &Map<String, J>, key: &str, ctx: &str) -> Result<f64> {
    obj.get(key)
        .with_context(|| format!("Export `{ctx}` is missing numeric field `{key}`"))?
        .as_f64()
        .with_context(|| format!("Export `{ctx}.{key}` is not a number"))
}

/// Export the whole graph as deterministic JSON (arrays sorted by id; map keys
/// sorted by serde_json). Same graph → byte-identical export.
pub fn export_graph(db: &dyn LoomDb) -> Result<J> {
    let mut nodes = Map::new();
    for &lbl in NODE_LABELS {
        let props = node_props(lbl);
        let prop_cols = props.iter().map(|p| format!("n.{p}")).collect::<Vec<_>>();
        let select = prop_cols.join(", ");
        let result = db.execute(&format!("MATCH (n:{lbl}) RETURN {select}"))?;
        let cols = col_map(&result);
        let mut items: Vec<J> = Vec::new();
        for row in result.rows() {
            let mut obj = Map::new();
            for (p, col) in props.iter().zip(prop_cols.iter()) {
                let v = super::row::get(row, &cols, col);
                obj.insert((*p).to_string(), grafeo_to_json(v, false));
            }
            items.push(J::Object(obj));
        }
        items.sort_by(|a, b| {
            a["id"].as_str().unwrap_or_default().cmp(b["id"].as_str().unwrap_or_default())
        });
        nodes.insert(lbl.to_string(), J::Array(items));
    }

    let mut edges = Map::new();
    for &etype in EDGE_TYPES {
        let props = edge_props(etype);
        let (la, lb) = endpoints(etype);
        let prop_cols = props.iter().map(|p| format!("r.{p}")).collect::<Vec<_>>();
        let select = prop_cols.join(", ");
        let result = db.execute(&format!(
            "MATCH (a:{la})-[r:{etype}]->(b:{lb}) RETURN a.id AS __from, b.id AS __to, {select}"
        ))?;
        let cols = col_map(&result);
        let mut items: Vec<J> = Vec::new();
        for row in result.rows() {
            let mut obj = Map::new();
            obj.insert("from".into(), grafeo_to_json(super::row::get(row, &cols, "__from"), false));
            obj.insert("to".into(), grafeo_to_json(super::row::get(row, &cols, "__to"), false));
            for (p, col) in props.iter().zip(prop_cols.iter()) {
                let v = super::row::get(row, &cols, col);
                obj.insert((*p).to_string(), grafeo_to_json(v, is_numeric(p)));
            }
            items.push(J::Object(obj));
        }
        items.sort_by(|a, b| {
            let ak = (
                a["from"].as_str().unwrap_or_default(),
                a["to"].as_str().unwrap_or_default(),
                a["id"].as_str().unwrap_or_default(),
            );
            let bk = (
                b["from"].as_str().unwrap_or_default(),
                b["to"].as_str().unwrap_or_default(),
                b["id"].as_str().unwrap_or_default(),
            );
            ak.cmp(&bk)
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
    /// Nodes/edges deliberately not carried over by `--as-planned` (groundings,
    /// codefiles, old-repo ignore/delegation patterns).
    pub skipped_nodes: usize,
    pub skipped_edges: usize,
}

/// Rebuild a graph from an export. The target graph must be content-empty
/// (fresh `loom init`) — import is restoration, not merge.
///
/// `as_planned` is the PORTING mode: the semantic plane travels, the physical
/// plane is rebuilt. Intents/hierarchy/criteria/rules/notes are adopted;
/// CodeFiles, IMPLEMENTS groundings, and old-repo Ignore/Delegation patterns
/// are dropped (they describe the SOURCE repo's disk); every intent arrives
/// lifecycle=planned; every verdict meta is reset to uninspected (it was
/// earned against the old code — criteria stay as the acceptance contract);
/// every validation arrives not_run (its command is a spec to re-express).
/// The source graph's identity is NOT adopted — a port is a new graph.
pub fn import_graph(db: &dyn LoomDb, data: &J, as_planned: bool) -> Result<ImportReport> {
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

    // TWO-PHASE import: phase 1 validates EVERY node and edge and builds the
    // full statement list with NO database writes; phase 2 executes. A
    // malformed item at row 500 therefore rejects the whole file loudly and
    // leaves the fresh graph untouched — a hostile/corrupted export can never
    // half-import (identity adoption is also deferred past validation).
    let mut report = ImportReport::default();
    let nodes = object_field(data, "nodes")?;
    let edges = object_field(data, "edges")?;
    let mut stmts: Vec<String> = Vec::new();

    // Porting drops the physical plane + repo-specific patterns…
    let skip_label = |lbl: &str| {
        as_planned && matches!(lbl, label::CODE_FILE | label::IGNORE | label::DELEGATION)
    };
    let skip_edge = |etype: &str| as_planned && etype == edge::IMPLEMENTS;
    // …and resets what was EARNED against the old code, keeping what was
    // DESIGNED (criterion, notes — the contract travels; the proof doesn't).
    let node_override = |lbl: &str, prop: &str, v: &str| -> Option<String> {
        if !as_planned {
            return None;
        }
        match (lbl, prop) {
            (label::INTENT, "lifecycle") => Some("planned".into()),
            (label::VALIDATION, "last_result") => Some("not_run".into()),
            (label::VALIDATION, "last_run") => Some(String::new()),
            // Hypotheses travel (they are design lineage), but a supported/
            // refuted verdict was earned against the OLD code — re-prove in
            // the new repo. Adopted/rejected are decisions and stay history.
            (label::HYPOTHESIS, "status") if matches!(v, "supported" | "refuted") => {
                Some("proposed".into())
            }
            (label::HYPOTHESIS, "evidence" | "inspected_by" | "last_inspected") => {
                Some(String::new())
            }
            _ => None,
        }
    };
    let edge_override = |prop: &str| -> Option<&'static str> {
        if !as_planned {
            return None;
        }
        match prop {
            "inspection_status" => Some("uninspected"),
            "evidence" | "last_inspected" | "inspected_by" => Some(""),
            "confidence" | "priority_score" => Some("0"),
            _ => None,
        }
    };

    // Phase 1a: nodes.
    for &lbl in NODE_LABELS {
        if is_additive_node_section(lbl) && !nodes.contains_key(lbl) {
            continue; // older export — the section simply doesn't exist yet
        }
        let items = array_in_object(nodes, "nodes", lbl)?;
        if skip_label(lbl) {
            report.skipped_nodes += items.len();
            continue;
        }
        for item in items {
            let obj = item_object(item, &format!("nodes.{lbl}"))?;
            let assigns = node_props(lbl)
                .iter()
                .map(|p| {
                    let v = if is_optional_prop(p) {
                        obj.get(*p).and_then(J::as_str).unwrap_or("")
                    } else {
                        required_str(obj, p, &format!("nodes.{lbl}"))?
                    };
                    let v = node_override(lbl, p, v).unwrap_or_else(|| v.to_string());
                    Ok(format!("{p}: '{}'", schema::esc(&v)))
                })
                .collect::<Result<Vec<_>>>()?
                .join(", ");
            stmts.push(format!("INSERT (:{lbl} {{{assigns}}})"));
            report.nodes += 1;
        }
    }

    // Phase 1b: edges, endpoint-matched.
    for &etype in EDGE_TYPES {
        if is_additive_edge_section(etype) && !edges.contains_key(etype) {
            continue;
        }
        let (la, lb) = endpoints(etype);
        let items = array_in_object(edges, "edges", etype)?;
        if skip_edge(etype) {
            report.skipped_edges += items.len();
            continue;
        }
        for item in items {
            let obj = item_object(item, &format!("edges.{etype}"))?;
            let from = required_str(obj, "from", &format!("edges.{etype}"))?;
            let to = required_str(obj, "to", &format!("edges.{etype}"))?;
            let assigns = edge_props(etype)
                .iter()
                .map(|p| {
                    if is_numeric(p) {
                        let v = required_f64(obj, p, &format!("edges.{etype}"))?;
                        let v = edge_override(p).map(|o| o.to_string()).unwrap_or_else(|| v.to_string());
                        Ok(format!("{p}: {v}"))
                    } else {
                        let v = required_str(obj, p, &format!("edges.{etype}"))?;
                        let v = edge_override(p).unwrap_or(v);
                        Ok(format!("{p}: '{}'", schema::esc(v)))
                    }
                })
                .collect::<Result<Vec<_>>>()?
                .join(", ");
            stmts.push(format!(
                "MATCH (a:{la} {{id: '{from}'}}), (b:{lb} {{id: '{to}'}}) \
                 INSERT (a)-[:{etype} {{{assigns}}}]->(b)",
                from = schema::esc(from),
                to = schema::esc(to),
            ));
            report.edges += 1;
        }
    }

    // Everything validated — adopt identity + custody (a restore IS the
    // exported graph; the fresh init's identity was a placeholder; older
    // exports without one keep the fresh identity), then write. A PORT does
    // NOT adopt identity: the target is a different repo — a new graph.
    let gid = data.get("graph_id").and_then(J::as_str).unwrap_or("");
    if !gid.is_empty() && !as_planned {
        super::meta::set_identity(
            db,
            gid,
            data.get("graph_name").and_then(J::as_str).unwrap_or(""),
            data.get("custody").and_then(J::as_str).unwrap_or("owned"),
        )?;
    }
    for s in &stmts {
        db.execute(s)?;
    }
    Ok(report)
}
