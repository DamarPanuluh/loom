use anyhow::Result;
use std::env;

use crate::db::queries::check_graph;
use crate::db::{ensure_initialized, GrafeoDb};
use crate::output::Printer;

pub fn run(printer: &Printer) -> Result<()> {
    let cwd = env::current_dir()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    let report = check_graph(&db)?;

    // Committed-export freshness (advisory — the hard gate is `loom export
    // --check`): when a travel-format file exists next to the graph, a doctor
    // run is a natural moment to notice it drifted.
    let mut hints = report.hints.clone();
    let export_path = cwd.join("loom.graph.json");
    if export_path.exists() {
        let live = serde_json::to_string_pretty(&crate::db::queries::export_graph(&db)?)?;
        if std::fs::read_to_string(&export_path).ok().as_deref() != Some(live.as_str()) {
            hints.push(
                "loom.graph.json is STALE vs the live graph — run `loom export` and commit it \
                 (gate it mechanically with `loom export --check` in a pre-commit hook/CI)"
                    .to_string(),
            );
        }
    }

    if printer.json {
        printer.print_json(&serde_json::json!({
            "healthy": report.healthy(),
            "schema_version": {
                "expected": report.expected_version,
                "found":    report.found_version,
                "ok":       report.version_ok,
            },
            "node_counts": report.node_counts,
            "edge_counts": report.edge_counts,
            "issues":      report.issues,
            "hints":       hints,
        }));
    } else {
        println!("── loom doctor ──────────────────────────────────────────────────────");
        println!(
            "  schema version: {} (expected {})  {}",
            report.found_version,
            report.expected_version,
            if report.version_ok { "✓" } else { "✗" }
        );
        println!();
        println!("  Nodes:");
        for (lbl, c) in &report.node_counts {
            println!("    {:<14} {}", lbl, c);
        }
        println!("  Edges:");
        for (etype, c) in &report.edge_counts {
            println!("    {:<14} {}", etype, c);
        }
        println!();
        if report.issues.is_empty() && report.version_ok {
            println!("  ✓ No integrity issues — the graph conforms to the schema.");
        } else {
            println!("  ✗ {} issue(s):", report.issues.len());
            for i in &report.issues {
                println!("    - {}", i);
            }
        }
        if !hints.is_empty() {
            println!();
            println!("  Hints (advisory — never fail the check):");
            for h in &hints {
                println!("    · {}", h);
            }
        }
    }

    // Non-zero exit on problems so `loom doctor` is scriptable; stdout (incl.
    // JSON) is already written, the error only goes to stderr.
    if !report.healthy() {
        anyhow::bail!("graph has integrity issues");
    }
    Ok(())
}
