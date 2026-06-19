use anyhow::Result;

use crate::db::queries::{
    blocked_validation_summary_from_snapshot, clone_suggestions, cochange_suggestions,
    lane_depths_from_snapshot, proof_locality_suggestions, shotgun_surgery_suggestions,
    status_report_from_snapshot, uninspected_outside_queues_from_snapshot,
    BlockedValidationSummary, GraphState, LaneDepths, QuerySnapshot, Smell,
    UninspectedOutsideQueues, GATE_REASON_MANUAL_ACCEPTANCE,
};
use crate::db::{GraphReadHandle, GraphReadRepository};
use crate::output::{fmt_pulse, Printer};
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
    /// `None` when the audit scan was deferred (`computed: false`) — serialized
    /// as JSON `null` so a programmatic consumer keying on this field cannot
    /// mistake "no scan ran" for "scan ran and found zero" (the false-green
    /// remnant: a literal `0` next to `computed:false` read as "audit clean").
    /// `Some(n)` only when the scan actually ran.
    open_findings: Option<usize>,
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

#[derive(Debug, Clone, Copy, serde::Serialize)]
struct IntakeCounts {
    untriaged: i64,
    triaged: i64,
    deferred: i64,
}

impl IntakeCounts {
    fn active(self) -> i64 {
        self.untriaged + self.triaged
    }
}

impl PopulatePulse {
    fn from_plan(plan: &crate::commands::populate::PopulatePlan) -> Self {
        Self {
            total: plan.pending_count(),
            interface_from_sagas: plan.interface_from_sagas.sagas_needing_repopulate,
            interface_gaps: plan.interface_gaps.total(),
            next_command: crate::commands::POPULATE_NEXT_COMMAND.to_string(),
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
    let blocked = blocked_validation_summary_from_snapshot(&snapshot);
    let intake = intake_counts(db)?;
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
        &blocked,
        intake,
        advisories,
        audit,
        align_count,
        adopt_count,
        export_freshness,
        printer,
    )
}

fn intake_counts(db: &dyn GraphReadRepository) -> Result<IntakeCounts> {
    let items = db.list_inbox_items(None, None)?;
    Ok(IntakeCounts {
        untriaged: items.iter().filter(|item| item.status == "new").count() as i64,
        triaged: items.iter().filter(|item| item.status == "triaged").count() as i64,
        deferred: items
            .iter()
            .filter(|item| item.status == "deferred")
            .count() as i64,
    })
}

fn should_compute_audit_pulse(gs: &GraphState) -> bool {
    matches!(gs.phase.as_str(), "audit" | "complete")
}

fn deferred_audit_pulse() -> AuditPulse {
    AuditPulse {
        computed: false,
        open_findings: None,
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
        open_findings: Some(open.len()),
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

fn gate_reason_counts(
    align_count: i64,
    adopt_count: i64,
    blocked: &BlockedValidationSummary,
) -> serde_json::Value {
    let mut counts = std::collections::BTreeMap::<String, i64>::new();
    if align_count > 0 {
        counts.insert("user_intent_confirmation".to_string(), align_count);
    }
    if adopt_count > 0 {
        counts.insert(GATE_REASON_MANUAL_ACCEPTANCE.to_string(), adopt_count);
    }
    for item in blocked.human_gate_reasons() {
        *counts.entry(item.reason.clone()).or_insert(0) += item.count;
    }
    serde_json::json!(counts)
}

#[derive(Debug, Clone, Copy)]
struct CompletionTotals {
    blocked_validation_audit: i64,
    human_blocked: i64,
    required_autonomous: i64,
    required_human: i64,
}

fn completion_totals(
    lanes: &LaneDepths,
    populate: &PopulatePulse,
    align_count: i64,
    adopt_count: i64,
    blocked: &BlockedValidationSummary,
) -> CompletionTotals {
    let blocked_validation_audit = blocked.autonomous_validation_count();
    let human_blocked = blocked.human_validation_count();
    CompletionTotals {
        blocked_validation_audit,
        human_blocked,
        required_autonomous: populate.total as i64
            + lanes.build
            + lanes.fix
            + lanes.validate
            + lanes.quality
            + blocked_validation_audit,
        required_human: align_count + adopt_count + human_blocked,
    }
}

fn completion_json(
    lanes: &LaneDepths,
    populate: &PopulatePulse,
    align_count: i64,
    adopt_count: i64,
    blocked: &BlockedValidationSummary,
    gs: &GraphState,
) -> serde_json::Value {
    let totals = completion_totals(lanes, populate, align_count, adopt_count, blocked);
    let mut completion = serde_json::Map::new();
    completion.insert(
        "required_autonomous_debt".to_string(),
        serde_json::json!({
            "total": totals.required_autonomous,
            "populate": populate.total,
            "build": lanes.build,
            "fix": lanes.fix,
            "validate": lanes.validate,
            "quality": lanes.quality,
            "blocked_validation_audit": totals.blocked_validation_audit,
        }),
    );
    completion.insert(
        crate::commands::REQUIRED_HUMAN_GATED_DEBT_KEY.to_string(),
        serde_json::json!({
            "total": totals.required_human,
            "align_confirmations": align_count,
            "adopt_rulings": adopt_count,
            "blocked_validations": totals.human_blocked,
            "affected_proof_edges": blocked.affected_proof_edges,
            "by_gate_reason": gate_reason_counts(align_count, adopt_count, blocked),
        }),
    );
    completion.insert(
        "optional_graph_enrichment".to_string(),
        serde_json::json!({
            "unexplored_relationship_pairs": gs.unexplored_pairs,
            "horizontally_explored": gs.horizontally_explored,
            "note_hygiene": gs.note_hygiene,
        }),
    );
    serde_json::Value::Object(completion)
}

#[allow(clippy::too_many_arguments)]
fn render_status(
    report: &StatusReport,
    gs: &GraphState,
    lanes: &LaneDepths,
    populate: &PopulatePulse,
    outside: &UninspectedOutsideQueues,
    blocked: &BlockedValidationSummary,
    intake: IntakeCounts,
    advisories: AdvisoryCounts,
    audit: AuditPulse,
    align_count: i64,
    adopt_count: i64,
    export_freshness: &str,
    printer: &Printer,
) -> Result<()> {
    let totals = completion_totals(lanes, populate, align_count, adopt_count, blocked);
    let human_gated = totals.required_human;

    if printer.json {
        let mut v = serde_json::to_value(report)?;
        if let Some(obj) = v.as_object_mut() {
            obj.insert("graph_state".to_string(), serde_json::to_value(gs)?);
            obj.insert("other_lanes".to_string(), other_lanes_json(lanes, populate));
            obj.insert(
                "completion".to_string(),
                completion_json(lanes, populate, align_count, adopt_count, blocked, gs),
            );
            obj.insert(
                "validation_health".to_string(),
                serde_json::json!({
                    "runnable_pass_rate": report.validation_pass_rate_runnable,
                    "all_pass_rate": report.validation_pass_rate,
                    "blocked_validations": blocked.validations,
                    "affected_proof_edges": blocked.affected_proof_edges,
                    "blocked_by_gate_reason": blocked.by_reason,
                }),
            );
            obj.insert("populate".to_string(), serde_json::to_value(populate)?);
            obj.insert("intake".to_string(), serde_json::to_value(intake)?);
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
                "blocked_validations": totals.human_blocked,
                "blocked_validation_audits": totals.blocked_validation_audit,
                "affected_proof_edges": blocked.affected_proof_edges,
                "by_gate_reason": gate_reason_counts(align_count, adopt_count, blocked),
                "note": if human_gated > 0 {
                    "These need the USER or external prerequisites. Drain autonomous queues now; batch true user decisions into ONE agenda (`loom next --mode align --take 50` for align)."
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
        render_plain_status(
            report,
            gs,
            lanes,
            populate,
            outside,
            blocked,
            intake,
            advisories,
            &audit,
            align_count,
            adopt_count,
            export_freshness,
            totals,
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_plain_status(
    report: &StatusReport,
    gs: &GraphState,
    lanes: &LaneDepths,
    populate: &PopulatePulse,
    outside: &UninspectedOutsideQueues,
    blocked: &BlockedValidationSummary,
    intake: IntakeCounts,
    advisories: AdvisoryCounts,
    audit: &AuditPulse,
    align_count: i64,
    adopt_count: i64,
    export_freshness: &str,
    totals: CompletionTotals,
) {
    println!("Completion:");
    println!("  required autonomous debt: {}", totals.required_autonomous);
    println!(
        "    populate {} · build {} · fix {} · validate {} · quality {} · blocker audit {}",
        populate.total,
        lanes.build,
        lanes.fix,
        lanes.validate,
        lanes.quality,
        totals.blocked_validation_audit
    );
    println!("  required human-gated debt: {}", totals.required_human);
    println!(
        "    align confirmations {} · adopt rulings {} · blocked validations {}",
        align_count, adopt_count, totals.human_blocked
    );
    println!(
        "  optional graph enrichment: {} relationship pair(s), not required for done",
        gs.unexplored_pairs
    );
    if intake.active() > 0 || intake.deferred > 0 {
        println!(
            "  inbox intake: {} untriaged · {} triaged · {} deferred (candidates, not graph truth)",
            intake.untriaged, intake.triaged, intake.deferred
        );
    }
    println!();
    println!("Validation Health:");
    println!(
        "  runnable validations: {:.1}% passing",
        report.validation_pass_rate_runnable * 100.0
    );
    if blocked.validations > 0 {
        println!(
            "  blocked validations: {} awaiting prerequisites, affecting {} proof edge(s)",
            blocked.validations, blocked.affected_proof_edges
        );
    } else {
        println!("  blocked validations: 0");
    }
    println!(
        "  all validations: {:.1}% passing including blocked prerequisites",
        report.validation_pass_rate * 100.0
    );
    println!();
    println!("Inventory:");
    println!(
        "  nodes: {} intents · {} code files · {} validations",
        report.total_intents, report.total_codefiles, report.total_validations
    );
    println!(
        "  raw edge states: {} total · {} passing · {} independent · {} failing · {} stale · {} uninspected",
        report.total_edges,
        report.passing_edges,
        report.independent_edges,
        report.failing_edges,
        report.needs_reverification,
        report.uninspected_edges
    );
    println!(
        "  proof coverage: {} intent(s) without validation",
        report.intents_without_validations
    );
    println!();
    println!("  {}", fmt_pulse(gs));
    if blocked.validations > 0 {
        println!(
            "  validations: runnable {:.0}% passing; {} blocked validation(s) await prerequisites, affecting {} proof edge(s).",
            report.validation_pass_rate_runnable * 100.0,
            blocked.validations,
            blocked.affected_proof_edges
        );
    }
    if outside.implements + blocked.affected_proof_edges > 0 {
        println!(
            "  ⓘ {} uninspected edge(s) sit outside the work queues: {} structural IMPLEMENTS (grounding assertions, not verdicts), {} on blocked validations (`loom validation list` shows the recorded reasons).",
            outside.implements + blocked.affected_proof_edges,
            outside.implements, blocked.affected_proof_edges
        );
    }
    if totals.required_human > 0 {
        println!(
            "  ⚑ {} human/prerequisite-gated item(s): {align_count} align drift suspect(s), {adopt_count} adopt ruling(s), {} blocked validation(s). Batch align with `loom next --mode align --take 50`; drain autonomous queues meanwhile.",
            totals.required_human, totals.human_blocked
        );
    }
    if totals.blocked_validation_audit > 0 {
        println!(
            "  autonomous blocker audit: {} blocked validation(s) look locally fixable or stale; inspect `loom validation list --result blocked --limit 0`.",
            totals.blocked_validation_audit
        );
    }
    if export_freshness == "stale" {
        println!("  {}", crate::commands::EXPORT_STALE_WARNING);
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
    if audit.computed && audit.open_findings.unwrap_or(0) > 0 {
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
            audit.open_findings.unwrap_or(0),
            kinds,
            top,
            audit.recommended_command
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
