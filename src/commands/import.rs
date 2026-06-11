//! `loom import` — rebuild a graph from `loom export` output. Restoration
//! into a fresh `loom init`, not a merge.

use anyhow::Result;
use std::fs;

use crate::db::queries::import_graph;
use crate::db::{ensure_initialized, GrafeoDb};
use crate::output::Printer;

pub fn run(file: &str, as_planned: bool, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    let raw = fs::read_to_string(cwd.join(file))
        .map_err(|e| anyhow::anyhow!("Cannot read '{}': {} — expects a `loom export` JSON (e.g. `loom import loom.graph.json`).", file, e))?;
    let data: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| anyhow::anyhow!("'{}' is not valid JSON: {} — expects a `loom export` JSON (e.g. `loom import loom.graph.json`).", file, e))?;

    // Atomic restore: validation already rejects bad exports before writing
    // (two-phase), and the transaction closes the remaining hole — a crash or
    // write error midway can no longer leave a partial graph behind.
    let report = crate::db::with_transaction(&db, || import_graph(&db, &data, as_planned))?;
    let next_step = if as_planned {
        "`loom guide --mode port` for the re-realization loop, then `loom next --mode build`."
    } else {
        "`loom sync` to reconcile against this machine's files, then `loom status`."
    };
    if printer.json {
        let payload = serde_json::json!({
            "status": "ok", "file": file, "as_planned": as_planned,
            "nodes": report.nodes, "edges": report.edges,
            "skipped_nodes": report.skipped_nodes, "skipped_edges": report.skipped_edges,
        });
        printer.print_json(&crate::output::with_anchor(payload, &db, next_step)?);
    } else if as_planned {
        println!(
            "✓ Design adopted from {file}  ({} nodes, {} edges; {} node(s) + {} edge(s) dropped — the old repo's files/groundings)",
            report.nodes, report.edges, report.skipped_nodes, report.skipped_edges
        );
        println!("  Every intent arrived lifecycle=planned; every proof not_run; verdict meta reset to uninspected.");
        crate::output::print_anchor(&db, next_step)?;
    } else {
        println!("✓ Graph imported from {file}  ({} nodes, {} edges)", report.nodes, report.edges);
        crate::output::print_anchor(&db, next_step)?;
    }
    Ok(())
}
