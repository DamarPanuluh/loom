use anyhow::Result;

use crate::db::queries::check_graph;
use crate::db::{ensure_initialized, GrafeoDb};
use crate::output::Printer;

pub fn run(printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;
    run_with_db(&db, &cwd, printer)
}

pub fn run_with_db(db: &GrafeoDb, root: &std::path::Path, printer: &Printer) -> Result<()> {
    let report = check_graph(db)?;

    // Committed-export freshness (advisory — the hard gate is `loom export
    // --check`): when a travel-format file exists next to the graph, a doctor
    // run is a natural moment to notice it drifted.
    let mut hints = report.hints.clone();
    if crate::db::queries::committed_export_stale(db, root)? == Some(true) {
        hints.push(
            "loom.graph.json is STALE vs the live graph — run `loom export` and commit it \
             alongside the code (verify any time with `loom export --check`; CI wiring is \
             optional extra hardening)"
                .to_string(),
        );
    }

    if printer.json {
        printer.print_json(&serde_json::json!({
            "healthy": report.healthy(),
            "schema_version": {
                "expected": report.expected_version,
                "found":    report.found_version,
                "ok":       report.version_ok,
            },
            "next_step": if report.version_ok { serde_json::Value::Null } else {
                serde_json::json!("loom migrate")
            },
            "node_counts": report.node_counts,
            "edge_counts": report.edge_counts,
            "issues":      report.issues,
            "issues_total": report.issues.len(),
            "hints":       hints,
            "hints_total": hints.len(),
        }));
    } else {
        println!("── loom doctor ──────────────────────────────────────────────────────");
        println!(
            "  schema version: {} (expected {})  {}",
            report.found_version,
            report.expected_version,
            if report.version_ok { "✓" } else { "✗" }
        );
        if !report.version_ok {
            println!(
                "    → `loom migrate` upgrades a v3 graph in place (one transaction, idempotent)."
            );
        }
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
            // Bounded: a badly-drifted graph can carry hundreds of issues —
            // doctor --json stays the diagnostic of record (full arrays).
            println!("  ✗ {} issue(s):", report.issues.len());
            for i in report.issues.iter().take(20) {
                println!("    - {}", i);
            }
            if let Some(m) = crate::output::more_marker(
                report.issues.len(),
                20,
                "`loom doctor --json` for the full list",
            ) {
                println!("    {m}");
            }
        }
        if !hints.is_empty() {
            println!();
            println!("  Hints (advisory — never fail the check):");
            for h in hints.iter().take(20) {
                println!("    · {}", h);
            }
            if let Some(m) = crate::output::more_marker(
                hints.len(),
                20,
                "`loom doctor --json` for the full list",
            ) {
                println!("    {m}");
            }
        }
    }

    // Non-zero exit on problems so `loom doctor` is scriptable; stdout (incl.
    // JSON) is already written, the error only goes to stderr.
    if !report.healthy() {
        anyhow::bail!("graph has integrity issues — the list above carries per-issue remedies; fix and re-run `loom doctor`.");
    }
    Ok(())
}
