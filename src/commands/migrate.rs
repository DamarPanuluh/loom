//! `loom migrate` — in-place schema upgrade for a live `.loom/` graph.
//!
//! v3 → v4: edge identity became DERIVED (`schema::edge_key`) instead of a
//! stored uuid. The data fix is small and self-contained: every note that
//! referenced a stored edge uuid is remapped to the derived key, and the meta
//! version is bumped. The legacy `id` props left on pre-v4 edges are inert —
//! nothing reads them — so they are left in place rather than churned.
//!
//! Idempotent: a v4 graph reports "already current" and exits 0.
//!
//! Crash-safety comes from IDEMPOTENCE + ORDER, not a transaction: every note
//! remap is an idempotent SET, and the meta version is stamped LAST — so a
//! migrate that dies midway leaves a graph that still reports v3 and simply
//! re-runs. (The first cut wrapped the whole upgrade in one transaction and
//! went quadratic on a real graph — minutes of CPU over ~5,000 notes, killed.
//! The trap is READS inside a big write transaction: each MATCH-by-property
//! rescans the label THROUGH the growing MVCC overlay, so N set-then-match
//! statements cost O(N²). Pure-INSERT transactions of the same size are fine —
//! `loom import` writes 6k+ statements in one transaction in ~3s.)
//!
//! (Repos that only have a committed `loom.graph.json` don't need this:
//! `loom import` upgrades v3 exports in flight. This command is for live
//! graphs with history — note targets — worth preserving.)

use anyhow::Result;
use grafeo::Value;
use std::collections::HashMap;

use crate::db::schema::{self, EDGE_TYPES, SCHEMA_VERSION};
use crate::db::{ensure_initialized, GrafeoDb, LoomDb};
use crate::output::Printer;

pub fn run(printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    let meta = crate::db::queries::get_meta(&db)?;
    let found = meta.map(|m| m.version).unwrap_or_default();

    if found == SCHEMA_VERSION {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "ok", "migrated": false,
                "version": SCHEMA_VERSION,
                "message": "Graph is already at the current schema version.",
            }));
        } else {
            println!("✓ Graph is already at schema v{SCHEMA_VERSION} — nothing to do (re-running is safe).");
        }
        return Ok(());
    }
    if !matches!(found.as_str(), "3" | "4") {
        anyhow::bail!(
            "Graph reports schema version '{found}' — this loom upgrades v3/v4 graphs to v{SCHEMA_VERSION}. \
             For older graphs, export with the loom version that wrote them and `loom import` the export here."
        );
    }

    // Older graphs predate `loom init`'s property indexes — create them first
    // (idempotent), so every per-node SET below is a key lookup, not a scan.
    for stmt in schema::index_statements() {
        db.execute(&stmt)?;
    }

    // ---- v3 → v4: notes referencing stored edge uuids → derived keys ----
    let (mut edges_mapped, mut notes_remapped) = (0usize, 0usize);
    if found == "3" {
        // Legacy stored edge uuid → derived v4 key. `r.id` is readable in
        // RETURN position (only FILTER position is broken — the probe suite),
        // so the map comes straight off the live edges.
        let mut legacy: HashMap<String, String> = HashMap::new();
        for &etype in EDGE_TYPES {
            let r = db.execute(&format!(
                "MATCH (a)-[r:{etype}]->(b) RETURN r.id AS old, a.id AS f, b.id AS t"
            ))?;
            for row in r.rows() {
                let old = str_of(&row[0]);
                if old.is_empty() {
                    continue; // edge written post-v4 — already id-less
                }
                let f = str_of(&row[1]);
                let t = str_of(&row[2]);
                legacy.insert(old, schema::edge_key(etype, &f, &t));
            }
        }
        edges_mapped = legacy.len();

        // Remap edge-targeted notes (node-keyed SET — reliable, idempotent,
        // auto-committed: see the module docs for why NOT one big transaction).
        for n in crate::db::queries::list_notes(&db, None, None)? {
            if n.target_kind != "edge" {
                continue;
            }
            if let Some(new_key) = legacy.get(&n.target_id) {
                db.execute(&format!(
                    "MATCH (n:Note {{id: '{nid}'}}) SET n.target_id = '{tid}'",
                    nid = schema::esc(&n.id),
                    tid = schema::esc(new_key),
                ))?;
                notes_remapped += 1;
            }
        }
    }

    // ---- v3/v4 → v5: JSON-string list props → native lists ----
    // Read the RAW values and convert only what is still stored as a string;
    // already-converted nodes are skipped, so re-running converges.
    let mut nodes_listified = 0usize;
    let intent_rows = db.execute("MATCH (n:Intent) RETURN n.id, n.source_refs, n.tags")?;
    for row in intent_rows.rows() {
        let id = str_of(&row[0]);
        let refs = string_json_list(&row[1]);
        let tags = string_json_list(&row[2]);
        if refs.is_none() && tags.is_none() {
            continue;
        }
        let mut p: HashMap<String, Value> = HashMap::new();
        let mut sets: Vec<&str> = Vec::new();
        p.insert("id".into(), Value::String(id.into()));
        if let Some(v) = refs {
            p.insert("refs".into(), list_value(v));
            sets.push("n.source_refs = $refs");
        }
        if let Some(v) = tags {
            p.insert("tags".into(), list_value(v));
            sets.push("n.tags = $tags");
        }
        db.execute_with_params(
            &format!("MATCH (n:Intent {{id: $id}}) SET {}", sets.join(", ")),
            p,
        )?;
        nodes_listified += 1;
    }
    let cf_rows = db.execute("MATCH (n:CodeFile) RETURN n.id, n.imports")?;
    for row in cf_rows.rows() {
        let id = str_of(&row[0]);
        let Some(imports) = string_json_list(&row[1]) else { continue };
        let mut p: HashMap<String, Value> = HashMap::new();
        p.insert("id".into(), Value::String(id.into()));
        p.insert("imports".into(), list_value(imports));
        db.execute_with_params("MATCH (n:CodeFile {id: $id}) SET n.imports = $imports", p)?;
        nodes_listified += 1;
    }

    // Stamp the new version LAST — the completion marker. Anything that died
    // before this line leaves the graph reporting the OLD version; re-running
    // finishes the remainder (every step above skips already-converted data).
    db.execute(&format!(
        "MATCH (m:LoomMeta) SET m.version = '{}'",
        schema::esc(SCHEMA_VERSION)
    ))?;

    let next_step = "`loom export` to refresh the committed loom.graph.json, then `loom doctor`.";
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok", "migrated": true,
            "from_version": found, "to_version": SCHEMA_VERSION,
            "legacy_edge_ids_mapped": edges_mapped,
            "notes_remapped": notes_remapped,
            "list_props_converted": nodes_listified,
            "next_step": next_step,
        }));
    } else {
        println!("✓ Migrated graph schema v{found} → v{SCHEMA_VERSION}");
        println!("  legacy edge ids mapped: {edges_mapped}");
        println!("  notes remapped:         {notes_remapped}");
        println!("  list props converted:   {nodes_listified}");
        println!("  → Next: {next_step}");
    }
    Ok(())
}

fn str_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.to_string(),
        _ => String::new(),
    }
}

/// Some(items) when the stored value is still a pre-v5 JSON-encoded string
/// ("" counts as empty list); None when already a native list (or absent).
fn string_json_list(v: &Value) -> Option<Vec<String>> {
    match v {
        Value::String(s) if s.trim().is_empty() => Some(Vec::new()),
        Value::String(s) => Some(serde_json::from_str(s.as_ref()).unwrap_or_default()),
        _ => None,
    }
}

fn list_value(items: Vec<String>) -> Value {
    Value::List(
        items
            .into_iter()
            .map(|s| Value::String(s.into()))
            .collect::<Vec<_>>()
            .into(),
    )
}
