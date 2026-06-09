//! `loom import` — rebuild a graph from `loom export` output. Restoration
//! into a fresh `loom init`, not a merge.

use anyhow::Result;
use std::env;
use std::fs;

use crate::db::queries::import_graph;
use crate::db::{ensure_initialized, GrafeoDb};
use crate::output::Printer;

pub fn run(file: &str, printer: &Printer) -> Result<()> {
    let cwd = env::current_dir()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    let raw = fs::read_to_string(cwd.join(file))
        .map_err(|e| anyhow::anyhow!("Cannot read '{}': {}", file, e))?;
    let data: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| anyhow::anyhow!("'{}' is not valid JSON: {}", file, e))?;

    let report = import_graph(&db, &data)?;

    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok", "file": file,
            "nodes": report.nodes, "edges": report.edges,
        }));
    } else {
        println!("✓ Graph imported from {file}  ({} nodes, {} edges)", report.nodes, report.edges);
        println!("  → `loom sync` to reconcile against this machine's files, then `loom status`.");
    }
    Ok(())
}
