use anyhow::Result;

use crate::db::queries::DoctorReport;
use crate::db::{GraphReadHandle, GraphReadRepository};
use crate::output::Printer;

pub fn run(printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let store = GraphReadHandle::open(&cwd)?;
    run_with_db(&store, &cwd, printer)
}

pub fn run_with_db(
    db: &dyn GraphReadRepository,
    root: &std::path::Path,
    printer: &Printer,
) -> Result<()> {
    let snapshot = db.query_snapshot()?;
    let report = db.doctor_report(&snapshot)?;
    let export_stale = db.committed_export_stale(root)?;
    render(report, export_stale, printer)
}

fn render(report: DoctorReport, export_stale: Option<bool>, printer: &Printer) -> Result<()> {
    // Committed-export freshness (advisory — the hard gate is `loom export
    // --check`): when a travel-format file exists next to the graph, a doctor
    // run is a natural moment to notice it drifted.
    let mut hints = report.hints.clone();
    if export_stale == Some(true) {
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
                serde_json::json!("re-export from the loom that wrote this graph, then `loom init . && loom import loom.graph.json` here")
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
                "    → no in-place upgrade exists: re-export from the loom that wrote this graph, \
                 then rebuild here with `loom init . && loom import loom.graph.json`."
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
