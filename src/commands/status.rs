use anyhow::Result;

use crate::db::queries::{
    lane_depths_from_snapshot, status_report_from_snapshot,
    uninspected_outside_queues_from_snapshot, GraphState, LaneDepths, UninspectedOutsideQueues,
};
use crate::db::{GraphReadHandle, GraphReadRepository};
use crate::output::{fmt_pulse, fmt_status, Printer};
use crate::types::StatusReport;

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
    // One graph scan — every count below is derived from the snapshot instead
    // of re-querying nodes/edges (the scale benchmark's status hot path).
    let snapshot = db.query_snapshot()?;
    let report = status_report_from_snapshot(&snapshot);
    let gs = db.graph_state(&snapshot)?;
    let lanes = lane_depths_from_snapshot(&snapshot);
    let outside = uninspected_outside_queues_from_snapshot(&snapshot);
    let align_count = db.align_candidate_count(&snapshot)?;
    let prove = db.prove_candidates(&snapshot)?;
    let in_prove: std::collections::HashSet<&str> =
        prove.iter().map(|(h, _)| h.id.as_str()).collect();
    let adopt_count = db
        .list_hypotheses(Some("supported"))?
        .iter()
        .filter(|h| !in_prove.contains(h.id.as_str()))
        .count() as i64;
    let export_freshness = match db.committed_export_stale(root)? {
        Some(true) => "stale",
        Some(false) => "fresh",
        None => "absent",
    };

    render_status(
        &report,
        &gs,
        &lanes,
        &outside,
        align_count,
        adopt_count,
        export_freshness,
        printer,
    )
}

/// Format the "other open lanes" footer: the autonomous work lanes that have
/// items AND aren't the lane the compass already pointed at. `discovery` (the
/// optional N×N grid, already signalled by `horizontal ○`) and the human-gated
/// align/adopt items (already on the `⚑` line) are intentionally omitted — this
/// is peripheral vision over the *autonomous closable* queues, so the single
/// pointer can't hide that other lanes have work. Empty when nothing qualifies.
fn other_lanes_line(lanes: &LaneDepths, phase: &str) -> Option<String> {
    let parts: Vec<String> = [
        ("build", lanes.build),
        ("fix", lanes.fix),
        ("validate", lanes.validate),
        ("quality", lanes.quality),
    ]
    .into_iter()
    .filter(|(name, count)| *count > 0 && *name != phase)
    .map(|(name, count)| format!("{name} {count}"))
    .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}

#[allow(clippy::too_many_arguments)]
fn render_status(
    report: &StatusReport,
    gs: &GraphState,
    lanes: &LaneDepths,
    outside: &UninspectedOutsideQueues,
    align_count: i64,
    adopt_count: i64,
    export_freshness: &str,
    printer: &Printer,
) -> Result<()> {
    let human_gated = align_count + adopt_count + outside.blocked_validations;

    if printer.json {
        let mut v = serde_json::to_value(report)?;
        if let Some(obj) = v.as_object_mut() {
            obj.insert("graph_state".to_string(), serde_json::to_value(gs)?);
            obj.insert("other_lanes".to_string(), serde_json::to_value(lanes)?);
            obj.insert(
                "uninspected_outside_queues".to_string(),
                serde_json::to_value(outside)?,
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
        println!("{}", fmt_status(report));
        println!();
        println!("  {}", fmt_pulse(gs));
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
        if let Some(others) = other_lanes_line(lanes, &gs.phase) {
            println!("  other open lanes: {others}");
        }
        // The verb signals the compass's own confidence: a directive phase (a
        // failure or binding gap) reads as a command; a recommended phase
        // (discretionary work the agent may sequence against the lanes above)
        // reads as a suggestion the agent can override with context loom lacks.
        let anchor = if gs.next_kind == "recommended" {
            "→ Recommended"
        } else {
            "→ Next"
        };
        println!("  {anchor}: {}", gs.next_action);
    }
    Ok(())
}
