use anyhow::Result;

use crate::db::queries::{
    edge_status_counts_from_snapshot, graph_state_from_snapshot,
    intents_without_validations_count_from_snapshot, uninspected_outside_queues_from_snapshot,
    validation_pass_rate_from_snapshot, QuerySnapshot,
};
use crate::db::{ensure_initialized, GrafeoDb};
use crate::output::{fmt_pulse, fmt_status, Printer};
use crate::types::StatusReport;

pub fn run(printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;
    run_with_db(&db, &cwd, printer)
}

pub fn run_with_db(db: &GrafeoDb, root: &std::path::Path, printer: &Printer) -> Result<()> {
    // One graph scan — every count below is derived from the snapshot instead
    // of re-querying nodes/edges (the scale benchmark's status hot path).
    let snapshot = QuerySnapshot::load(db)?;

    let total_intents = snapshot.intents.len() as i64;
    let total_codefiles = snapshot.codefiles.len() as i64;
    let total_validations = snapshot.validations.len() as i64;

    let by_status = edge_status_counts_from_snapshot(&snapshot);
    let total_edges = by_status.values().sum::<i64>();
    let uninspected = *by_status.get("uninspected").unwrap_or(&0);
    let passing = *by_status.get("passing").unwrap_or(&0);
    let failing = *by_status.get("failing").unwrap_or(&0);
    let independent = *by_status.get("independent").unwrap_or(&0);
    let needs_reverification = *by_status.get("needs_reverification").unwrap_or(&0);

    let pass_rate = validation_pass_rate_from_snapshot(&snapshot);
    let (blocked_validations, validation_pass_rate_runnable) =
        crate::db::queries::blocked_count_and_runnable_rate(&snapshot.validations);
    let no_val_count = intents_without_validations_count_from_snapshot(&snapshot);

    let report = StatusReport {
        total_intents,
        total_codefiles,
        total_validations,
        total_edges,
        uninspected_edges: uninspected,
        passing_edges: passing,
        failing_edges: failing,
        independent_edges: independent,
        needs_reverification,
        intents_without_validations: no_val_count,
        validation_pass_rate: pass_rate,
        blocked_validations,
        validation_pass_rate_runnable,
        open_issues: failing,
    };

    let gs = graph_state_from_snapshot(db, &snapshot)?;
    let outside = uninspected_outside_queues_from_snapshot(&snapshot);
    let align_count =
        crate::db::queries::align_candidates_from_snapshot(db, &snapshot)?.len() as i64;
    let prove = crate::db::queries::prove_candidates(db)?;
    let in_prove: std::collections::HashSet<&str> =
        prove.iter().map(|(h, _)| h.id.as_str()).collect();
    let adopt_count = crate::db::queries::list_hypotheses(db, Some("supported"))?
        .iter()
        .filter(|h| !in_prove.contains(h.id.as_str()))
        .count() as i64;
    let human_gated = align_count + adopt_count + outside.blocked_validations;
    let export_freshness = match crate::db::queries::committed_export_stale(db, root)? {
        Some(true) => "stale",
        Some(false) => "fresh",
        None => "absent",
    };

    if printer.json {
        let mut v = serde_json::to_value(&report)?;
        if let Some(obj) = v.as_object_mut() {
            obj.insert("graph_state".to_string(), serde_json::to_value(&gs)?);
            obj.insert(
                "uninspected_outside_queues".to_string(),
                serde_json::to_value(&outside)?,
            );
            obj.insert(
                "committed_export".to_string(),
                serde_json::json!(export_freshness),
            );
            obj.insert("human_gated".to_string(), serde_json::json!({
                "total": human_gated,
                "align_drift_suspects": align_count,
                "adopt_rulings": adopt_count,
                "blocked_validations": outside.blocked_validations,
                "note": if human_gated > 0 {
                    "These need the USER. Drain autonomous queues now; batch these into ONE agenda for the next conversation window (`loom next --all` tags each queue's gate)."
                } else { "" },
            }));
            if export_freshness == "stale" {
                obj.insert(
                    "committed_export_action".to_string(),
                    serde_json::json!("loom export   (the committed loom.graph.json drifted from the live graph — refresh it before committing code)"),
                );
            }
        }
        printer.print_json(&v);
    } else {
        println!("{}", fmt_status(&report));
        println!();
        println!("  {}", fmt_pulse(&gs));
        if outside.implements + outside.blocked_validations > 0 {
            println!(
                "  ⓘ {} uninspected edge(s) sit outside the work queues: {} structural IMPLEMENTS (grounding assertions, not verdicts), {} on blocked validations (`loom validation list` shows the recorded reasons).",
                outside.implements + outside.blocked_validations,
                outside.implements, outside.blocked_validations
            );
        }
        if human_gated > 0 {
            println!(
                "  ⚑ {human_gated} item(s) need the user: {align_count} align drift suspect(s), {adopt_count} adopt ruling(s), {} blocked proof(s). Batch them into one agenda; drain autonomous queues meanwhile (`loom next --all` tags each queue's gate).",
                outside.blocked_validations
            );
        }
        if export_freshness == "stale" {
            println!(
                "  ⚠ committed loom.graph.json is STALE — `loom export` before committing code."
            );
        }
        println!("  → Next: {}", gs.next_action);
    }
    Ok(())
}
