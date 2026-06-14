//! `loom migrate` — in-place schema upgrade for a live `.loom/` graph.
//!
//! v3 → v4: edge identity became DERIVED (`schema::edge_key`) instead of a
//! stored uuid. The data fix is small and self-contained: every note that
//! referenced a stored edge uuid is remapped to the derived key, and the meta
//! version is bumped. The legacy `id` props left on pre-v4 edges are inert —
//! nothing reads them — so they are left in place rather than churned.
//!
//! v5 → v6: product domain and architecture layer split. If the old graph had
//! a declared `domain_order`, it was using domains as layers: copy each
//! Intent.domain into Intent.layer and move the order to `layer_order`.
//! Without a declared order, keep layer empty.
//!
//! v6 → v7: `CodeFile.symbols` is an additive native-list physical fact.
//! Missing symbols are backfilled as [] and populated by the next `loom sync`.
//!
//! v7 → v8: `CodeFile.symbol_facts` is an additive native-list of JSON
//! SymbolFact objects. Missing facts are backfilled as [] and populated by the
//! next `loom sync`.
//!
//! Idempotent: a current graph reports "already current" and exits 0.
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

use anyhow::{Context, Result};
use grafeo::Value;
use std::collections::HashMap;

use crate::db::schema::{self, EDGE_TYPES, SCHEMA_VERSION};
use crate::db::{ensure_initialized, GrafeoDb, LoomDb};
use crate::output::Printer;

pub fn run(printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;
    run_with_db(&db, &cwd, printer)
}

pub fn run_with_db(db: &GrafeoDb, _root: &std::path::Path, printer: &Printer) -> Result<()> {
    let meta = crate::db::queries::get_meta(db)?;
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
    if !matches!(found.as_str(), "3" | "4" | "5" | "6" | "7") {
        anyhow::bail!(
            "Graph reports schema version '{found}' — this loom upgrades v3/v4/v5/v6/v7 graphs to v{SCHEMA_VERSION}. \
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
        for n in crate::db::queries::list_notes(db, None, None)? {
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
        let refs = string_json_list(&row[1], "Intent", &id, "source_refs")?;
        let tags = string_json_list(&row[2], "Intent", &id, "tags")?;
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
        let Some(imports) = string_json_list(&row[1], "CodeFile", &id, "imports")? else {
            continue;
        };
        let mut p: HashMap<String, Value> = HashMap::new();
        p.insert("id".into(), Value::String(id.into()));
        p.insert("imports".into(), list_value(imports));
        db.execute_with_params("MATCH (n:CodeFile {id: $id}) SET n.imports = $imports", p)?;
        nodes_listified += 1;
    }

    // ---- v5 → v6: product domain/layer split ----
    let legacy_order = crate::db::queries::get_legacy_domain_order(db)?;
    let mut layers_populated = 0usize;
    let mut layer_order_migrated = false;
    let layer_source = if legacy_order.is_empty() {
        String::new()
    } else {
        crate::db::queries::set_layer_order(db, &legacy_order)?;
        layer_order_migrated = true;
        "__copy_domain__".into()
    };
    let rows = db.execute("MATCH (n:Intent) RETURN n.id, n.domain, n.layer")?;
    for row in rows.rows() {
        let id = str_of(&row[0]);
        let domain = str_of(&row[1]);
        let current_layer = str_of(&row[2]);
        if !current_layer.is_empty() {
            continue;
        }
        let layer = if layer_source == "__copy_domain__" {
            domain
        } else {
            String::new()
        };
        let mut p: HashMap<String, Value> = HashMap::new();
        p.insert("id".into(), Value::String(id.into()));
        p.insert("layer".into(), Value::String(layer.into()));
        db.execute_with_params("MATCH (n:Intent {id: $id}) SET n.layer = $layer", p)?;
        layers_populated += 1;
    }

    // ---- v6 → v7: CodeFile.symbols additive list backfill ----
    let mut symbols_backfilled = 0usize;
    let rows = db.execute("MATCH (n:CodeFile) WHERE n.symbols IS NULL RETURN n.id")?;
    for row in rows.rows() {
        let id = str_of(&row[0]);
        let mut p: HashMap<String, Value> = HashMap::new();
        p.insert("id".into(), Value::String(id.into()));
        p.insert("symbols".into(), list_value(Vec::new()));
        db.execute_with_params("MATCH (n:CodeFile {id: $id}) SET n.symbols = $symbols", p)?;
        symbols_backfilled += 1;
    }

    // ---- v7 → v8: CodeFile.symbol_facts additive list backfill ----
    let mut symbol_facts_backfilled = 0usize;
    let rows = db.execute("MATCH (n:CodeFile) WHERE n.symbol_facts IS NULL RETURN n.id")?;
    for row in rows.rows() {
        let id = str_of(&row[0]);
        let mut p: HashMap<String, Value> = HashMap::new();
        p.insert("id".into(), Value::String(id.into()));
        p.insert("symbol_facts".into(), list_value(Vec::new()));
        db.execute_with_params(
            "MATCH (n:CodeFile {id: $id}) SET n.symbol_facts = $symbol_facts",
            p,
        )?;
        symbol_facts_backfilled += 1;
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
            "layers_populated": layers_populated,
            "legacy_domain_order_migrated": layer_order_migrated,
            "symbols_backfilled": symbols_backfilled,
            "symbol_facts_backfilled": symbol_facts_backfilled,
            "next_step": next_step,
        }));
    } else {
        println!("✓ Migrated graph schema v{found} → v{SCHEMA_VERSION}");
        println!("  legacy edge ids mapped: {edges_mapped}");
        println!("  notes remapped:         {notes_remapped}");
        println!("  list props converted:   {nodes_listified}");
        println!("  layers populated:       {layers_populated}");
        println!("  legacy order migrated:  {layer_order_migrated}");
        println!("  symbols backfilled:     {symbols_backfilled}");
        println!("  symbol facts backfilled:{symbol_facts_backfilled}");
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
fn string_json_list(v: &Value, label: &str, id: &str, prop: &str) -> Result<Option<Vec<String>>> {
    match v {
        Value::String(s) if s.trim().is_empty() => Ok(Some(Vec::new())),
        Value::String(s) => serde_json::from_str(s.as_ref()).map(Some).with_context(|| {
            format!(
                "Failed to parse pre-v5 JSON list for {label} node '{id}' property '{prop}'. \
                     Fix the stored JSON string before running `loom migrate`."
            )
        }),
        _ => Ok(None),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::queries::get_meta;
    use crate::db::{GrafeoDb, LoomDb};
    use crate::output::Printer;
    use std::path::Path;

    fn version(db: &GrafeoDb) -> String {
        get_meta(db).unwrap().map(|m| m.version).unwrap_or_default()
    }

    fn migrate(db: &GrafeoDb) -> String {
        let p = Printer::capturing(true);
        run_with_db(db, Path::new("."), &p).unwrap();
        p.captured().unwrap()
    }

    #[test]
    fn already_current_is_an_idempotent_noop() {
        let db = GrafeoDb::in_memory();
        db.execute(&schema::insert_meta(SCHEMA_VERSION, "t", "g", "n", "owned"))
            .unwrap();
        let out = migrate(&db);
        assert_eq!(version(&db), SCHEMA_VERSION);
        assert!(out.contains("\"migrated\":false"), "{out}");
    }

    #[test]
    fn upgrades_a_v6_graph_then_converges_on_rerun() {
        let db = GrafeoDb::in_memory();
        db.execute(&schema::insert_meta("6", "t", "g", "n", "owned"))
            .unwrap();
        db.execute("INSERT (:CodeFile {id: 'cf1', path: '/a.rs'})")
            .unwrap();

        let out = migrate(&db);
        assert_eq!(version(&db), SCHEMA_VERSION, "version stamped to current");
        assert!(
            out.contains("\"migrated\":true") && out.contains("\"from_version\":\"6\""),
            "{out}"
        );
        // v6→v7 then v7→v8 backfilled the two additive physical-fact lists for
        // the one codefile that lacked them.
        assert!(out.contains("\"symbols_backfilled\":1"), "{out}");
        assert!(out.contains("\"symbol_facts_backfilled\":1"), "{out}");

        // Crash-safety contract: the version is stamped LAST and every step skips
        // already-converted data, so a re-run is a clean noop (a migrate that
        // died midway would simply re-run the remainder).
        let again = migrate(&db);
        assert!(
            again.contains("\"migrated\":false"),
            "second run is a noop: {again}"
        );
    }

    #[test]
    fn v5_to_v6_leaves_layer_empty_without_a_declared_domain_order() {
        let db = GrafeoDb::in_memory();
        db.execute(&schema::insert_meta("5", "t", "g", "n", "owned"))
            .unwrap();
        db.execute("INSERT (:Intent {id: 'i1', domain: 'storage', layer: ''})")
            .unwrap();

        migrate(&db);
        assert_eq!(version(&db), SCHEMA_VERSION);
        // Documented contract: domain is NOT copied into layer unless the old
        // graph declared a domain_order (i.e. was using domains AS layers).
        let r = db
            .execute("MATCH (n:Intent {id: 'i1'}) RETURN n.layer")
            .unwrap();
        assert_eq!(str_of(&r.rows()[0][0]), "", "layer stays empty");
    }

    #[test]
    fn refuses_a_version_it_cannot_upgrade() {
        let db = GrafeoDb::in_memory();
        db.execute(&schema::insert_meta("2", "t", "g", "n", "owned"))
            .unwrap();
        let err = run_with_db(&db, Path::new("."), &Printer::capturing(true)).unwrap_err();
        assert!(err.to_string().contains("schema version '2'"), "{err}");
    }
}
