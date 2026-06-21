use anyhow::Result;

use crate::db::queries::{
    completeness_gaps_from_snapshot, current_blocked_validation_ids,
    edge_status_counts_from_snapshot, failing_governs_from_snapshot,
    intents_without_validations_from_snapshot, recent_passing_from_snapshot,
    status_report_from_snapshot, top_intents_by_centrality_from_snapshot,
    vertical_completeness_from_snapshot, QuerySnapshot, VerticalCompleteness,
};
use crate::db::{GraphReadHandle, GraphReadRepository};
use crate::output::{fmt_status, Printer};
use crate::types::{
    FullReport, Governs, Intent, IntentCentrality, RelatesTo, StatusReport, Validation,
};

/// Per-section list cap. On a large graph these lists run to thousands of
/// entries; an unbounded `report` (human OR --json) buries the headline and
/// floods an agent's context. The section header always states the true total.
const REPORT_LIST_CAP: usize = 50;

struct ReportData {
    status: StatusReport,
    deprecated_intents: i64,
    top_intents: Vec<IntentCentrality>,
    intents_no_val: Vec<Intent>,
    failing_governs: Vec<Governs>,
    recent: Vec<RelatesTo>,
    by_status: std::collections::HashMap<String, i64>,
    gaps: Vec<String>,
    vc: VerticalCompleteness,
    blocked: Vec<Validation>,
}

pub fn run(printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let store = GraphReadHandle::open(&cwd)?;
    run_with_db(&store, &cwd, printer)
}

fn report_data_from_snapshot(snapshot: &QuerySnapshot, total_all_intents: i64) -> ReportData {
    let status = status_report_from_snapshot(snapshot);
    let deprecated_intents = total_all_intents - status.total_intents;
    let top_intents = top_intents_by_centrality_from_snapshot(snapshot, 5);
    let intents_no_val = intents_without_validations_from_snapshot(snapshot);
    let failing_governs = failing_governs_from_snapshot(snapshot);
    let recent = recent_passing_from_snapshot(snapshot, 10);
    let by_status = edge_status_counts_from_snapshot(snapshot);
    let gaps = completeness_gaps_from_snapshot(snapshot);
    let vc = vertical_completeness_from_snapshot(snapshot);
    // Use the shared current-blocked classifier so `report` agrees with `status`
    // (a blocked validation whose target intent is deferred is parked, not
    // current debt; previously report counted raw last_result=="blocked").
    let current_blocked = current_blocked_validation_ids(snapshot);
    let blocked = snapshot
        .validations
        .iter()
        .filter(|validation| current_blocked.contains(validation.id.as_str()))
        .cloned()
        .collect();

    ReportData {
        status,
        deprecated_intents,
        top_intents,
        intents_no_val,
        failing_governs,
        recent,
        by_status,
        gaps,
        vc,
        blocked,
    }
}

pub fn run_with_db(
    db: &dyn GraphReadRepository,
    _root: &std::path::Path,
    printer: &Printer,
) -> Result<()> {
    let snapshot = db.query_snapshot()?;
    let total_all_intents = db.count_intents_including_deprecated()?;
    render_report(
        report_data_from_snapshot(&snapshot, total_all_intents),
        printer,
    )
}

fn render_report(data: ReportData, printer: &Printer) -> Result<()> {
    let ReportData {
        status,
        deprecated_intents,
        top_intents,
        intents_no_val,
        failing_governs,
        recent,
        by_status,
        gaps,
        vc,
        blocked,
    } = data;

    if printer.json {
        // Cap the unbounded lists, but keep the true totals so the consumer
        // knows the list was clipped and can dig deeper (`loom next`, `loom coverage`).
        let intents_no_val_total = intents_no_val.len();
        let gaps_total = gaps.len();
        let intents_no_val_capped: Vec<Intent> = intents_no_val
            .iter()
            .take(REPORT_LIST_CAP)
            .cloned()
            .collect();
        let gaps_capped: Vec<String> = gaps.iter().take(REPORT_LIST_CAP).cloned().collect();
        let report = FullReport {
            status,
            top_intents_by_centrality: top_intents,
            intents_without_validations: intents_no_val_capped,
            failing_governs,
            recent_passing: recent,
            edge_counts_by_status: by_status,
        };
        let mut v = serde_json::to_value(&report)?;
        if let Some(obj) = v.as_object_mut() {
            obj.insert(
                "deprecated_intents".to_string(),
                serde_json::json!(deprecated_intents),
            );
            obj.insert(
                "completeness_gaps".to_string(),
                serde_json::to_value(&gaps_capped)?,
            );
            obj.insert(
                "intents_without_validations_total".to_string(),
                serde_json::json!(intents_no_val_total),
            );
            obj.insert(
                "completeness_gaps_total".to_string(),
                serde_json::json!(gaps_total),
            );
            obj.insert(
                "vertical_completeness".to_string(),
                serde_json::to_value(&vc)?,
            );
            obj.insert(
                "blocked_validations".to_string(),
                serde_json::to_value(&blocked)?,
            );
        }
        printer.print_json(&v);
        return Ok(());
    }

    // ---- Human-readable report ----
    println!("══ loom report ═════════════════════════════════════════════════════");
    println!();
    println!("{}", fmt_status(&status));
    if deprecated_intents > 0 {
        println!(
            "  (+{deprecated_intents} deprecated intent(s) not in the count above — `loom intent list` shows all)"
        );
    }
    println!();

    println!("── Top Intents by RELATES_TO Centrality ─────────────────────────────");
    println!("  (degree counts horizontal RELATES_TO links only — a root with many");
    println!("   HIERARCHY children can still show degree 0; that's expected.)");
    if top_intents.is_empty() {
        println!("  (none)");
    } else {
        for ic in &top_intents {
            println!(
                "  degree={deg:<4}  {name}  ({id})",
                deg = ic.degree,
                name = ic.intent.name,
                id = ic.intent.id,
            );
        }
    }
    println!();

    println!("── Edge Counts by Inspection Status ─────────────────────────────────");
    let mut sorted_statuses: Vec<_> = by_status.iter().collect();
    sorted_statuses.sort_by_key(|(status, _)| *status);
    for (s, count) in &sorted_statuses {
        // The summary block above reports the ACTIONABLE (queue-served)
        // uninspected count; this raw per-status tally also includes stale
        // blocked-validation edges no queue serves. Spell out the gap so the two
        // "uninspected" numbers don't read as a self-contradiction.
        if *s == "uninspected" && **count != status.uninspected_edges {
            let stale = (**count - status.uninspected_edges).max(0);
            println!(
                "  {s:<30}  {count}  (raw; {} queue-served + {stale} stale blocked-validation edge(s))",
                status.uninspected_edges
            );
        } else {
            println!("  {s:<30}  {count}");
        }
    }
    println!();

    println!(
        "── Intents with No Validations ({}) ─────────────────────────────────",
        intents_no_val.len()
    );
    if intents_no_val.is_empty() {
        println!("  ✓ All intents have at least one validation.");
    } else {
        for i in intents_no_val.iter().take(REPORT_LIST_CAP) {
            println!("  [RISKY]  {}  ({})", i.name, i.id);
        }
        if intents_no_val.len() > REPORT_LIST_CAP {
            println!(
                "  … and {} more (`loom next --mode validate` works the queue)",
                intents_no_val.len() - REPORT_LIST_CAP
            );
        }
        println!();
        println!(
            "  → Add validations with `loom validation add` and link via `loom edge validates`."
        );
    }
    println!();

    println!(
        "── Failing GOVERNS Edges ({}) ──────────────────────────────────────",
        failing_governs.len()
    );
    if failing_governs.is_empty() {
        println!("  ✓ No rule violations found.");
    } else {
        for g in &failing_governs {
            println!(
                "  [failing]  rule={rname}  intent={iname}  evidence={}",
                truncate_chars(&g.evidence, 80),
                rname = g.rule_name,
                iname = g.intent_name,
            );
        }
    }
    println!();

    if !blocked.is_empty() {
        println!(
            "── Blocked Validations ({}) ─────────────────────────────────────────",
            blocked.len()
        );
        for v in &blocked {
            println!("  ⊘ {}  ({})", v.name, v.id);
        }
        println!("  → Waiting on something external (the why is on each VALIDATES edge).");
        println!(
            "    Unblock with `loom validation mark <id> --result passed|failed --evidence …`."
        );
        println!();
    }

    println!(
        "── Recent Passing Edges ({}) ────────────────────────────────────────",
        recent.len()
    );
    if recent.is_empty() {
        println!("  (none)");
    } else {
        for e in &recent {
            println!(
                "  {}  {} → {}  criterion='{}'",
                e.id,
                e.from_name,
                e.to_name,
                truncate_chars(&e.criterion, 60),
            );
        }
    }
    println!();

    println!("── Vertical Completeness (the binding spine) ────────────────────────");
    println!(
        "  {}  tree: {} root(s) · {} leaf intent(s)  [{}]",
        if vc.complete { "✓" } else { "✗" },
        vc.roots,
        vc.leaves,
        if vc.multi_parent.is_empty() && !vc.cycle {
            "well-formed"
        } else {
            "MALFORMED — see `loom doctor`"
        },
    );
    if !vc.unrealized_leaves.is_empty() {
        println!("  ✗ unrealized leaf intents (implemented but no code grounds them):");
        for n in vc.unrealized_leaves.iter().take(40) {
            println!("      - {}", n);
        }
        if let Some(m) = crate::output::more_marker(
            vc.unrealized_leaves.len(),
            40,
            "`loom report --json` for the full list",
        ) {
            println!("      {m}");
        }
        println!("    → `loom edge implement <intent> <codefile>` or decompose with `loom edge hierarchy`.");
    }
    if !vc.unremoved_leaves.is_empty() {
        println!("  ✗ to_be_removed leaf intents whose code is still present (cleanup not done):");
        for n in vc.unremoved_leaves.iter().take(40) {
            println!("      - {}", n);
        }
        if let Some(m) = crate::output::more_marker(
            vc.unremoved_leaves.len(),
            40,
            "`loom report --json` for the full list",
        ) {
            println!("      {m}");
        }
        println!("    → delete the code and unground it; cleanup is done by absence (`loom next --mode build`).");
    }
    if !vc.unreached_codefiles.is_empty() {
        println!("  ✗ CodeFiles reached by no intent (code with no recorded purpose):");
        for p in vc.unreached_codefiles.iter().take(40) {
            println!("      - {}", p);
        }
        if let Some(m) = crate::output::more_marker(
            vc.unreached_codefiles.len(),
            40,
            "`loom report --json` for the full list",
        ) {
            println!("      {m}");
        }
        println!("    → ground them (`loom edge implement`) or drop the CodeFile.");
    }
    if !vc.non_system_roots.is_empty() {
        println!(
            "  ⚠ root intents not at `system` level (advisory — does not block completeness):"
        );
        for n in vc.non_system_roots.iter().take(40) {
            println!("      - {}", n);
        }
        if let Some(m) = crate::output::more_marker(
            vc.non_system_roots.len(),
            40,
            "`loom report --json` for the full list",
        ) {
            println!("      {m}");
        }
    }
    if vc.complete {
        if gaps.is_empty() {
            println!("  ✓ Every implemented leaf is realized and every CodeFile is reached.");
        } else {
            // The leaf SPINE is sound, but the broader completeness gaps below
            // (confirmed non-leaf intents not grounded, missing proof, path
            // coverage) bound the headline. A bare ✓ directly above "N
            // Completeness Gaps" reads as "completeness done" — scope-label it
            // to LEAF and reconcile with the adjacent gaps so the ✓ can't
            // out-rank the co-located negatives.
            println!(
                "  ✓ Leaf spine sound — every implemented LEAF is realized and every CodeFile is reached ({} broader completeness gap(s) below: confirmed non-leaf intents, missing proof, or path coverage).",
                gaps.len()
            );
        }
    }
    println!();

    println!(
        "── Completeness Gaps ({}) ───────────────────────────────────────────",
        gaps.len()
    );
    if gaps.is_empty() {
        println!("  ✓ No gaps — grounded, validated, and error/fallback paths covered.");
    } else {
        for g in gaps.iter().take(REPORT_LIST_CAP) {
            println!("  • {}", g);
        }
        if gaps.len() > REPORT_LIST_CAP {
            println!("  … and {} more", gaps.len() - REPORT_LIST_CAP);
        }
    }

    Ok(())
}
fn truncate_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}
