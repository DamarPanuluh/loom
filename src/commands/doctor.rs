use anyhow::Result;

use crate::db::queries::DoctorReport;
use crate::db::{loom_dir, GraphReadHandle, GraphReadRepository};
use crate::output::Printer;

/// Dead backend relics — files loom once wrote to `.loom/` but no longer reads
/// after the storage generations landed on `graph.sqlite`. An explicit
/// allowlist (NOT a generic "anything unrecognized" sweep): a future artifact
/// loom doesn't know about yet, or a user file, is never touched. The live
/// `graph.sqlite` (+ its WAL/SHM) is never on this list.
const DEAD_BACKEND_RELICS: &[&str] = &[
    "graph.grafeo",
    "db.sqlite",
    "db.sqlite-wal",
    "db.sqlite-shm",
    "graph.db",
    "graph.db-wal",
    "graph.db-shm",
    "graph.db-journal",
];

pub fn run(clean_orphans: bool, yes: bool, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    if clean_orphans {
        return clean_orphaned_backends(&cwd, yes, printer);
    }
    let store = GraphReadHandle::open(&cwd)?;
    run_with_db(&store, &cwd, printer)
}

/// `loom doctor --clean-orphans` — reap dead backend relics from `.loom/`.
/// Dry-run by default (lists what would go + the --yes reminder); `--yes`
/// actually removes. Targets only [`DEAD_BACKEND_RELICS`] — never the live
/// `graph.sqlite`. The honesty fix for the 0.3.3→0.4.0 upgrade leaving
/// graph.grafeo / db.sqlite / graph.db behind for the driver to delete by hand.
fn clean_orphaned_backends(root: &std::path::Path, yes: bool, printer: &Printer) -> Result<()> {
    let loom = loom_dir(root);
    let mut found: Vec<String> = Vec::new();
    for name in DEAD_BACKEND_RELICS {
        if loom.join(name).is_file() {
            found.push((*name).to_string());
        }
    }
    let removed: Vec<String> = if yes {
        let mut removed = Vec::new();
        for name in &found {
            match std::fs::remove_file(loom.join(name)) {
                Ok(()) => removed.push(name.clone()),
                Err(e) => {
                    // A reap that silently skips a file it claimed to remove
                    // would be the new honesty hole — report the failure in
                    // both audiences instead of swallowing it.
                    if printer.json {
                        printer.print_json(&serde_json::json!({
                            "removed": removed,
                            "failed": { "file": name, "error": e.to_string() },
                            "dry_run": false,
                        }));
                    } else {
                        println!("  failed to remove {name}: {e}");
                    }
                    return Ok(());
                }
            }
        }
        removed
    } else {
        Vec::new()
    };

    if printer.json {
        printer.print_json(&serde_json::json!({
            "orphaned_relics": found,
            "removed": removed,
            "dry_run": !yes,
        }));
        return Ok(());
    }

    if found.is_empty() {
        println!("(no orphaned backend relics in {})", loom.display());
        return Ok(());
    }
    let verb = if yes { "removed" } else { "would remove" };
    for name in &found {
        let mark = if yes && removed.contains(name) {
            "✓"
        } else {
            "·"
        };
        println!("  {mark} {verb} {name}");
    }
    if !yes {
        println!(
            "  dry-run — re-run with --yes to remove these {} relic(s) \
             (the live graph.sqlite is never touched).",
            found.len()
        );
    }
    Ok(())
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
                serde_json::json!(crate::output::REBUILD_FROM_EXPORT_HINT)
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
