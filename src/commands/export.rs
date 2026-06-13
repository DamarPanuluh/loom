//! `loom export` — write the graph's travel format: deterministic JSON meant
//! to be committed to git (diffable in PRs) and rebuilt with `loom import`.

use anyhow::Result;
use std::fs;

use crate::db::queries::export_graph;
use crate::db::{ensure_initialized, GrafeoDb};
use crate::output::Printer;

pub fn run(out: &str, check: bool, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;
    run_with_db(&db, &cwd, out, check, printer)
}

pub fn run_with_db(
    db: &GrafeoDb,
    root: &std::path::Path,
    out: &str,
    check: bool,
    printer: &Printer,
) -> Result<()> {
    let graph = export_graph(db)?;
    let pretty = serde_json::to_string_pretty(&graph)?;

    if check {
        // The commit guard: the export is deterministic (same graph →
        // identical bytes), so freshness is a byte comparison. Non-zero exit
        // on drift makes this hookable (pre-commit / CI) — a graph change can
        // never silently ship without its travel format.
        if out == "-" {
            anyhow::bail!("--check needs a file to compare against (not '-') — use `loom export --check loom.graph.json` or drop --check.");
        }
        let on_disk = fs::read_to_string(root.join(out)).ok();
        let fresh = on_disk.as_deref() == Some(pretty.as_str());
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": if fresh { "ok" } else if on_disk.is_none() { "missing" } else { "stale" },
                "out": out,
                "next_step": if fresh {
                    format!("commit {out} so the graph travels")
                } else {
                    format!("run `loom export` and commit {out}")
                },
            }));
        } else if fresh {
            println!("✓ {out} is up to date with the graph.");
        } else if on_disk.is_none() {
            println!("✗ {out} does not exist — run `loom export` and commit it.");
        } else {
            println!("✗ {out} is STALE — the graph has changed since it was written.");
            println!("  Run `loom export` and commit the result.");
        }
        if !fresh {
            anyhow::bail!(
                "export file is stale or missing — run `loom export` and commit the result."
            );
        }
        return Ok(());
    }

    if out == "-" {
        println!("{pretty}");
        return Ok(());
    }
    let target = root.join(out);
    let mut tmp = target.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    fs::write(&tmp, &pretty)?;
    fs::rename(&tmp, &target)?;

    let nodes: usize = graph["nodes"]
        .as_object()
        .map(|m| {
            m.values()
                .filter_map(|v| v.as_array())
                .map(|a| a.len())
                .sum()
        })
        .unwrap_or(0);
    let edges: usize = graph["edges"]
        .as_object()
        .map(|m| {
            m.values()
                .filter_map(|v| v.as_array())
                .map(|a| a.len())
                .sum()
        })
        .unwrap_or(0);
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok", "out": out, "nodes": nodes, "edges": edges,
            "next_step": format!("commit {out} so the graph travels"),
        }));
    } else {
        println!("✓ Graph exported to {out}  ({nodes} nodes, {edges} edges)");
        println!("  Commit it so the graph travels with the repo; rebuild anywhere with:");
        println!("  loom init . && loom import {out}");
    }
    Ok(())
}
