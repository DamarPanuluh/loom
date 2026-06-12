use anyhow::Result;

use crate::db::{ensure_initialized, GrafeoDb};
use crate::db::queries::{
    count_all_edges_by_inspection_status, count_codefiles, count_intents,
    count_validations, graph_state, intents_without_validations, validation_pass_rate,
};
use crate::output::{fmt_pulse, fmt_status, Printer};
use crate::types::StatusReport;

pub fn run(printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    let total_intents     = count_intents(&db)?;
    let total_codefiles   = count_codefiles(&db)?;
    let total_validations = count_validations(&db)?;

    let by_status            = count_all_edges_by_inspection_status(&db)?;
    let total_edges          = by_status.values().sum::<i64>();
    let uninspected          = *by_status.get("uninspected").unwrap_or(&0);
    let passing              = *by_status.get("passing").unwrap_or(&0);
    let failing              = *by_status.get("failing").unwrap_or(&0);
    let independent          = *by_status.get("independent").unwrap_or(&0);
    let needs_reverification = *by_status.get("needs_reverification").unwrap_or(&0);

    let pass_rate     = validation_pass_rate(&db)?;
    let no_val_count  = intents_without_validations(&db)?.len() as i64;

    let report = StatusReport {
        total_intents,
        total_codefiles,
        total_validations,
        total_edges,
        uninspected_edges:           uninspected,
        passing_edges:               passing,
        failing_edges:               failing,
        independent_edges:           independent,
        needs_reverification,
        intents_without_validations: no_val_count,
        validation_pass_rate:        pass_rate,
        open_issues:                 failing,
    };

    let gs = graph_state(&db)?;
    // The raw `uninspected` histogram counts edge types the work queues
    // deliberately don't serve (structural IMPLEMENTS, blocked proofs) — name
    // them, so `uninspected_edges: 1, unresolved_edges: 0` reconciles itself.
    let outside = crate::db::queries::uninspected_outside_queues(&db)?;
    // The oscillation summary — what needs the USER, computed the same way
    // `loom next --all` gates its queues: align drift suspects (the graph
    // cannot read heads), supported hypotheses awaiting the adopt/reject
    // ruling, and blocked proofs (an external prerequisite someone must
    // provide). The agent drains everything else alone; these get batched
    // into one agenda for the next conversation window.
    let align_count = crate::db::queries::align_candidates(&db)?.len() as i64;
    let prove = crate::db::queries::prove_candidates(&db)?;
    let in_prove: std::collections::HashSet<&str> =
        prove.iter().map(|(h, _)| h.id.as_str()).collect();
    let adopt_count = crate::db::queries::list_hypotheses(&db, Some("supported"))?
        .iter()
        .filter(|h| !in_prove.contains(h.id.as_str()))
        .count() as i64;
    let human_gated = align_count + adopt_count + outside.blocked_validations;
    // The travel format must move WITH graph changes — surface drift in-band
    // (status is an orientation command; the agent reads this, no repo
    // plumbing required). "fresh" | "stale" | "absent".
    let export_freshness = match crate::db::queries::committed_export_stale(&db, &cwd)? {
        Some(true) => "stale",
        Some(false) => "fresh",
        None => "absent",
    };

    if printer.json {
        // Keep the StatusReport fields flat, add the graph_state pulse.
        let mut v = serde_json::to_value(&report)?;
        if let Some(obj) = v.as_object_mut() {
            obj.insert("graph_state".to_string(), serde_json::to_value(&gs)?);
            obj.insert(
                "uninspected_outside_queues".to_string(),
                serde_json::to_value(&outside)?,
            );
            obj.insert("committed_export".to_string(), serde_json::json!(export_freshness));
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
            println!("  ⚠ committed loom.graph.json is STALE — `loom export` before committing code.");
        }
        println!("  → Next: {}", gs.next_action);
    }
    Ok(())
}
