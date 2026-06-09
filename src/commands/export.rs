//! `loom export` — write the graph's travel format: deterministic JSON meant
//! to be committed to git (diffable in PRs) and rebuilt with `loom import`.

use anyhow::Result;
use std::env;
use std::fs;

use crate::db::queries::export_graph;
use crate::db::{ensure_initialized, GrafeoDb};
use crate::output::Printer;

pub fn run(out: &str, printer: &Printer) -> Result<()> {
    let cwd = env::current_dir()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    let graph = export_graph(&db)?;
    let pretty = serde_json::to_string_pretty(&graph)?;

    if out == "-" {
        println!("{pretty}");
        return Ok(());
    }
    fs::write(cwd.join(out), &pretty)?;

    let nodes: usize = graph["nodes"].as_object().map(|m| m.values().filter_map(|v| v.as_array()).map(|a| a.len()).sum()).unwrap_or(0);
    let edges: usize = graph["edges"].as_object().map(|m| m.values().filter_map(|v| v.as_array()).map(|a| a.len()).sum()).unwrap_or(0);
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok", "out": out, "nodes": nodes, "edges": edges,
        }));
    } else {
        println!("✓ Graph exported to {out}  ({nodes} nodes, {edges} edges)");
        println!("  Commit it so the graph travels with the repo; rebuild anywhere with:");
        println!("  loom init . && loom import {out}");
    }
    Ok(())
}
