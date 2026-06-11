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
            obj.insert("committed_export".to_string(), serde_json::json!(export_freshness));
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
        if export_freshness == "stale" {
            println!("  ⚠ committed loom.graph.json is STALE — `loom export` before committing code.");
        }
        println!("  → Next: {}", gs.next_action);
    }
    Ok(())
}
