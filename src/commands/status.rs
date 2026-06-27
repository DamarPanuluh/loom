use anyhow::Result;

use crate::db::queries::{
    blocked_validation_summary_from_snapshot, build_ladder, clone_suggestions,
    cochange_suggestions, lane_depths_from_snapshot, proof_locality_suggestions,
    review_candidates_from_snapshot, shotgun_surgery_suggestions, status_report_from_snapshot,
    uninspected_outside_queues_from_snapshot, BlockedValidationSummary, GraphState, LaneDepths,
    MaturityLadder, QuerySnapshot, Smell, SourceCorpusCoverage, UninspectedOutsideQueues,
    GATE_REASON_MANUAL_ACCEPTANCE,
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
struct CertificationRollup {
    /// Default user-facing certification. Loom's default policy is now
    /// excellence-oriented: a production-ready-but-messy codebase is yellow, not
    /// green. Consumers that only need deploy fitness can read `production`.
    overall: String,
    default_profile: String,
    map_integrity: String,
    behavior: String,
    quality: String,
    production: String,
    excellence: String,
    excellence_debt: usize,
    note: String,
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

#[derive(Debug, Clone, serde::Serialize)]
struct OneTurnPlan {
    focus_rung: String,
    focus_lane: String,
    agent_role: String,
    agent_export: String,
    guide_command: String,
    next_command: String,
    rule: String,
}

/// `honesty-next #2`: map-vs-territory, surfaced ALWAYS — not only at the
/// audit gate. On a red graph (e.g. phase=fix) the compass used to hide that
/// real files on disk weren't in the graph; the information existed (loom
/// coverage) but the one screen every driver reads buried it behind near-green.
/// This is the always-on disclosure: counts of files the graph doesn't account
/// for, with the same remedy language the audit gate uses. Computed in
/// `run()` (NOT in `graph_state`) so the disk-walk + content-hash cost is paid
/// by `loom status` alone, not every `graph_state` pulse (next/report/…).
#[derive(Debug, Clone, serde::Serialize)]
struct DiskPulse {
    /// files on disk the graph doesn't account for (no codefile, not ignored/delegated)
    unaccounted: usize,
    /// registered codefiles whose content drifted since the last sync
    drifted: usize,
    /// registered codefiles whose path no longer exists on disk (phantom map)
    missing: usize,
    total: usize,
    /// the one-line human message (mirrors the audit-gate language)
    message: String,
}

fn disk_pulse(
    snapshot: &QuerySnapshot,
    db: &dyn GraphReadRepository,
    root: &std::path::Path,
) -> Result<DiskPulse> {
    let ignores = db.list_ignores()?;
    let delegations = db.list_delegations()?;
    let disk = crate::repo::walk_files(root)?;
    let recon = crate::db::queries::integrity::disk_reconciliation_from_parts(
        &disk,
        &snapshot.codefiles,
        &ignores,
        &delegations,
        &|p| {
            std::fs::read(root.join(p))
                .ok()
                .map(|b| crate::repo::content_hash(&b))
        },
    );
    let unaccounted = recon.unaccounted_files.len();
    let drifted = recon.drifted_codefiles.len();
    let missing = recon.missing_codefiles.len();
    let total = unaccounted + drifted + missing;
    // Split by bucket — each needs a DIFFERENT remedy, and lumping them under
    // "on disk the graph doesn't account for" is wrong for `missing` (registered
    // but GONE from disk — not on disk at all) and points the AI at `loom
    // coverage`/`codefile add`, which don't fix deletions. coverage only shows
    // UNMAPPED, so a missing-dominated count there reads as "0 missed".
    let message = if total == 0 {
        "disk reconciled ✓ — nothing unmapped/drifted/missing.".to_string()
    } else {
        let mut parts: Vec<String> = Vec::new();
        if unaccounted > 0 {
            parts.push(format!(
                "{unaccounted} unmapped (on disk, not in the graph) — `loom coverage` to see them; `loom codefile add` + `loom edge implement` to map, or `loom ignore add <glob> --reason …` to exclude"
            ));
        }
        if drifted > 0 {
            parts.push(format!(
                "{drifted} drifted (content changed since last sync) — `loom sync` to re-hash"
            ));
        }
        if missing > 0 {
            parts.push(format!(
                "{missing} MISSING (registered, now gone from disk) — `loom codefile remove <path>` to drop them, or `loom sync` to detect"
            ));
        }
        format!("map ≠ territory: {}.", parts.join(" · "))
    };
    Ok(DiskPulse {
        unaccounted,
        drifted,
        missing,
        total,
        message,
    })
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
    // Open todos = the LLM-filled follow-up backlog. loom can't read their prose
    // to auto-clear them, but it surfaces the COUNT here — the always-run compass
    // that survives compaction — so an agent can't silently forget them. Advisory
    // (gates nothing); closed via `loom note resolve`.
    let open_todos = db
        .notes_by_kind("todo")?
        .into_iter()
        .filter(|n| n.resolution.is_empty())
        .count() as i64;
    let decision_notes = db.notes_by_kind("decision")?;
    let advisories = advisory_counts(root, &snapshot, &ignores, &decision_notes);
    // Open smells are computed once at the audit gate (phase audit|complete) and
    // reused for BOTH the audit pulse and the fully_proven badge's proof-locality.
    let (open_smells, excellence_debt_count) = if should_compute_audit_pulse(&gs) {
        let report = db.smell_report(&snapshot)?;
        let excellence_debt_count = report.advisory.len() + report.debt.len() + advisories.total;
        (report.open, excellence_debt_count)
    } else {
        (Vec::new(), advisories.total)
    };
    let audit = if should_compute_audit_pulse(&gs) {
        audit_pulse(open_smells.clone())
    } else {
        deferred_audit_pulse()
    };
    let align_count = db.align_candidate_count(&snapshot)?;
    let prove = db.prove_candidates(&snapshot)?;
    // Optional-but-autonomous lanes — surfaced so the compass can't hide them
    // (they are NOT human-gated and NOT counted in required debt).
    let optional_lanes = OptionalLanes {
        review: review_candidates_from_snapshot(&snapshot).len() as i64,
        prove: prove.len() as i64,
    };
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
    // honesty-next #2: map-vs-territory is ALWAYS surfaced (not only at the
    // audit gate), so a red graph can't hide files-on-disk the graph ignores.
    let disk = disk_pulse(&snapshot, db, root)?;

    // The maturity ladder — loom's certification vector. Rolled up from the
    // gates above (vertical spine, comprehensiveness ledgers, and the former
    // fully_proven gate set), it REPLACES the scattered badges. The assembly is
    // shared verbatim with `loom complete`, so the two reads cannot drift.
    let inbox_items = db.list_inbox_items(None, None)?;
    let ladder_bundle = build_ladder(
        root,
        &snapshot,
        &gs,
        &decision_notes,
        &inbox_items,
        &open_smells,
        excellence_debt_count,
        intake.untriaged.max(0) as usize,
        export_freshness == "stale",
    );
    let source_corpus = ladder_bundle.source_corpus.clone();
    let ladder = ladder_bundle.ladder;
    let certification =
        certification_rollup(&ladder, &gs, &disk, export_freshness, excellence_debt_count);
    // Normative blind spot: coded intents not covered by any inspected GOVERNS
    // verdict — neither directly nor via an ancestor with covers_descendants.
    // Only alarms when rules exist — an empty normative plane is routed by the
    // compass, not alarmed here. Uses the SAME shared coverage predicate as
    // normative_coverage_from_snapshot and the unmeasured_intents smell, so the
    // alarm, the queue, and the smell can never disagree on what's covered.
    let unmeasured_intents = if snapshot.rules.is_empty() {
        0
    } else {
        use crate::db::queries::scoring::{covers_descendants_set, governs_covers_intent};
        use std::collections::{HashMap, HashSet};
        let considered: HashSet<(&str, &str)> = snapshot
            .governs
            .iter()
            .filter(|g| {
                matches!(
                    g.inspection_status.as_str(),
                    "passing" | "failing" | "independent" | "partial"
                )
            })
            .map(|g| (g.rule_id.as_str(), g.intent_id.as_str()))
            .collect();
        let covers_set = covers_descendants_set(&snapshot.governs);
        let parent_of: HashMap<&str, &str> = snapshot
            .hierarchy
            .iter()
            .map(|(p, c)| (c.as_str(), p.as_str()))
            .collect();
        snapshot
            .intents
            .iter()
            .filter(|i| {
                if i.status == "deprecated" || !snapshot.with_code.contains(&i.id) {
                    return false;
                }
                // An intent is measured if ANY rule covers it (directly or via
                // a covers_descendants ancestor).
                !snapshot.rules.iter().any(|r| {
                    governs_covers_intent(
                        r.id.as_str(),
                        i.id.as_str(),
                        &considered,
                        &covers_set,
                        &parent_of,
                    )
                })
            })
            .count() as i64
    };

    render_status(
        &report,
        &gs,
        &lanes,
        optional_lanes,
        &populate_pulse,
        &outside,
        &blocked,
        intake,
        open_todos,
        advisories,
        audit,
        align_count,
        adopt_count,
        export_freshness,
        &disk,
        &ladder,
        &certification,
        &source_corpus,
        unmeasured_intents,
        printer,
    )
}

fn rung_status(ladder: &MaturityLadder, name: &str) -> Option<crate::db::queries::RungStatus> {
    ladder
        .rungs
        .iter()
        .find(|r| r.name == name)
        .map(|r| r.status)
}

fn status_word(ok: bool) -> String {
    if ok { "green" } else { "red" }.to_string()
}

fn certification_rollup(
    ladder: &MaturityLadder,
    gs: &GraphState,
    disk: &DiskPulse,
    export_freshness: &str,
    excellence_debt: usize,
) -> CertificationRollup {
    let seeded_ok = rung_status(ladder, "Seeded").is_some_and(|s| s.cleared());
    let realized_ok = rung_status(ladder, "Realized").is_some_and(|s| s.cleared());
    let proven_ok = rung_status(ladder, "Proven").is_some_and(|s| s.cleared());
    let hardened_ok = rung_status(ladder, "Hardened").is_some_and(|s| s.cleared());
    let production_ok = rung_status(ladder, "Production-ready").is_some_and(|s| s.cleared());
    let excellent_ok = rung_status(ladder, "Excellent").is_some_and(|s| s.cleared());

    let map_ok = seeded_ok
        && gs.vertically_complete
        && gs.horizontally_explored
        && disk.total == 0
        && export_freshness != "stale";
    let behavior_ok = realized_ok && proven_ok;
    let quality_ok = hardened_ok;
    let excellence = if excellent_ok {
        "green"
    } else if production_ok {
        "yellow"
    } else {
        "red"
    }
    .to_string();
    let note = if excellent_ok {
        "Excellent: the map is trustworthy, production fitness is certified, and no unresolved excellence debt remains.".to_string()
    } else if production_ok {
        format!(
            "Production-ready, but not Excellent: {excellence_debt} unresolved excellence debt item(s) remain. The map may be green while codebase excellence is still yellow."
        )
    } else {
        "Not production-ready yet; climb the focus rung before pursuing excellence debt."
            .to_string()
    };

    CertificationRollup {
        overall: excellence.clone(),
        default_profile: "excellent".to_string(),
        map_integrity: status_word(map_ok),
        behavior: status_word(behavior_ok),
        quality: status_word(quality_ok),
        production: status_word(production_ok),
        excellence,
        excellence_debt,
        note,
    }
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
/// horizontal risk/survey plane, already signalled by the horizontal line) and the human-gated
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

fn role_for_lane(lane: &str) -> &'static str {
    match lane {
        "build" | "populate" | "export" | "wiki" | "refactor" => "builder",
        "discovery" | "prove" | "review" => "analyzer",
        "fix" => "fixer",
        "validate" | "align" => "validator",
        "quality" => "quality",
        _ => "analyzer",
    }
}

fn first_backticked_command(text: &str) -> Option<String> {
    let start = text.find('`')?;
    let rest = &text[start + 1..];
    let end = rest.find('`')?;
    Some(rest[..end].to_string())
}

fn one_turn_plan(gs: &GraphState, ladder: &MaturityLadder) -> OneTurnPlan {
    let (focus_rung, lane) = match ladder.focus_rung() {
        Some(rung) => (
            rung.name.to_string(),
            rung.lane.unwrap_or(gs.phase.as_str()).to_string(),
        ),
        None => ("Production-ready".to_string(), gs.phase.clone()),
    };
    let role = role_for_lane(&lane).to_string();
    let next_command = first_backticked_command(&gs.next_action).unwrap_or_else(|| {
        if lane == "export" {
            "loom export".to_string()
        } else {
            format!("loom next --mode {lane}")
        }
    });
    OneTurnPlan {
        focus_rung,
        focus_lane: lane,
        agent_export: ["export LOOM_AGENT=llm:", role.as_str()].concat(),
        guide_command: format!("loom guide --role {role}"),
        agent_role: role,
        next_command,
        rule: "ALARMS above preempt this plan. Otherwise ignore other debt counters this turn: run the next command, complete and record exactly one item, run `loom sync`/`loom export` when applicable, then rerun `loom status`.".to_string(),
    }
}

/// Autonomous lanes that don't gate the selected certification profile but MUST stay visible. The single
/// compass pointer names one lane; `other_lanes` covers the *required* closable
/// queues; `horizontal ○` flags optional discovery. Review (the tiered
/// double-check of low-confidence verdicts) and prove (hypotheses awaiting their
/// proof) had NO compass signal at all — so a status-driven driver was blind to
/// real autonomous work and could mistake it for human-gated or nonexistent.
/// This surfaces them honestly: autonomous, drainable now, not required for certification.
#[derive(Debug, Clone, Copy)]
struct OptionalLanes {
    review: i64,
    prove: i64,
}

impl OptionalLanes {
    fn any(self) -> bool {
        self.review > 0 || self.prove > 0
    }
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
        "horizontal_grid".to_string(),
        serde_json::json!({
            "unexplored_relationship_pairs": gs.unexplored_pairs,
            "priority_unexplored_relationship_pairs": gs.priority_unexplored_pairs,
            "required_for_complete": !gs.horizontally_explored,
            "horizontally_explored": gs.horizontally_explored,
            "survey_required_for_complete": false,
            "note_hygiene": gs.note_hygiene,
        }),
    );
    serde_json::Value::Object(completion)
}

/// The ALARM strip — the derived union of urgent signals that PREEMPT the focus
/// rung, surfaced at the TOP of `status` so a cold reader sees "what's on fire"
/// before "what to climb". Derived each call (never stored): failing edges/proofs,
/// open gating findings, disk drift, untriaged inbox — LLM-actionable ONLY
/// (human-gated work needs the user and stays on its own line, not here).
/// Empty ⇒ no strip. It SELECTS urgent signals from where they already live (their
/// lane); it never relocates them, so it cannot drift from the truth.
fn alarm_strip(
    report: &StatusReport,
    audit: &AuditPulse,
    disk: &DiskPulse,
    intake: IntakeCounts,
    export_freshness: &str,
    unmeasured_intents: i64,
    intents: i64,
) -> Vec<String> {
    let mut a = Vec::new();
    // Possible data loss FIRST: an empty live graph next to a committed
    // loom.graph.json means the durable graph wasn't loaded — the `.loom/`
    // SQLite store was deleted/lost (loom silently recreates it empty), or this
    // is a fresh checkout that never imported. Either way the committed export is
    // the recovery path; say so loudly instead of presenting empty-as-normal.
    if intents == 0 && export_freshness != "absent" {
        a.push(
            "live graph is EMPTY but a committed loom.graph.json exists — the durable graph isn't loaded (deleted `.loom/graph.sqlite`, or a fresh checkout). Restore it: `loom import loom.graph.json`".to_string(),
        );
    }
    if report.failing_edges > 0 {
        a.push(format!(
            "{} failing edge(s) — recovery depends on the edge family: re-run a failed proof after fixing the code (`loom validate <intent>`, or `loom saga run <saga>` for a saga boundary — note `loom validate --all` only runs NOT-yet-run proofs, not settled failures), fix failing relationships / needs_change (`loom next --mode fix`), or re-earn failing quality verdicts (`loom next --mode quality`). `loom next --all` lists each lane's outstanding work.",
            report.failing_edges
        ));
    }
    if unmeasured_intents > 0 {
        a.push(format!(
            "{unmeasured_intents} coded intent(s) not covered by any inspected GOVERNS — rules exist but never measured against this code (directly or via a --covers-descendants ancestor): `loom next --mode quality`"
        ));
    }
    let open = audit.open_findings.unwrap_or(0);
    if open > 0 {
        a.push(format!(
            "{open} open audit finding(s) — adjudicate or fix: `loom smells --summary`"
        ));
    }
    if disk.total > 0 {
        a.push(format!(
            "{} file(s) unmapped/drifted/missing (map ≠ territory) — `loom sync`, then `loom coverage`",
            disk.total
        ));
    }
    if disk.drifted > 0 {
        a.push(format!(
            "{} codefile(s) DRIFTED (content changed since sync) — source-fact reads (`loom smells`, `loom coverage`) are computed from stale facts and UNDER-REPORT the current code until `loom sync`",
            disk.drifted
        ));
    }
    if intake.untriaged > 0 {
        a.push(format!(
            "{} untriaged inbox item(s) — `loom inbox`",
            intake.untriaged
        ));
    }
    if export_freshness == "stale" {
        a.push(
            "committed loom.graph.json is STALE — `loom export` before committing code (`loom export --check` for CI)"
                .to_string(),
        );
    }
    a
}

#[allow(clippy::too_many_arguments)]
fn render_status(
    report: &StatusReport,
    gs: &GraphState,
    lanes: &LaneDepths,
    optional: OptionalLanes,
    populate: &PopulatePulse,
    outside: &UninspectedOutsideQueues,
    blocked: &BlockedValidationSummary,
    intake: IntakeCounts,
    open_todos: i64,
    advisories: AdvisoryCounts,
    audit: AuditPulse,
    align_count: i64,
    adopt_count: i64,
    export_freshness: &str,
    disk: &DiskPulse,
    ladder: &MaturityLadder,
    certification: &CertificationRollup,
    source_corpus: &SourceCorpusCoverage,
    unmeasured_intents: i64,
    printer: &Printer,
) -> Result<()> {
    let totals = completion_totals(lanes, populate, align_count, adopt_count, blocked);
    let human_gated = totals.required_human;

    if printer.json {
        let mut v = serde_json::to_value(report)?;
        if let Some(obj) = v.as_object_mut() {
            obj.insert("graph_state".to_string(), serde_json::to_value(gs)?);
            obj.insert("other_lanes".to_string(), other_lanes_json(lanes, populate));
            // Optional autonomous lanes (review/prove): visible, never required,
            // never human-gated — so an orchestrator routes an agent, not a person.
            obj.insert(
                "optional_autonomous".to_string(),
                serde_json::json!({
                    "review": optional.review,
                    "prove": optional.prove,
                    "gate": "autonomous",
                    "required_for_certification": false,
                    "note": "Drainable now by an agent (reviewer re-checks uncertain or high-risk verdicts; prove tests hypotheses) — not human-gated, not required for the selected certification profile.",
                }),
            );
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
            obj.insert("open_todos".to_string(), serde_json::json!(open_todos));
            obj.insert(
                "uninspected_outside_queues".to_string(),
                serde_json::to_value(outside)?,
            );
            obj.insert("advisories".to_string(), serde_json::to_value(advisories)?);
            obj.insert("audit".to_string(), serde_json::to_value(&audit)?);
            // honesty-next #2: always-on map-vs-territory (see DiskPulse).
            obj.insert("map_vs_territory".to_string(), serde_json::to_value(disk)?);
            obj.insert(
                "committed_export".to_string(),
                serde_json::json!(export_freshness),
            );
            // The maturity ladder — loom's certification vector (rung-vector).
            obj.insert("maturity".to_string(), serde_json::to_value(ladder)?);
            obj.insert(
                "certification".to_string(),
                serde_json::to_value(certification)?,
            );
            obj.insert(
                "one_turn".to_string(),
                serde_json::to_value(one_turn_plan(gs, ladder))?,
            );
            obj.insert(
                "source_corpus".to_string(),
                serde_json::to_value(source_corpus)?,
            );
            obj.insert(
                "alarms".to_string(),
                serde_json::json!(alarm_strip(
                    report,
                    &audit,
                    disk,
                    intake,
                    export_freshness,
                    unmeasured_intents,
                    gs.intents
                )),
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
        }
        printer.print_json(&v);
    } else {
        render_plain_status(
            report,
            gs,
            lanes,
            optional,
            populate,
            outside,
            blocked,
            intake,
            open_todos,
            advisories,
            &audit,
            align_count,
            adopt_count,
            export_freshness,
            disk,
            ladder,
            certification,
            source_corpus,
            unmeasured_intents,
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
    optional: OptionalLanes,
    populate: &PopulatePulse,
    outside: &UninspectedOutsideQueues,
    blocked: &BlockedValidationSummary,
    intake: IntakeCounts,
    open_todos: i64,
    advisories: AdvisoryCounts,
    audit: &AuditPulse,
    align_count: i64,
    adopt_count: i64,
    export_freshness: &str,
    disk: &DiskPulse,
    ladder: &MaturityLadder,
    certification: &CertificationRollup,
    source_corpus: &SourceCorpusCoverage,
    unmeasured_intents: i64,
    totals: CompletionTotals,
) {
    // The alarm strip — urgent signals that preempt the focus rung (cold readers
    let alarms = alarm_strip(
        report,
        audit,
        disk,
        intake,
        export_freshness,
        unmeasured_intents,
        gs.intents,
    );
    if !alarms.is_empty() {
        println!("⚠ ALARMS — handle these before the focus rung:");
        for line in &alarms {
            println!("   · {line}");
        }
        println!();
    }
    println!(
        "certification: overall {} (profile: {}) — {}",
        certification.overall, certification.default_profile, certification.note
    );
    println!(
        "  axes: map {} · behavior {} · quality {} · production {} · excellence {} (debt {})",
        certification.map_integrity,
        certification.behavior,
        certification.quality,
        certification.production,
        certification.excellence,
        certification.excellence_debt
    );

    // The maturity ladder — the FRAME: the map (all rungs), the
    // cursor (focus rung), the directive. A cold reader gets "where am I → what
    // to do" before the supporting detail below. Shown ALWAYS (no phase gating).
    println!("ladder: {}", ladder.vector_line());
    println!("  → {}", ladder.focus_summary());
    // The verb signals the compass's confidence: a directive phase (a failure or
    // binding gap) reads as a command; a recommended phase reads as a suggestion
    // the agent may override with context loom lacks.
    let anchor = if gs.next_kind == "recommended" {
        "→ Recommended"
    } else {
        "→ Next"
    };
    println!("  {anchor}: {}", gs.next_action);
    let turn = one_turn_plan(gs, ladder);
    println!("  one turn:");
    println!("    1. {}", turn.agent_export);
    println!("    2. {}", turn.guide_command);
    println!("    3. {}", turn.next_command);
    println!("    rule: {}", turn.rule);
    // JIT: status is the index; the depth (this rung's queue + skill) loads on
    // demand, so the frame stays constant and the prompt never becomes a firehose.
    println!("  → how: `loom guide` (this rung's skill — bare `guide` is focus-scoped, JIT)");
    if source_corpus.has_signal() {
        if source_corpus.ids_total > 0 {
            println!(
                "  source corpus: {} structured doc ID(s), {} modeled, {} resolved, {} unresolved",
                source_corpus.ids_total,
                source_corpus.modeled,
                source_corpus.resolved,
                source_corpus.unresolved
            );
        } else {
            println!(
                "  source corpus: {} doc file(s), no structured IDs detected — corpus completeness unknown; use `loom seed --inbox` for LLM triage",
                source_corpus.doc_files
            );
        }
        if !source_corpus.warning.is_empty() {
            println!("  ⚑ {}", source_corpus.warning);
        }
    }
    println!();
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
        "  horizontal risk: {} priority unexplored pair(s), current={} — REQUIRED for the HARDENED rung. Full survey remaining: {} optional pair(s) via `loom edge unexplored --class all`",
        gs.priority_unexplored_pairs,
        gs.horizontally_explored,
        (gs.unexplored_pairs - gs.priority_unexplored_pairs).max(0)
    );
    if intake.active() > 0 || intake.deferred > 0 {
        println!(
            "  inbox intake: {} untriaged · {} triaged · {} deferred (candidates, not graph truth)",
            intake.untriaged, intake.triaged, intake.deferred
        );
    }
    if open_todos > 0 {
        println!(
            "  ⬚ {open_todos} open todo(s) — follow-ups loom is holding so you can't forget them; \
             `loom note list --kind todo`, close with `loom note resolve <id> --reason …` (advisory)"
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
    // honesty-next #2: always-on map-vs-territory — the compass must not hide
    // files-on-disk the graph ignores, whatever phase the graph is in.
    println!("  🗺 {}", disk.message);
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
    if optional.any() {
        let mut bits = Vec::new();
        if optional.review > 0 {
            bits.push(format!(
                "{} review (re-check uncertain/high-risk verdicts)",
                optional.review
            ));
        }
        if optional.prove > 0 {
            bits.push(format!(
                "{} prove (hypotheses awaiting proof)",
                optional.prove
            ));
        }
        println!(
            "  optional autonomous: {} — an AGENT drains these (not human-gated), drainable now, not required for the selected certification profile: `loom next --mode review`/`--mode prove`.",
            bits.join(" · ")
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
}
