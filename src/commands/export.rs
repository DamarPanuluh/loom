//! `loom export` — write the graph's travel format: deterministic JSON meant
//! to be committed to git (diffable in PRs) and rebuilt with `loom import`.

use anyhow::Result;
use std::fs;

use crate::db::{GraphReadHandle, GraphReadRepository};
use crate::output::Printer;

pub fn run(out: &str, check: bool, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let store = GraphReadHandle::open(&cwd)?;
    run_with_db(&store, &cwd, out, check, printer)
}

pub fn run_with_db(
    db: &dyn GraphReadRepository,
    root: &std::path::Path,
    out: &str,
    check: bool,
    printer: &Printer,
) -> Result<()> {
    let graph = db.export_json()?;
    run_graph(root, out, check, printer, graph)
}

fn run_graph(
    root: &std::path::Path,
    out: &str,
    check: bool,
    printer: &Printer,
    graph: serde_json::Value,
) -> Result<()> {
    let pretty = serde_json::to_string_pretty(&graph)?;

    if check {
        // The commit guard: the export is deterministic (same graph →
        // identical bytes), so freshness is a byte comparison. Non-zero exit
        // on drift makes this hookable (pre-commit / CI) — a graph change can
        // never silently ship without its travel format.
        if out == "-" {
            anyhow::bail!("--check needs a file to compare against (not '-') — use `loom export --check loom.graph.json` or drop --check.");
        }
        let confined_out = crate::repo::confine(root, std::path::Path::new(out))
            .ok_or_else(|| anyhow::anyhow!("export path escapes graph root: {out}"))?;
        let on_disk = fs::read_to_string(root.join(confined_out)).ok();
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
            println!("{}", crate::output::up_to_date_line(out));
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
    let confined_out = crate::repo::confine(root, std::path::Path::new(out))
        .ok_or_else(|| anyhow::anyhow!("export path escapes graph root: {out}"))?;
    let target = root.join(confined_out);
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
