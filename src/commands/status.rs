use anyhow::Result;

use crate::db::queries::{
    clone_suggestions, cochange_suggestions, lane_depths_from_snapshot, proof_locality_suggestions,
    shotgun_surgery_suggestions, status_report_from_snapshot,
    uninspected_outside_queues_from_snapshot, GraphState, LaneDepths, QuerySnapshot, Smell,
    UninspectedOutsideQueues,
};
use crate::db::{GraphReadHandle, GraphReadRepository};
use crate::output::{fmt_pulse, fmt_status, Printer};
use crate::types::{Ignore, StatusReport};

#[derive(Debug, Clone, Copy, serde::Serialize)]
struct AdvisoryCounts {
    total: usize,
    code_clones: usize,
    cochange_suggestions: usize,
    shotgun_surgery: usize,
    proof_locality_suggestions: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
struct AuditPulse {
    computed: bool,
    open_findings: usize,
    top_kinds: Vec<KindCount>,
    top_findings: Vec<FindingPulse>,
    recommended_command: String,
    note: String,
}

#[derive(Debug, Clone, serde::Serialize)]
struct KindCount {
    kind: String,
    count: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
struct FindingPulse {
    kind: String,
    summary: String,
    score: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
struct PopulatePulse {
    total: usize,
    interface_from_sagas: usize,
    interface_gaps: usize,
    next_command: String,
}

impl PopulatePulse {
    fn from_plan(plan: &crate::commands::populate::PopulatePlan) -> Self {
        Self {
            total: plan.pending_count(),
            interface_from_sagas: plan.interface_from_sagas.sagas_needing_repopulate,
            interface_gaps: plan.interface_gaps.total(),
            next_command: "loom next --mode populate".to_string(),
        }
    }
}

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
    let populate = crate::commands::populate::plan_with_repo(db, root)?;
    let populate_pulse = PopulatePulse::from_plan(&populate);
    let outside = uninspected_outside_queues_from_snapshot(&snapshot);
    let ignores = db.list_ignores()?;
    let decision_notes = db.notes_by_kind("decision")?;
    let advisories = advisory_counts(root, &snapshot, &ignores, &decision_notes);
    let audit = if should_compute_audit_pulse(&gs) {
        audit_pulse(db.smell_report(&snapshot)?.open)
    } else {
        deferred_audit_pulse()
    };
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
        &populate_pulse,
        &outside,
        advisories,
        audit,
        align_count,
        adopt_count,
        export_freshness,
        printer,
    )
}

fn should_compute_audit_pulse(gs: &GraphState) -> bool {
    matches!(gs.phase.as_str(), "audit" | "complete")
}

fn deferred_audit_pulse() -> AuditPulse {
    AuditPulse {
        computed: false,
        open_findings: 0,
        top_kinds: Vec::new(),
        top_findings: Vec::new(),
        recommended_command: "loom smells --summary".to_string(),
        note: "Audit scan deferred on non-audit phases to keep status hot; run the recommended command for current findings.".to_string(),
    }
}

fn audit_pulse(open: Vec<Smell>) -> AuditPulse {
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for smell in &open {
        *counts.entry(smell.kind.clone()).or_insert(0) += 1;
    }
    let mut top_kinds: Vec<_> = counts
        .into_iter()
        .map(|(kind, count)| KindCount { kind, count })
        .collect();
    top_kinds.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.kind.cmp(&b.kind)));
    top_kinds.truncate(5);
    let top_findings = open
        .iter()
        .take(3)
        .map(|smell| FindingPulse {
            kind: smell.kind.clone(),
            summary: smell.summary.clone(),
            score: smell.score,
        })
        .collect();
    AuditPulse {
        computed: true,
        open_findings: open.len(),
        top_kinds,
        top_findings,
        recommended_command: "loom smells --summary".to_string(),
        note: String::new(),
    }
}

fn advisory_counts(
    root: &std::path::Path,
    snapshot: &QuerySnapshot,
    ignores: &[Ignore],
    decision_notes: &[crate::types::Note],
) -> AdvisoryCounts {
    let paths: std::collections::HashSet<String> =
        snapshot.codefiles.iter().map(|c| c.path.clone()).collect();
    let cc = crate::repo::git_cochange(root, &paths, 800);
    let (cochange_open, _) = crate::commands::smells::split_advisories_for_adjudication(
        snapshot,
        cochange_suggestions(snapshot, &cc.pairs, &cc.individual),
        decision_notes,
    );
    let cochange_suggestions = cochange_open.len();
    let (shotgun_open, _) = crate::commands::smells::split_advisories_for_adjudication(
        snapshot,
        shotgun_surgery_suggestions(snapshot, &cc.pairs, &cc.individual),
        decision_notes,
    );
    let shotgun_surgery = shotgun_open.len();
    let (proof_open, _) = crate::commands::smells::split_advisories_for_adjudication(
        snapshot,
        proof_locality_suggestions(snapshot),
        decision_notes,
    );
    let proof_locality_suggestions = proof_open.len();
    let clone_patterns: Vec<glob::Pattern> = ignores
        .iter()
        .filter_map(|i| glob::Pattern::new(&i.pattern).ok())
        .collect();
    let (clone_open, _) = crate::commands::smells::split_advisories_for_adjudication(
        snapshot,
        clone_suggestions(snapshot, &clone_patterns),
        decision_notes,
    );
    let code_clones = clone_open.len();
    AdvisoryCounts {
        total: code_clones + cochange_suggestions + shotgun_surgery + proof_locality_suggestions,
        code_clones,
        cochange_suggestions,
        shotgun_surgery,
        proof_locality_suggestions,
    }
}

/// Format the "other open lanes" footer: the autonomous work lanes that have
/// items AND aren't the lane the compass already pointed at. `discovery` (the
/// optional N×N grid, already signalled by `horizontal ○`) and the human-gated
/// align/adopt items (already on the `⚑` line) are intentionally omitted — this
/// is peripheral vision over the *autonomous closable* queues, so the single
/// pointer can't hide that other lanes have work. Empty when nothing qualifies.
fn other_lanes_line(lanes: &LaneDepths, populate: &PopulatePulse, phase: &str) -> Option<String> {
    let parts: Vec<String> = [
        ("populate", populate.total as i64),
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

fn other_lanes_json(lanes: &LaneDepths, populate: &PopulatePulse) -> serde_json::Value {
    serde_json::json!({
        "populate": populate.total,
        "build": lanes.build,
        "fix": lanes.fix,
        "validate": lanes.validate,
        "quality": lanes.quality,
    })
}

#[allow(clippy::too_many_arguments)]
fn render_status(
    report: &StatusReport,
    gs: &GraphState,
    lanes: &LaneDepths,
    populate: &PopulatePulse,
    outside: &UninspectedOutsideQueues,
    advisories: AdvisoryCounts,
    audit: AuditPulse,
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
            obj.insert("other_lanes".to_string(), other_lanes_json(lanes, populate));
            obj.insert("populate".to_string(), serde_json::to_value(populate)?);
            obj.insert(
                "uninspected_outside_queues".to_string(),
                serde_json::to_value(outside)?,
            );
            obj.insert("advisories".to_string(), serde_json::to_value(advisories)?);
            obj.insert("audit".to_string(), serde_json::to_value(&audit)?);
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
        if advisories.total > 0 {
            println!(
                "  advisories: {} waiting — code clones {} · co-change {} · shotgun {} · proof-locality {} (`loom smells --summary`).",
                advisories.total,
                advisories.code_clones,
                advisories.cochange_suggestions,
                advisories.shotgun_surgery,
                advisories.proof_locality_suggestions
            );
        }
        if audit.computed && audit.open_findings > 0 {
            let kinds = audit
                .top_kinds
                .iter()
                .map(|k| format!("{} {}", k.kind, k.count))
                .collect::<Vec<_>>()
                .join(" · ");
            let top = audit
                .top_findings
                .first()
                .map(|f| format!("; top: [{}] {}", f.kind, f.summary))
                .unwrap_or_default();
            println!(
                "  audit: {} open finding(s) — {}{} (`{}`).",
                audit.open_findings, kinds, top, audit.recommended_command
            );
        } else if !audit.computed {
            println!(
                "  audit: deferred while phase={} keeps another lane active (`{}`).",
                gs.phase, audit.recommended_command
            );
        }
        if populate.total > 0 {
            println!(
                "  populate: {} gap(s) waiting — interface backfill {} · interface gaps {} (`{}`).",
                populate.total,
                populate.interface_from_sagas,
                populate.interface_gaps,
                populate.next_command
            );
        }
        if let Some(others) = other_lanes_line(lanes, populate, &gs.phase) {
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
