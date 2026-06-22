use super::*;

// ---------------------------------------------------------------------------
// --all: the CLOSEOUT view — every role queue at once. One prioritized answer
// to "what's left?" instead of five `next` calls + status + doctor reconciled
// by hand. Read-only; each line carries the exact command that works it.
// ---------------------------------------------------------------------------

pub(super) fn run_all(
    store: &dyn GraphReadRepository,
    root: &std::path::Path,
    printer: &Printer,
) -> Result<()> {
    let snapshot = store.query_snapshot()?;
    let gs = store.graph_state(&snapshot)?;
    let doctor = store.doctor_report(&snapshot)?;
    let all_smells = if matches!(gs.phase.as_str(), "audit" | "complete") {
        Some(store.smell_report(&snapshot)?.open)
    } else {
        None
    };
    let prove = store.prove_candidates(&snapshot)?;
    let supported_hypotheses = store.list_hypotheses(Some("supported"))?;
    let align = store.align_candidates(&snapshot)?;
    let populate = crate::commands::populate::plan_with_repo(store, root)?;
    let inbox_items = store.list_inbox_items(None, None)?;
    let export_freshness = match store.committed_export_stale(root)? {
        Some(true) => "stale",
        Some(false) => "fresh",
        None => "absent",
    }
    .to_string();
    render_all(
        snapshot,
        gs,
        doctor,
        all_smells,
        prove,
        supported_hypotheses,
        align,
        populate,
        inbox_items,
        export_freshness,
        printer,
    )
}

fn inbox_counts(items: &[crate::types::InboxItem]) -> (i64, i64, i64) {
    let untriaged = items.iter().filter(|item| item.status == "new").count() as i64;
    let triaged = items.iter().filter(|item| item.status == "triaged").count() as i64;
    let deferred = items
        .iter()
        .filter(|item| item.status == "deferred")
        .count() as i64;
    (untriaged, triaged, deferred)
}

struct CloseoutQueues {
    queues: Vec<serde_json::Value>,
    human_gated: i64,
    blocked_validation_audit: i64,
    human_blocked_validations: i64,
    affected_proof_edges: i64,
}

#[allow(clippy::too_many_arguments)]
fn build_closeout_queues(
    snapshot: &QuerySnapshot,
    gs: &GraphState,
    vc: &crate::db::queries::VerticalCompleteness,
    populate: &crate::commands::populate::PopulatePlan,
    inbox_untriaged: i64,
    inbox_triaged: i64,
    prove: &[(Hypothesis, f64)],
    supported_hypotheses: Vec<Hypothesis>,
    align: &[AlignCandidate],
) -> CloseoutQueues {
    let build = build_candidates_from_snapshot(snapshot);
    let fix = scored_candidates_from_snapshot(snapshot, "fix");
    let active_ids: std::collections::HashSet<&str> = snapshot
        .intents
        .iter()
        .filter(|i| i.status != "deprecated")
        .map(|i| i.id.as_str())
        .collect();
    let discovery_uninspected = snapshot
        .relates
        .iter()
        .filter(|e| e.inspection_status == "uninspected")
        .filter(|e| {
            active_ids.contains(e.from_id.as_str()) && active_ids.contains(e.to_id.as_str())
        })
        .count() as i64;
    let validate = validate_candidates_from_snapshot(snapshot);
    let quality = quality_candidates_from_snapshot(snapshot);
    let blocked = blocked_validation_summary_from_snapshot(snapshot);
    let blocked_validation_audit = blocked.autonomous_validation_count();
    let human_blocked_validations = blocked.human_validation_count();
    // Queues in dependency order (the handoff order from `loom guide`), each
    // with its count + top item. Vertical gaps slot in as builder work; the
    // horizontal grid comes last, flagged optional.
    //
    // Every queue carries a GATE: `autonomous` (an agent drains it alone) or
    // `human` (the item needs the user — a meaning to re-affirm, a ruling to
    // make). The gate is what makes the interactive↔autonomous oscillation
    // plannable: drain autonomous queues now, BATCH human-gated items into one
    // agenda for the next conversation window instead of dribbling questions.
    let mut queues: Vec<serde_json::Value> = Vec::new();
    if populate.pending_count() > 0 {
        let p = &populate.interface_from_sagas;
        let gaps = &populate.interface_gaps;
        let top = if p.is_pending() {
            format!(
                "interface_from_sagas: {} stale saga call set(s), {} missing surface(s)",
                p.stale_call_sets, p.missing_surfaces
            )
        } else {
            format!(
                "interface_gaps: {} total ({} surface/no-calls, {} boundary/no-calls, {} calls/no-validates)",
                gaps.total(),
                gaps.surface_without_calls,
                gaps.boundary_intent_without_calls,
                gaps.call_without_validates
            )
        };
        queues.push(serde_json::json!({
            "queue": "populate", "role": "builder", "gate": "autonomous",
            "count": populate.pending_count(), "command": crate::commands::POPULATE_NEXT_COMMAND,
            "top": top,
        }));
    }
    if inbox_untriaged + inbox_triaged > 0 {
        queues.push(serde_json::json!({
            "queue": "inbox", "role": "builder", "gate": "autonomous", "optional": true,
            "count": inbox_untriaged + inbox_triaged,
            "command": crate::commands::INBOX_TRIAGE_COMMAND,
            "top": format!("{} untriaged, {} triaged intake card(s); candidates, not graph truth", inbox_untriaged, inbox_triaged),
        }));
    }
    if !build.is_empty() {
        let c = &build[0];
        queues.push(serde_json::json!({
            "queue": "build", "role": if c.intent.lifecycle == "needs_change" || c.intent.lifecycle == "to_be_removed" { "fixer" } else { "builder" },
            "gate": "autonomous",
            "count": build.len(), "command": "loom next --mode build",
            "top": format!("'{}' ({})", c.intent.name, c.intent.lifecycle),
        }));
    }
    if !fix.is_empty() {
        let (e, _) = &fix[0];
        // The fix queue mixes lanes: failing → fixer (code repair),
        // needs_reverification → analyzer (re-inspection). Report the split and
        // an honest role instead of a flat "fixer" over the mix.
        let failing = fix
            .iter()
            .filter(|(e, _)| e.inspection_status == "failing")
            .count();
        let needs_rev = fix.len() - failing;
        let role = match (failing > 0, needs_rev > 0) {
            (true, true) => "mixed",
            (true, false) => "fixer",
            _ => "analyzer",
        };
        queues.push(serde_json::json!({
            "queue": "fix", "role": role, "gate": "autonomous",
            "count": fix.len(), "failing": failing, "needs_reverification": needs_rev,
            "command": "loom next --mode fix",
            "top": format!("'{}' × '{}' [{}]", e.from_name, e.to_name, e.inspection_status),
        }));
    }
    let ground_gaps = vc.unrealized_leaves.len() + vc.unreached_codefiles.len();
    if ground_gaps > 0 {
        let top = vc
            .unrealized_leaves
            .first()
            .map(|n| format!("unrealized leaf intent '{n}'"))
            .or_else(|| {
                vc.unreached_codefiles
                    .first()
                    .map(|p| format!("unreached file {p}"))
            })
            .unwrap_or_default();
        queues.push(serde_json::json!({
            "queue": "ground", "role": "builder", "gate": "autonomous",
            "count": ground_gaps, "command": "loom report  (then `loom edge implement` / `loom edge hierarchy` / `loom ignore`)",
            "top": top,
        }));
    }
    if !validate.is_empty() {
        let c = &validate[0];
        queues.push(serde_json::json!({
            "queue": "validate", "role": "validator", "gate": "autonomous",
            "count": validate.len(), "command": "loom next --mode validate",
            "top": format!("'{}' — {}", c.intent.name, c.reason),
        }));
    }
    if !quality.is_empty() {
        let (g, _) = &quality[0];
        queues.push(serde_json::json!({
            "queue": "quality", "role": "quality", "gate": "autonomous",
            "count": quality.len(), "command": "loom next --mode quality",
            "top": format!("rule '{}' → '{}' [{}]", g.rule_name, g.intent_name, g.inspection_status),
        }));
    }
    push_review_and_human_queues(
        &mut queues,
        snapshot,
        gs,
        prove,
        supported_hypotheses,
        align,
        &blocked,
        blocked_validation_audit,
        human_blocked_validations,
        discovery_uninspected,
    );

    // The oscillation summary: how much of the remainder needs the user.
    let human_gated: i64 = queues
        .iter()
        .filter(|q| q["gate"].as_str() == Some("human"))
        .map(|q| q["count"].as_i64().unwrap_or(0))
        .sum();

    CloseoutQueues {
        queues,
        human_gated,
        blocked_validation_audit,
        human_blocked_validations,
        affected_proof_edges: blocked.affected_proof_edges,
    }
}

#[allow(clippy::too_many_arguments)]
fn push_review_and_human_queues(
    queues: &mut Vec<serde_json::Value>,
    snapshot: &QuerySnapshot,
    gs: &GraphState,
    prove: &[(Hypothesis, f64)],
    supported_hypotheses: Vec<Hypothesis>,
    align: &[AlignCandidate],
    blocked: &crate::db::queries::BlockedValidationSummary,
    blocked_validation_audit: i64,
    human_blocked_validations: i64,
    discovery_uninspected: i64,
) {
    let review = review_candidates_from_snapshot(snapshot);
    if !review.is_empty() {
        queues.push(serde_json::json!({
            "queue": "review", "role": "reviewer", "gate": "autonomous", "optional": true, "effort": "high",
            "count": review.len(), "command": "loom next --mode review",
            "top": "low-confidence verdicts × centrality — the tiered double-check",
        }));
    }
    if !prove.is_empty() {
        let (h, _) = &prove[0];
        queues.push(serde_json::json!({
            "queue": "prove", "role": "analyzer", "gate": "autonomous", "optional": true, "effort": "high",
            "count": prove.len(), "command": "loom next --mode prove",
            "top": if h.status == "supported" {
                format!("hypothesis '{}' — support went stale (target code changed)", h.name)
            } else {
                format!("hypothesis '{}' awaits its proof", h.name)
            },
        }));
    }
    // Supported hypotheses NOT back in the prove queue await the adopt/reject
    // ruling — a judgment call on scope, so it is human-gated: the agent
    // prepares the case, the user (or an explicitly entrusted builder) rules.
    let in_prove: std::collections::HashSet<&str> =
        prove.iter().map(|(h, _)| h.id.as_str()).collect();
    let adopt: Vec<_> = supported_hypotheses
        .into_iter()
        .filter(|h| !in_prove.contains(h.id.as_str()))
        .collect();
    if !adopt.is_empty() {
        queues.push(serde_json::json!({
            "queue": "adopt", "role": "builder", "gate": "human",
            "count": adopt.len(),
            "command": "loom hypothesis show <id>  → loom hypothesis adopt <id> --spawned <planned-intent>… | loom hypothesis reject <id> --reason …",
            "top": format!("hypothesis '{}' is supported — awaiting the adopt/reject ruling", adopt[0].name),
        }));
    }
    // The user↔intent drift queue: meanings to re-affirm WITH the user. The
    // graph cannot read heads — this queue is human-gated by definition.
    if !align.is_empty() {
        queues.push(serde_json::json!({
            "queue": "align", "role": "validator", "gate": "human", "optional": true,
            "gate_reason": "user_intent_confirmation",
            "count": align.len(), "command": "loom next --mode align --take 50",
            "top": format!("'{}' — re-affirm its meaning with the user", align[0].intent.name),
        }));
    }
    if blocked_validation_audit > 0 {
        queues.push(serde_json::json!({
            "queue": "blocked-validation-audit", "role": "fixer", "gate": "autonomous",
            "gate_reason": "missing_artifact_or_stale_blocker",
            "count": blocked_validation_audit,
            "command": "loom validation list --result blocked --limit 0  (audit missing artifacts/stale blockers; regenerate artifacts or reclassify honestly)",
            "top": "blocked validation(s) whose blocker looks locally fixable or stale",
        }));
    }
    if human_blocked_validations > 0 {
        queues.push(serde_json::json!({
            "queue": "blocked-validations", "role": "validator", "gate": "human",
            "gate_reason": "blocked_prerequisite",
            "count": human_blocked_validations,
            "affected_proof_edges": blocked.affected_proof_edges,
            "by_gate_reason": blocked.human_gate_reasons(),
            "command": "loom validation list --result blocked --limit 0  (review blocked reasons, then unblock by changing prerequisites or marking the proof)",
            "top": "blocked validation object(s) with recorded prerequisites; one may affect many proof edges",
        }));
    }
    let discovery_backlog = discovery_uninspected + gs.unexplored_pairs;
    if discovery_backlog > 0 {
        queues.push(serde_json::json!({
            "queue": "horizontal-grid", "role": "analyzer", "gate": "autonomous", "optional": false,
            "count": discovery_backlog, "command": "loom edge unexplored",
            "top": "horizontal N×N grid: not for the vertical spine, but REQUIRED for the HARDENED rung. `loom edge unexplored` lists every pair (with pre-filled commands); `loom next --mode discovery` serves the high-signal ones",
        }));
    }
}

#[allow(clippy::too_many_arguments)]
fn render_all(
    snapshot: QuerySnapshot,
    gs: GraphState,
    doctor: DoctorReport,
    all_smells: Option<Vec<Smell>>,
    prove: Vec<(Hypothesis, f64)>,
    supported_hypotheses: Vec<Hypothesis>,
    align: Vec<AlignCandidate>,
    populate: crate::commands::populate::PopulatePlan,
    inbox_items: Vec<crate::types::InboxItem>,
    export_freshness: String,
    printer: &Printer,
) -> Result<()> {
    let mut vc = vertical_completeness_from_snapshot(&snapshot);
    let smells_computed = all_smells.is_some();
    let all_smells = all_smells.unwrap_or_default();
    let smells_total = all_smells.len();
    let smells_top: Vec<_> = all_smells.into_iter().take(3).collect();
    let (inbox_untriaged, inbox_triaged, inbox_deferred) = inbox_counts(&inbox_items);
    let closeout = build_closeout_queues(
        &snapshot,
        &gs,
        &vc,
        &populate,
        inbox_untriaged,
        inbox_triaged,
        &prove,
        supported_hypotheses,
        &align,
    );
    let queues = closeout.queues;
    let human_gated = closeout.human_gated;
    let blocked_validation_audit = closeout.blocked_validation_audit;
    let human_blocked_validations = closeout.human_blocked_validations;
    let affected_proof_edges = closeout.affected_proof_edges;

    if printer.json {
        let unrealized_leaves_total = vc.unrealized_leaves.len();
        let unreached_codefiles_total = vc.unreached_codefiles.len();
        vc.unrealized_leaves.truncate(20);
        vc.unreached_codefiles.truncate(20);
        let required_autonomous: i64 = queues
            .iter()
            .filter(|q| {
                q["gate"].as_str() == Some("autonomous")
                    && !q.get("optional").and_then(|v| v.as_bool()).unwrap_or(false)
            })
            .map(|q| q["count"].as_i64().unwrap_or(0))
            .sum();
        let horizontal_grid: i64 = queues
            .iter()
            .filter(|q| q["queue"].as_str() == Some("horizontal-grid"))
            .map(|q| q["count"].as_i64().unwrap_or(0))
            .sum();
        let mut completion = serde_json::Map::new();
        completion.insert(
            "required_autonomous_debt".to_string(),
            serde_json::json!(required_autonomous),
        );
        completion.insert(
            crate::commands::REQUIRED_HUMAN_GATED_DEBT_KEY.to_string(),
            serde_json::json!(human_gated),
        );
        completion.insert(
            "horizontal_grid_required_for_complete".to_string(),
            serde_json::json!(horizontal_grid),
        );
        completion.insert(
            "blocked_validations".to_string(),
            serde_json::json!(human_blocked_validations),
        );
        completion.insert(
            "blocked_validation_audit".to_string(),
            serde_json::json!(blocked_validation_audit),
        );
        completion.insert(
            "affected_proof_edges".to_string(),
            serde_json::json!(affected_proof_edges),
        );
        printer.print_json(&serde_json::json!({
            "mode": "all",
            "doctor": { "healthy": doctor.healthy(), "issues": doctor.issues, "hints": doctor.hints },
            "committed_export": export_freshness,
            "queues": queues,
            "completion": completion,
            "intake": {
                "untriaged": inbox_untriaged,
                "triaged": inbox_triaged,
                "deferred": inbox_deferred,
            },
            "human_gated": human_gated,
            "human_gated_note": if human_gated > 0 {
                "These items need the user or external prerequisites. Drain autonomous queues now; batch true user decisions into ONE agenda."
            } else { "" },
            "vertical_gaps": {
                "unrealized_leaves": vc.unrealized_leaves,
                "unreached_codefiles": vc.unreached_codefiles,
                "unrealized_leaves_total": unrealized_leaves_total,
                "unreached_codefiles_total": unreached_codefiles_total,
            },
            "smells_total": smells_total,
            "smells_computed": smells_computed,
            "smells_note": if smells_computed {
                ""
            } else {
                "Audit scan deferred while another phase is active; run `loom smells --summary` for current findings."
            },
            "smells_top": smells_top.iter().map(|s| serde_json::json!({
                "kind": s.kind, "summary": s.summary, "remedy": s.remedy,
            })).collect::<Vec<_>>(),
            "next_step": gs.next_action,
            "graph_state": pulse_json(&gs),
        }));
        return Ok(());
    }

    println!("── Closeout — every lane, one list ─────────────────────────────────");
    println!();
    if !doctor.healthy() {
        println!(
            "  0. [integrity] {} issue(s) — fix these first: `loom doctor`",
            doctor.issues.len()
        );
    }
    if export_freshness == "stale" {
        println!("  {}", crate::commands::EXPORT_STALE_WARNING);
    }
    if queues.is_empty() && doctor.healthy() {
        println!("  ✓ Nothing left in any queue — every lane is clear.");
    }
    for (i, q) in queues.iter().enumerate() {
        let opt = if q.get("optional").is_some() {
            "  (optional)"
        } else {
            ""
        };
        let gate = if q["gate"].as_str() == Some("human") {
            "  ⚑ human-gated"
        } else {
            ""
        };
        println!(
            "  {}. [{:<9}] {:<9} {:>4} item(s)   → {}{}{}",
            i + 1,
            q["role"].as_str().unwrap_or(""),
            q["queue"].as_str().unwrap_or(""),
            q["count"].as_i64().unwrap_or(0),
            q["command"].as_str().unwrap_or(""),
            opt,
            gate,
        );
        println!("       top: {}", q["top"].as_str().unwrap_or(""));
    }
    if human_gated > 0 {
        println!();
        println!(
            "  ⚑ {human_gated} item(s) need the user. Drain the autonomous queues now; batch the"
        );
        println!("    human-gated ones into ONE agenda for the next conversation window.");
    }
    println!();
    if smells_computed && smells_total > 0 {
        println!(
            "  smells: {} finding(s), top: {} — `loom smells`",
            smells_total,
            smells_top.first().map(|s| s.summary.as_str()).unwrap_or("")
        );
    } else if !smells_computed {
        println!("  smells: deferred while another phase is active — `loom smells --summary`.");
    }
    if doctor.healthy() {
        println!(
            "  doctor: ✓ healthy{}",
            if doctor.hints.is_empty() {
                String::new()
            } else {
                format!("  ({} hint(s) — `loom doctor`)", doctor.hints.len())
            }
        );
    }
    println!();
    println!("  Start here → {}", gs.next_action);
    println!("  {}", fmt_pulse(&gs));
    Ok(())
}
