use super::SqliteGraphStore;
use super::*;

impl SqliteGraphStore {
    pub fn import_export_json(&mut self, data: &JsonValue) -> Result<()> {
        if data.get("loom_export").and_then(JsonValue::as_i64) != Some(1) {
            anyhow::bail!("Not a loom export (missing/unknown `loom_export` marker).");
        }

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
