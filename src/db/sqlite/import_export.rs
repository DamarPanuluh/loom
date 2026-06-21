use super::SqliteGraphStore;
use super::*;

/// Validate an export's structural + value invariants BEFORE any insert, so a
/// malformed graph (HIERARCHY cycle/multi-parent/self-loop, dangling edge,
/// out-of-range confidence) is REFUSED rather than silently persisted and then
/// re-exported as byte-clean "truth". Mirrors what the interactive write paths
/// and `loom doctor` enforce; the raw import INSERTs only have DDL to lean on,
/// which can't express these cross-row invariants.
fn validate_import_data(data: &JsonValue) -> Result<()> {
    use std::collections::{HashMap, HashSet};

    // Every node id, to catch a dangling edge with a clear message instead of a
    // raw SQLite foreign-key error.
    let mut node_ids: HashSet<&str> = HashSet::new();
    if let Some(nodes) = data.get("nodes").and_then(JsonValue::as_object) {
        for items in nodes.values() {
            for n in items.as_array().into_iter().flatten() {
                if let Some(id) = n.get("id").and_then(JsonValue::as_str) {
                    node_ids.insert(id);
                }
            }
        }
    }

    let Some(edges) = data.get("edges").and_then(JsonValue::as_object) else {
        return Ok(());
    };

    // Per-edge across all types: endpoints must resolve, confidence stays bounded.
    for (etype, items) in edges {
        for e in items.as_array().into_iter().flatten() {
            for slot in ["from", "to"] {
                let id = e.get(slot).and_then(JsonValue::as_str).unwrap_or("");
                if !id.is_empty() && !node_ids.contains(id) {
                    anyhow::bail!(
                        "Import rejected: a {etype} edge references a {slot} node '{id}' that no node defines — a dangling edge. Fix the export and re-import (nothing was imported)."
                    );
                }
            }
            if let Some(c) = e.get("confidence").and_then(JsonValue::as_f64) {
                if !(0.0..=1.0).contains(&c) {
                    anyhow::bail!(
                        "Import rejected: a {etype} edge has confidence {c} outside [0,1] — confidence is a bounded trust signal. Fix the export and re-import (nothing was imported)."
                    );
                }
            }
        }
    }

    // HIERARCHY must be an acyclic tree: one parent per child, no self-loop, no cycle.
    if let Some(hier) = edges.get("HIERARCHY").and_then(JsonValue::as_array) {
        let mut parent_of: HashMap<&str, &str> = HashMap::new();
        for e in hier {
            let parent = e.get("from").and_then(JsonValue::as_str).unwrap_or("");
            let child = e.get("to").and_then(JsonValue::as_str).unwrap_or("");
            if parent.is_empty() || child.is_empty() {
                continue;
            }
            if parent == child {
                anyhow::bail!(
                    "Import rejected: intent '{child}' is its own HIERARCHY parent (a self-loop). Fix the export and re-import (nothing was imported)."
                );
            }
            if let Some(prev) = parent_of.insert(child, parent) {
                if prev != parent {
                    anyhow::bail!(
                        "Import rejected: intent '{child}' has two HIERARCHY parents ('{prev}' and '{parent}') — the hierarchy must be a tree (one parent per child). Fix the export and re-import (nothing was imported)."
                    );
                }
            }
        }
        // A cycle = following a child up its parent chain revisits a node. Walk
        // each chain ONCE: a `safe` set of nodes already proven to reach a root
        // (or a known-safe node) makes this O(N) total instead of O(N^2) — the
        // naive per-node re-walk let a crafted long chain hang import for seconds.
        let mut safe: HashSet<&str> = HashSet::new();
        for start in parent_of.keys() {
            let mut on_path: HashSet<&str> = HashSet::new();
            let mut path: Vec<&str> = Vec::new();
            let mut cur: &str = start;
            loop {
                if safe.contains(cur) {
                    break;
                }
                if !on_path.insert(cur) {
                    anyhow::bail!(
                        "Import rejected: the HIERARCHY contains a cycle (through '{cur}') — the hierarchy must be an acyclic tree. Fix the export and re-import (nothing was imported)."
                    );
                }
                path.push(cur);
                match parent_of.get(cur) {
                    Some(&p) => cur = p,
                    None => break,
                }
            }
            safe.extend(path);
        }
    }

    Ok(())
}

impl SqliteGraphStore {
    pub fn import_export_json(&mut self, data: &JsonValue) -> Result<()> {
        if data.get("loom_export").and_then(JsonValue::as_i64) != Some(1) {
            anyhow::bail!("Not a loom export (missing/unknown `loom_export` marker).");
        }
        // Import is a TRUSTED graph-construction path (federation, restore, a
        // PR-merged loom.graph.json), but it inserts with raw INSERTs that only
        // the DDL constraints guard — so a hand-edit or bad merge could persist a
        // graph the interactive write paths (and `loom doctor`) would condemn:
        // a HIERARCHY cycle/multi-parent, an out-of-range confidence, a dangling
        // edge. Validate the data FIRST so a malformed graph never imports
        // "successfully" and then travels as byte-clean truth.
        validate_import_data(data)?;

        let tx = self.write_tx()?;
        clear_all(&tx)?;

        let layer_order = compact_json(data.get("layer_order").unwrap_or(&json!([])))?;
        tx.execute(
            "INSERT INTO meta(
                id, schema_version, graph_id, graph_name, custody, created_at,
                last_synced, transition_cap, layer_order
             ) VALUES(1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                // Stamp the ACTIVE schema version, not the export's: the data is
                // normalized into THIS loom's schema on import, so an older
                // export upgrades to the current version instead of carrying a
                // stale one (which would trip the doctor version check).
                crate::db::schema::SCHEMA_VERSION,
                str_top(data, "graph_id"),
                str_top(data, "graph_name"),
                str_top(data, "custody"),
                str_top(data, "created_at"),
                str_top(data, "last_synced"),
                str_top(data, "transition_cap"),
                layer_order,
            ],
        )?;

        let nodes = object_field(data, "nodes")?;
        for spec in NODE_SPECS {
            let items = section_array(nodes, "nodes", spec.label)?;
            for item in items {
                insert_node(&tx, *spec, item_object(item, spec.label)?)?;
            }
        }

        let edges = object_field(data, "edges")?;
        for spec in EDGE_SPECS {
            let items = section_array(edges, "edges", spec.edge_type)?;
            for item in items {
                insert_edge(&tx, *spec, item_object(item, spec.edge_type)?)?;
            }
        }

        tx.commit()?;
        Ok(())
    }
    pub fn export_json(&self) -> Result<JsonValue> {
        // All meta fields travel: created_at (graph birth), last_synced and
        // transition_cap were previously dropped on export, so a round-trip reset
        // birth/sync stamps to "" and silently reverted a customized --set-cap to
        // the default (import reads all three via str_top).
        #[allow(clippy::type_complexity)]
        let (
            schema_version,
            graph_id,
            graph_name,
            custody,
            created_at,
            last_synced,
            transition_cap,
            layer_order,
        ): (
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
        ) = self
            .conn
            .query_row(
                "SELECT schema_version, graph_id, graph_name, custody, created_at, \
                 last_synced, transition_cap, layer_order FROM meta WHERE id = 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .optional()?
            .unwrap_or_default();

        let mut nodes = Map::new();
        for spec in NODE_SPECS {
            let mut arr = export_nodes(&self.conn, *spec)?;
            if spec.label == label::NOTE {
                // Export-time note retention: routine `transition` breadcrumbs are
                // local audit churn that dominates the artifact (97% of nodes) and
                // makes every commit churn ~20k diff lines. Full history stays in
                // .loom/graph.sqlite; the portable, diffable artifact carries only
                // the durable notes (decision/justification/confirm/todo/idea/…).
                arr.retain(|n| n.get("kind").and_then(JsonValue::as_str) != Some("transition"));
            }
            nodes.insert(spec.label.to_string(), JsonValue::Array(arr));
        }

        let mut edges = Map::new();
        for spec in EDGE_SPECS {
            edges.insert(
                spec.edge_type.to_string(),
                JsonValue::Array(export_edges(&self.conn, *spec)?),
            );
        }

        Ok(json!({
            "loom_export": 1,
            "schema_version": schema_version,
            "graph_id": graph_id,
            "graph_name": graph_name,
            "custody": custody,
            "created_at": created_at,
            "last_synced": last_synced,
            "transition_cap": transition_cap,
            "layer_order": parse_json_array(&layer_order)?,
            "nodes": nodes,
            "edges": edges,
        }))
    }
    pub fn counts(&self) -> Result<(usize, usize)> {
        let mut nodes = 0usize;
        for spec in NODE_SPECS {
            nodes += count_table(&self.conn, spec.table)?;
        }
        let mut edges = 0usize;
        for spec in EDGE_SPECS {
            edges += count_table(&self.conn, spec.table)?;
        }
        Ok((nodes, edges))
    }
    pub fn committed_export_stale(&self, root: &Path) -> Result<Option<bool>> {
        let path = root.join("loom.graph.json");
        if !path.exists() {
            return Ok(None);
        }
        let live = serde_json::to_string_pretty(&self.export_json()?)?;
        Ok(Some(
            std::fs::read_to_string(&path).ok().as_deref() != Some(live.as_str()),
        ))
    }
}
