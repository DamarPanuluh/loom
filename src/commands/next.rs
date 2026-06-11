use anyhow::Result;

use crate::db::{ensure_initialized, GrafeoDb};
use crate::db::queries::{
    build_candidates, build_candidates_from_snapshot, check_graph, compute_smells_from, get_intent,
    graph_state, list_implements_for_intent, notes_for_target, parse_sync_cause, quality_candidates,
    quality_candidates_from_snapshot, review_candidates_from_snapshot, scored_candidates,
    scored_candidates_from_snapshot, unexplored_pairs_scored, validate_candidates,
    validate_candidates_from_snapshot, validations_for_intent, vertical_completeness,
    QuerySnapshot,
};
use crate::output::{fmt_edge_detail, fmt_intent, fmt_pulse, more_marker, Printer, SECTION_CAP};
use crate::types::{CodeFile, EdgeType, WorkItem};

pub fn run(mode: &str, all: bool, take: usize, printer: &Printer) -> Result<()> {
    if all {
        let cwd = crate::db::resolve_root()?;
        let db_file = ensure_initialized(&cwd)?;
        let db = GrafeoDb::open(&db_file)?;
        return run_all(&db, printer);
    }
    if !matches!(mode, "discovery" | "fix" | "build" | "validate" | "quality" | "review" | "triage") {
        anyhow::bail!(
            "Unknown mode '{}'. Valid values: discovery, fix, build, validate, quality, review, triage
\
             discovery = inspect relationships (analyzer) · fix = resolve failures/stale · \
             build = realize planned/needs_change intents (builder) · \
             validate = run/repair proofs (validator) · quality = earn GOVERNS green (quality) · \
             review = re-inspect LOW-CONFIDENCE verdicts (the tiered double-check; resolves by \
             re-recording with confidence ≥ 0.7 or overturning) · \
             triage = prove PROPOSED hypotheses (analyzer; the pre-decision plane — optional).",
            mode
        );
    }

    if take > 0 && !matches!(mode, "discovery" | "fix") {
        anyhow::bail!(
            "--take is a bulk read of the discovery/fix queues (the post-sync drain path). \
             The other modes already resolve one command per item — use `loom next --mode {mode}` \
             and `loom batch` for bulk rule verdicts."
        );
    }

    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    match mode {
        "build" => return run_build(&db, printer),
        "validate" => return run_validate(&db, printer),
        "quality" => return run_quality(&db, printer),
        "review" => return run_review(&db, printer),
        "triage" => return run_triage(&db, printer),
        _ => {}
    }

    let mut candidates = scored_candidates(&db, mode)?;

    // Discovery keeps going once every materialised edge is inspected: fall back
    // to intent pairs that have no edge yet, so the N×N grid gets explored.
    if candidates.is_empty() && mode == "discovery" {
        candidates = unexplored_pairs_scored(&db)?;
    }

    let gs = graph_state(&db)?;

    if candidates.is_empty() {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status":  "empty",
                "mode":    mode,
                "message": "No work items found for this mode.",
                "graph_state": gs,
            }));
        } else {
            match mode {
                "discovery" => println!("✓ No uninspected edges — nothing left to discover."),
                "fix"       => println!("✓ No failing or needs_reverification edges."),
                _           => println!("✓ No work items found."),
            }
            println!();
            println!("  {}", fmt_pulse(&gs));
            println!("  → Next: {}", gs.next_action);
        }
        return Ok(());
    }

    // The bulk-read path: N compact items, one shared anchor.
    if take > 0 {
        return run_take(&db, mode, &candidates, take, &gs, printer);
    }

    let (top_edge, score) = &candidates[0];

    // Fetch rich context for both intents
    let intent_a = get_intent(&db, &top_edge.from_id)?
        .ok_or_else(|| anyhow::anyhow!(
            "Intent '{}' not found in DB — graph inconsistency; run `loom doctor`.",
            top_edge.from_id
        ))?;
    let intent_b_opt = get_intent(&db, &top_edge.to_id)?;

    // Fetch code files related to intent_a (via IMPLEMENTS)
    let mut implements_a = list_implements_for_intent(&db, &top_edge.from_id)?;
    let implements_total = cap_section(&mut implements_a);
    let code_files: Vec<CodeFile> = implements_a
        .iter()
        .map(|imp| CodeFile {
            id:            imp.codefile_id.clone(),
            path:          imp.codefile_path.clone(),
            language:      String::new(), // path is the primary identifier
            last_modified: String::new(),
            imports:       Vec::new(),
            content_hash:  String::new(),
        })
        .collect();

    // Fetch validations for intent_a (via VALIDATES)
    let mut validations = validations_for_intent(&db, &top_edge.from_id)?;
    let validations_total = cap_section(&mut validations);

    // discovery surfaces analyzer work. The fix queue SPLITS by what the item
    // actually is: a stale (needs_reverification) claim is RE-INSPECTION — the
    // criterion already exists, nothing gets fixed unless the re-inspection
    // finds breakage — so it belongs to the analyzer at mid effort; only a
    // recorded failure is repair work for the fixer, at high effort.
    let (role, effort) = match (mode, top_edge.inspection_status.as_str()) {
        ("fix", "failing") => ("fixer", "high"),
        ("fix", _) => ("analyzer", "mid"),
        _ => ("analyzer", "mid"),
    };

    // Gather accumulated memory: notes on the edge (if it exists yet) and on
    // both intents, so prior reasoning travels with the work item.
    let mut notes = Vec::new();
    if !top_edge.id.is_empty() {
        notes.extend(notes_for_target(&db, &top_edge.id)?);
    }
    notes.extend(notes_for_target(&db, &top_edge.from_id)?);
    notes.extend(notes_for_target(&db, &top_edge.to_id)?);
    sort_notes_for_role(&mut notes, role);
    let notes_total = cap_notes(&mut notes, role);

    // Build suggested action string
    let suggested_action = build_suggested_action(top_edge, score);

    let item = WorkItem {
        edge_type:         EdgeType::RelatesTo.to_string(),
        edge_id:           top_edge.id.clone(),
        inspection_status: top_edge.inspection_status.clone(),
        criterion:         top_edge.criterion.clone(),
        evidence:          top_edge.evidence.clone(),
        priority_score:    *score,
        intent_a:          intent_a.clone(),
        intent_b:          intent_b_opt.clone(),
        code_files:        code_files.clone(),
        implements:        implements_a.clone(),
        validations:       validations.clone(),
        notes:             notes.clone(),
        suggested_action:  suggested_action.clone(),
    };

    if printer.json {
        let mut v = serde_json::to_value(&item)?;
        if let Some(obj) = v.as_object_mut() {
            obj.insert("graph_state".to_string(), serde_json::to_value(&gs)?);
            obj.insert("notes_total".to_string(), notes_total.into());
            obj.insert("implements_total".to_string(), implements_total.into());
            obj.insert("validations_total".to_string(), validations_total.into());
            add_dispatch(obj, role, effort);
        }
        printer.print_json(&v);
        return Ok(());
    }

    // ---- Human-readable output ----
    println!(
        "── Next Work Item  [mode={}  priority={:.2}] ─────────────────────────────",
        mode, score
    );
    println!();

    println!("── Intent A ────────────────────────────────────────────────────────");
    println!("{}", fmt_intent(&intent_a));
    println!();

    if let Some(ref intent_b) = intent_b_opt {
        println!("── Intent B ────────────────────────────────────────────────────────");
        println!("{}", fmt_intent(intent_b));
        println!();
    }

    println!("── Edge  [RELATES_TO] ──────────────────────────────────────────────");
    println!("{}", fmt_edge_detail(top_edge));
    println!();

    // Code files (with locator precision where grounded)
    if !item.implements.is_empty() {
        println!("── Related Code Files ──────────────────────────────────────────────");
        for imp in &item.implements {
            if imp.locator.is_empty() {
                println!("  {}", imp.codefile_path);
            } else {
                println!("  {}  @ {}", imp.codefile_path, imp.locator);
            }
        }
        if let Some(m) = more_marker(
            implements_total,
            item.implements.len(),
            &format!("loom intent show {}", top_edge.from_id),
        ) {
            println!("  {m}");
        }
        println!();
    }

    // Validations
    if !validations.is_empty() {
        println!("── Validations on Intent A ─────────────────────────────────────────");
        for v in &validations {
            let result_mark = match v.last_result.as_str() {
                "passed"  => "✓",
                "failed"  => "✗",
                "blocked" => "⊘",
                _         => "?",
            };
            println!(
                "  {} {}  [{}]  cmd: {}",
                result_mark, v.name, v.last_result, v.command
            );
        }
        if let Some(m) = more_marker(validations_total, validations.len(), "loom validation list") {
            println!("  {m}");
        }
        println!();
    }

    // Accumulated memory
    if !notes.is_empty() {
        if notes_total > notes.len() {
            println!("── Notes ({}, showing {}) ─────────────────────────────────────────", notes_total, notes.len());
        } else {
            println!("── Notes ({}) ──────────────────────────────────────────────────────", notes.len());
        }
        for n in &notes {
            println!("  [{}] {}  ({})", n.kind, n.text, n.author);
        }
        let fetch = if top_edge.id.is_empty() {
            format!("loom note list --intent {}", top_edge.from_id)
        } else {
            format!("loom note list --edge {}", top_edge.id)
        };
        if let Some(m) = more_marker(notes_total, notes.len(), &fetch) {
            println!("  {m}");
        }
        println!();
    }

    // Suggested action
    println!("── Suggested Action ────────────────────────────────────────────────");
    println!("{}", suggested_action);
    println!();
    println!("  Dispatch — {}  [effort: {effort}]", dispatch_line(role));
    println!("  {}", fmt_pulse(&gs));

    Ok(())
}

// ---------------------------------------------------------------------------
// `--take N` — the bulk-read half of the batch loop
//
// `loom batch` (bulk WRITE) exists because post-sync re-verification floods:
// touching a few central files stales dozens of claims. But the read side had
// the same flood: one rich item per call (intents + groundings + validations
// + notes + a full anchor each) and the same hot file re-read once per claim.
// A take is ONE call: compact items GROUPED BY THE FILE THAT STALED THEM
// (parsed from the sync transition notes), a prefilled `loom batch` template,
// and a single anchor — read each hot file once, verdict its whole group.
// ---------------------------------------------------------------------------

fn run_take(
    db: &GrafeoDb,
    mode: &str,
    candidates: &[(crate::types::RelatesTo, f64)],
    take: usize,
    gs: &crate::db::queries::GraphState,
    printer: &Printer,
) -> Result<()> {
    // BOUNDED: the take is itself a section — clamp hard so this call can't
    // recreate the flood it exists to prevent.
    const TAKE_CAP: usize = 50;
    let n = take.min(TAKE_CAP).min(candidates.len());
    let queue_total = candidates.len();

    // Group by staling file: the latest sync-flip note per edge names the
    // changed file; "" = no sync cause on record (fresh pairs, manual flips).
    let mut groups: Vec<(String, Vec<serde_json::Value>)> = Vec::new();
    let mut batch_lines: Vec<String> = Vec::new();
    for (edge, score) in candidates.iter().take(n) {
        let staled_by = if edge.id.is_empty() {
            String::new()
        } else {
            notes_for_target(db, &edge.id)?
                .iter()
                .rev() // newest last → scan backwards for the latest flip
                .find_map(|nt| parse_sync_cause(&nt.text).map(str::to_string))
                .unwrap_or_default()
        };
        // Re-inspection usually re-affirms the existing criterion — prefill
        // it; the agent rewrites the line to issue/independent on breakage.
        batch_lines.push(
            serde_json::json!({
                "op": "ground",
                "a": edge.from_id,
                "b": edge.to_id,
                "criterion": if edge.criterion.is_empty() { "<criterion>" } else { edge.criterion.as_str() },
                "confidence": 0.9,
            })
            .to_string(),
        );
        let item = serde_json::json!({
            "edge_id": edge.id,
            "a": { "id": edge.from_id, "name": edge.from_name },
            "b": { "id": edge.to_id, "name": edge.to_name },
            "inspection_status": edge.inspection_status,
            "criterion": edge.criterion,
            "priority_score": score,
            "staled_by": staled_by,
        });
        match groups.iter_mut().find(|(f, _)| *f == staled_by) {
            Some((_, items)) => items.push(item),
            None => groups.push((staled_by, vec![item])),
        }
    }
    // Biggest file-group first (one read pays for the most verdicts);
    // ungrouped ("") last.
    groups.sort_by(|a, b| {
        (a.0.is_empty(), std::cmp::Reverse(a.1.len()), &a.0)
            .cmp(&(b.0.is_empty(), std::cmp::Reverse(b.1.len()), &b.0))
    });

    let guidance = "Per group: read the staling file ONCE, decide each claim — keep `ground` if it still holds, rewrite to `issue` (+evidence) on breakage, `independent` if there is no relationship — then apply every verdict in ONE call: save the edited template and `loom batch <file>` (or pipe to `loom batch -`). Re-inspection is analyzer work at mid effort; recorded failures route to the fixer.";

    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "mode": mode,
            "taken": n,
            "queue_total": queue_total,
            "groups": groups
                .iter()
                .map(|(f, items)| serde_json::json!({ "staled_by": f, "items": items }))
                .collect::<Vec<_>>(),
            "batch_template": batch_lines,
            "guidance": guidance,
            "dispatch": { "role": "analyzer", "effort": "mid" },
            "graph_state": gs,
        }));
        return Ok(());
    }

    println!(
        "── Next: {n} of {queue_total}  [mode={mode}] — bulk read, grouped by staling file ────",
    );
    for (file, items) in &groups {
        println!();
        if file.is_empty() {
            println!("  (no sync cause on record)");
        } else {
            println!("  {} ({} claim(s) staled by this file)", file, items.len());
        }
        for it in items {
            println!(
                "    [{:<21} {:>5.2}]  {} × {}  ({})",
                it["inspection_status"].as_str().unwrap_or(""),
                it["priority_score"].as_f64().unwrap_or(0.0),
                it["a"]["name"].as_str().unwrap_or(""),
                it["b"]["name"].as_str().unwrap_or(""),
                if it["edge_id"].as_str().unwrap_or("").is_empty() { "no edge yet" } else { it["edge_id"].as_str().unwrap_or("") },
            );
        }
    }
    println!();
    println!("── Batch template (edit per finding, then `loom batch <file>`) ─────");
    for l in &batch_lines {
        println!("  {l}");
    }
    println!();
    println!("  {guidance}");
    println!();
    println!("  Dispatch — analyzer  [effort: mid]");
    println!("  {}", fmt_pulse(gs));
    Ok(())
}

// ---------------------------------------------------------------------------
// Build a one-line (or multi-line) action hint for the LLM
// ---------------------------------------------------------------------------

fn build_suggested_action(edge: &crate::types::RelatesTo, _score: &f64) -> String {
    match edge.inspection_status.as_str() {
        "unexplored" => format!(
            "No relationship is tracked yet between intent '{}' and intent '{}'.{} \
             Inspect whether they interact, then record the result (this creates the edge):\n\
             \n  loom edge explore {from} {to} ground --criterion \"<coexistence criterion>\" --confidence 0.9\
             \n  loom edge explore {from} {to} issue  --criterion \"<criterion>\" --evidence \"<problem>\"\
             \n  loom edge explore {from} {to} independent --notes \"<why unrelated>\"",
            edge.from_name, edge.to_name,
            if edge.notes.is_empty() { String::new() } else { format!(" ({})", edge.notes) },
            from = edge.from_id, to = edge.to_id,
        ),
        "uninspected" => format!(
            "Ground this edge — inspect whether intent '{}' and intent '{}' interact:\n\
             \n  loom edge explore {from} {to} ground --criterion \"<coexistence criterion>\" --confidence 0.9\
             \n  loom edge explore {from} {to} issue  --criterion \"<criterion>\" --evidence \"<problem>\"\
             \n  loom edge explore {from} {to} independent --notes \"<why unrelated>\"",
            edge.from_name, edge.to_name,
            from = edge.from_id, to = edge.to_id,
        ),
        "failing" => format!(
            "Fix the violation, then record it — IN THIS ORDER (sync before fix: \
             sync flips passing claims on changed files, so syncing after would \
             stale the green you just earned):\n\
             \n  1. Change the code so the criterion holds (minimal change, root cause).\
             \n  2. loom sync   (flags everything the change touched; this edge stays failing)\
             \n  3. loom edge fix {} --description \"<what you changed>\"",
            edge.id
        ),
        "needs_reverification" => format!(
            "Re-inspect this edge — a code change invalidated the previous assessment:\n\
             \n  loom edge explore {from} {to} ground --criterion \"<updated criterion>\"\
             \n  loom edge explore {from} {to} issue  --criterion \"<criterion>\" --evidence \"<finding>\"",
            from = edge.from_id, to = edge.to_id,
        ),
        other => format!("Review edge with inspection_status='{}' (id: {})", other, edge.id),
    }
}

// ---------------------------------------------------------------------------
// Role dispatch — name the lane that owns this item + the fields it fills, so an
// orchestrator can hand it to a role-scoped subagent straight from `loom next`.
// ---------------------------------------------------------------------------

/// The fields the owning role fills for a work item, keyed by role.
fn role_fills(role: &str) -> &'static str {
    match role {
        "analyzer"  => "criterion, evidence, confidence, inspection_status (the verdict)",
        "builder"   => "write code → `loom codefile add` → `loom edge implement` (locator) → mark implemented",
        "fixer"     => "the minimal change → `loom edge fix` / mark implemented",
        "validator" => "run the proof (or `loom validation mark`) → `loom intent confirm`",
        "quality"   => "the GOVERNS verdict — criterion, evidence, confidence (`loom rule verdict`)",
        _ => "its owned fields (see `loom schema`)",
    }
}

/// One-line dispatch hint: which role owns this item, how to run it as that role,
/// and what it fills. Used in both `--json` (as `owner_role`/`dispatch`) and human.
fn dispatch_line(role: &str) -> String {
    let lane = crate::gate::mode_for_role(role).unwrap_or("");
    format!(
        "this is {role} work — fills {fills}. Whoever takes it declares `LOOM_AGENT=llm:{role}` \
         (or stay bare `llm` for solo); its queue is `loom next --mode {lane}`. Same contract whether \
         that's you now, a later pass, or a parallel agent.",
        fills = role_fills(role),
    )
}

/// Inject `owner_role` + `effort` + `dispatch` into a work-item JSON object.
/// `effort` (low | mid | high) names how much capability THIS item's work
/// needs — a statement about the work, computed from structure; the harness
/// maps it to whatever models exist. Never a model name.
fn add_dispatch(obj: &mut serde_json::Map<String, serde_json::Value>, role: &str, effort: &str) {
    obj.insert("owner_role".to_string(), serde_json::json!(role));
    obj.insert("effort".to_string(), serde_json::json!(effort));
    obj.insert("dispatch".to_string(), serde_json::json!(dispatch_line(role)));
}

// ---------------------------------------------------------------------------
// --all: the CLOSEOUT view — every role queue at once. One prioritized answer
// to "what's left?" instead of five `next` calls + status + doctor reconciled
// by hand. Read-only; each line carries the exact command that works it.
// ---------------------------------------------------------------------------

fn run_all(db: &GrafeoDb, printer: &Printer) -> Result<()> {
    let gs = graph_state(db)?;
    let doctor = check_graph(db)?;
    let mut vc = vertical_completeness(db)?;
    let snapshot = QuerySnapshot::load(db)?;
    let build = build_candidates_from_snapshot(&snapshot);
    let fix = scored_candidates_from_snapshot(&snapshot, "fix");
    let discovery_uninspected =
        snapshot.relates.iter().filter(|e| e.inspection_status == "uninspected").count() as i64;
    let validate = validate_candidates_from_snapshot(&snapshot);
    let quality = quality_candidates_from_snapshot(&snapshot);
    let all_smells = compute_smells_from(db, &snapshot)?;
    let smells_total = all_smells.len();
    let smells_top: Vec<_> = all_smells.into_iter().take(3).collect();

    // Queues in dependency order (the handoff order from `loom guide`), each
    // with its count + top item. Vertical gaps slot in as builder work; the
    // horizontal grid comes last, flagged optional.
    let mut queues: Vec<serde_json::Value> = Vec::new();
    if !build.is_empty() {
        let c = &build[0];
        queues.push(serde_json::json!({
            "queue": "build", "role": if c.intent.lifecycle == "needs_change" { "fixer" } else { "builder" },
            "count": build.len(), "command": "loom next --mode build",
            "top": format!("'{}' ({})", c.intent.name, c.intent.lifecycle),
        }));
    }
    if !fix.is_empty() {
        let (e, _) = &fix[0];
        queues.push(serde_json::json!({
            "queue": "fix", "role": "fixer",
            "count": fix.len(), "command": "loom next --mode fix",
            "top": format!("'{}' × '{}' [{}]", e.from_name, e.to_name, e.inspection_status),
        }));
    }
    let ground_gaps = vc.unrealized_leaves.len() + vc.unreached_codefiles.len();
    if ground_gaps > 0 {
        let top = vc.unrealized_leaves.first()
            .map(|n| format!("unrealized leaf intent '{n}'"))
            .or_else(|| vc.unreached_codefiles.first().map(|p| format!("unreached file {p}")))
            .unwrap_or_default();
        queues.push(serde_json::json!({
            "queue": "ground", "role": "builder",
            "count": ground_gaps, "command": "loom report  (then `loom edge implement` / `loom edge hierarchy` / `loom ignore`)",
            "top": top,
        }));
    }
    if !validate.is_empty() {
        let c = &validate[0];
        queues.push(serde_json::json!({
            "queue": "validate", "role": "validator",
            "count": validate.len(), "command": "loom next --mode validate",
            "top": format!("'{}' — {}", c.intent.name, c.reason),
        }));
    }
    if !quality.is_empty() {
        let (g, _) = &quality[0];
        queues.push(serde_json::json!({
            "queue": "quality", "role": "quality",
            "count": quality.len(), "command": "loom next --mode quality",
            "top": format!("rule '{}' → '{}' [{}]", g.rule_name, g.intent_name, g.inspection_status),
        }));
    }
    let review = review_candidates_from_snapshot(&snapshot);
    if !review.is_empty() {
        queues.push(serde_json::json!({
            "queue": "review", "role": "reviewer", "optional": true, "effort": "high",
            "count": review.len(), "command": "loom next --mode review",
            "top": "low-confidence verdicts × centrality — the tiered double-check",
        }));
    }
    let triage = crate::db::queries::triage_candidates(db)?;
    if !triage.is_empty() {
        let (h, _) = &triage[0];
        queues.push(serde_json::json!({
            "queue": "triage", "role": "analyzer", "optional": true, "effort": "high",
            "count": triage.len(), "command": "loom next --mode triage",
            "top": if h.status == "supported" {
                format!("hypothesis '{}' — support went stale (target code changed)", h.name)
            } else {
                format!("hypothesis '{}' awaits its proof", h.name)
            },
        }));
    }
    let discovery_backlog = discovery_uninspected + gs.unexplored_pairs;
    if discovery_backlog > 0 {
        queues.push(serde_json::json!({
            "queue": "discovery", "role": "analyzer", "optional": true,
            "count": discovery_backlog, "command": "loom next",
            "top": "the horizontal N×N grid — understanding/cleanup, not required for done",
        }));
    }

    if printer.json {
        let unrealized_leaves_total = vc.unrealized_leaves.len();
        let unreached_codefiles_total = vc.unreached_codefiles.len();
        vc.unrealized_leaves.truncate(20);
        vc.unreached_codefiles.truncate(20);
        printer.print_json(&serde_json::json!({
            "mode": "all",
            "doctor": { "healthy": doctor.healthy(), "issues": doctor.issues, "hints": doctor.hints },
            "queues": queues,
            "vertical_gaps": {
                "unrealized_leaves": vc.unrealized_leaves,
                "unreached_codefiles": vc.unreached_codefiles,
                "unrealized_leaves_total": unrealized_leaves_total,
                "unreached_codefiles_total": unreached_codefiles_total,
            },
            "smells_total": smells_total,
            "smells_top": smells_top,
            "graph_state": gs,
        }));
        return Ok(());
    }

    println!("── Closeout — every lane, one list ─────────────────────────────────");
    println!();
    if !doctor.healthy() {
        println!("  0. [integrity] {} issue(s) — fix these first: `loom doctor`", doctor.issues.len());
    }
    if queues.is_empty() && doctor.healthy() {
        println!("  ✓ Nothing left in any queue — every lane is clear.");
    }
    for (i, q) in queues.iter().enumerate() {
        let opt = if q.get("optional").is_some() { "  (optional)" } else { "" };
        println!(
            "  {}. [{:<9}] {:<9} {:>4} item(s)   → {}{}",
            i + 1,
            q["role"].as_str().unwrap_or(""),
            q["queue"].as_str().unwrap_or(""),
            q["count"].as_i64().unwrap_or(0),
            q["command"].as_str().unwrap_or(""),
            opt,
        );
        println!("       top: {}", q["top"].as_str().unwrap_or(""));
    }
    println!();
    if smells_total > 0 {
        println!("  smells: {} finding(s), top: {} — `loom smells`", smells_total,
            smells_top.first().map(|s| s.summary.as_str()).unwrap_or(""));
    }
    if doctor.healthy() {
        println!("  doctor: ✓ healthy{}", if doctor.hints.is_empty() { String::new() }
            else { format!("  ({} hint(s) — `loom doctor`)", doctor.hints.len()) });
    }
    println!();
    println!("  Start here → {}", gs.next_action);
    println!("  {}", fmt_pulse(&gs));
    Ok(())
}

// ---------------------------------------------------------------------------
// Build mode: realize `planned` / `needs_change` intents (greenfield/refactor)
// ---------------------------------------------------------------------------

fn run_build(db: &GrafeoDb, printer: &Printer) -> Result<()> {
    crate::db::queries::ensure_owned(
        db, "work the build queue (there is nothing to build in someone else's repo)",
    )?;
    let candidates = build_candidates(db)?;
    let gs = graph_state(db)?;

    if candidates.is_empty() {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "empty", "mode": "build",
                "message": "No planned or needs_change intents — nothing to build.",
                "graph_state": gs,
            }));
        } else {
            println!("✓ No planned/needs_change intents — nothing to build.");
            println!();
            println!("  {}", fmt_pulse(&gs));
            println!("  → Next: {}", gs.next_action);
        }
        return Ok(());
    }

    let c = &candidates[0];
    let (intent, score) = (&c.intent, &c.score);
    let mut implements = list_implements_for_intent(db, &intent.id)?;
    let implements_total = cap_section(&mut implements);
    let mut validations = validations_for_intent(db, &intent.id)?;
    let validations_total = cap_section(&mut validations);
    // planned → builder constructs it; needs_change → fixer changes it.
    // Effort names the work: writing NEW code from a criterion (planned leaf)
    // and repairing existing code (needs_change) are high; verifying a
    // roll-up is mid.
    let (role, effort) = if intent.lifecycle == "needs_change" {
        ("fixer", "high")
    } else if c.rollup {
        ("builder", "mid")
    } else {
        ("builder", "high")
    };
    let mut notes = notes_for_target(db, &intent.id)?;
    sort_notes_for_role(&mut notes, role);
    let notes_total = cap_notes(&mut notes, role);
    let action = build_action(intent, c.rollup);

    let item = WorkItem {
        edge_type:         "BUILD".to_string(),
        edge_id:           String::new(),
        inspection_status: intent.lifecycle.clone(),
        criterion:         String::new(),
        evidence:          String::new(),
        priority_score:    *score,
        intent_a:          intent.clone(),
        intent_b:          None,
        code_files:        Vec::new(),
        implements:        implements.clone(),
        validations:       validations.clone(),
        notes:             notes.clone(),
        suggested_action:  action.clone(),
    };

    if printer.json {
        let mut v = serde_json::to_value(&item)?;
        if let Some(obj) = v.as_object_mut() {
            obj.insert("graph_state".to_string(), serde_json::to_value(&gs)?);
            obj.insert("notes_total".to_string(), notes_total.into());
            obj.insert("implements_total".to_string(), implements_total.into());
            obj.insert("validations_total".to_string(), validations_total.into());
            add_dispatch(obj, role, effort);
        }
        printer.print_json(&v);
        return Ok(());
    }

    println!(
        "── Next Build Item  [{}  priority={:.2}] ───────────────────────────────",
        intent.lifecycle, score
    );
    println!();
    println!("── Intent ──────────────────────────────────────────────────────────");
    println!("{}", fmt_intent(intent));
    println!();
    if !implements.is_empty() {
        println!("── Currently grounded at ───────────────────────────────────────────");
        for im in &implements {
            let loc = if im.locator.is_empty() { String::new() } else { format!("  @ {}", im.locator) };
            println!("  {}{}", im.codefile_path, loc);
        }
        if let Some(m) = more_marker(
            implements_total,
            implements.len(),
            &format!("loom intent show {}", intent.id),
        ) {
            println!("  {m}");
        }
        println!();
    }
    if !notes.is_empty() {
        if notes_total > notes.len() {
            println!("── Notes ({}, showing {}) ─────────────────────────────────────────", notes_total, notes.len());
        } else {
            println!("── Notes ({}) ──────────────────────────────────────────────────────", notes.len());
        }
        for n in &notes {
            println!("  [{}] {}  ({})", n.kind, n.text, n.author);
        }
        if let Some(m) = more_marker(
            notes_total,
            notes.len(),
            &format!("loom note list --intent {}", intent.id),
        ) {
            println!("  {m}");
        }
        println!();
    }
    println!("── Suggested Action ────────────────────────────────────────────────");
    println!("{}", action);
    println!();
    println!("  Dispatch — {}", dispatch_line(role));
    println!("  {}", fmt_pulse(&gs));
    Ok(())
}

// ---------------------------------------------------------------------------
// Validate mode: the validator's queue — intents with failing/unrun/missing proof
// ---------------------------------------------------------------------------

fn run_validate(db: &GrafeoDb, printer: &Printer) -> Result<()> {
    let candidates = validate_candidates(db)?;
    let gs = graph_state(db)?;

    if candidates.is_empty() {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "empty", "mode": "validate",
                "message": "Every intent's proof is green — nothing to validate.",
                "graph_state": gs,
            }));
        } else {
            println!("✓ Every intent's proof is green — nothing to validate.");
            println!();
            println!("  {}", fmt_pulse(&gs));
            println!("  → Next: {}", gs.next_action);
        }
        return Ok(());
    }

    let c = &candidates[0];
    let mut validations = validations_for_intent(db, &c.intent.id)?;
    let mut notes = notes_for_target(db, &c.intent.id)?;
    sort_notes_for_role(&mut notes, "validator");
    let notes_total = cap_notes(&mut notes, "validator");
    let action = if validations.is_empty() {
        format!(
            "PROVE this intent — it has no validations:\n\
             1. Decide how it can be proven (test | assertion | benchmark | manual_check —\n     \
                or, if it's part of an endpoint-reachable journey, a consumer saga:\n     \
                `loom saga add <spec.yaml>` proves the whole chain by executing it).\n  \
             2. loom validation add --name \"…\" --type test --command \"…\" --intent {id}\n  \
             3. loom validate {id}",
            id = c.intent.id,
        )
    } else {
        let saga_hint = validations
            .iter()
            .find(|v| v.validation_type == "saga")
            .map(|v| format!(
                "\nA saga proof is linked — run it directly for step-level output: loom saga run {}",
                v.name
            ))
            .unwrap_or_default();
        format!(
            "Run this intent's validations and record the verdicts:\n\
             \n  loom validate {id}\n\
             \nIf one fails, the intent is not fulfilled — flag it: \
             loom intent mark {id} --lifecycle needs_change --reason \"<validation failure>\"{saga_hint}",
            id = c.intent.id,
        )
    };
    let validations_total = cap_section(&mut validations);

    if printer.json {
        printer.print_json(&serde_json::json!({
            "mode":             "validate",
            "reason":           c.reason,
            "priority_score":   c.score,
            "intent":           c.intent,
            "validations":      validations,
            "validations_total": validations_total,
            "notes":            notes,
            "notes_total":      notes_total,
            "suggested_action": action,
            "owner_role":       "validator",
            "effort":           if validations.is_empty() { "mid" } else { "low" },
            "dispatch":         dispatch_line("validator"),
            "graph_state":      gs,
        }));
        return Ok(());
    }

    println!(
        "── Next Validation Item  [priority={:.2}] ──────────────────────────────",
        c.score
    );
    println!();
    println!("  why: {}", c.reason);
    println!();
    println!("── Intent ──────────────────────────────────────────────────────────");
    println!("{}", fmt_intent(&c.intent));
    println!();
    if !validations.is_empty() {
        println!("── Linked Validations ──────────────────────────────────────────────");
        for v in &validations {
            let mark = match v.last_result.as_str() {
                "passed" => "✓",
                "failed" => "✗",
                "blocked" => "⊘",
                _ => "?",
            };
            println!("  {} {}  [{}]  cmd: {}", mark, v.name, v.last_result, v.command);
        }
        if let Some(m) = more_marker(validations_total, validations.len(), "loom validation list") {
            println!("  {m}");
        }
        println!();
    }
    println!("── Suggested Action ────────────────────────────────────────────────");
    println!("{}", action);
    println!();
    println!("  Dispatch — {}  [effort: {}]", dispatch_line("validator"),
        if validations.is_empty() { "mid" } else { "low" });
    println!("  {}", fmt_pulse(&gs));
    Ok(())
}

// ---------------------------------------------------------------------------
// Quality mode: the quality agent's queue — GOVERNS edges whose green is unearned
// ---------------------------------------------------------------------------

fn run_quality(db: &GrafeoDb, printer: &Printer) -> Result<()> {
    let candidates = quality_candidates(db)?;
    let gs = graph_state(db)?;

    if candidates.is_empty() {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "empty", "mode": "quality",
                "message": "No uninspected, failing, or stale GOVERNS edges — the green gate holds.",
                "graph_state": gs,
            }));
        } else {
            println!("✓ No uninspected, failing, or stale GOVERNS edges — the green gate holds.");
            println!();
            println!("  {}", fmt_pulse(&gs));
            println!("  → Next: {}", gs.next_action);
        }
        return Ok(());
    }

    let (g, score) = &candidates[0];
    let intent = get_intent(db, &g.intent_id)?;
    let mut implements = list_implements_for_intent(db, &g.intent_id)?;
    let implements_total = cap_section(&mut implements);
    let mut notes = if g.id.is_empty() { Vec::new() } else { notes_for_target(db, &g.id)? };
    sort_notes_for_role(&mut notes, "quality");
    let notes_total = cap_notes(&mut notes, "quality");
    // Effort comes from the RULE: the pack author knows statically whether
    // holding this stick against code is a near-mechanical scan or deep
    // semantic reading. "" (unannotated/older rules) reads as mid.
    let rule_effort = crate::db::queries::list_rules(db)?
        .into_iter()
        .find(|r| r.id == g.rule_id)
        .map(|r| r.inspection_effort)
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| "mid".to_string());
    let action = if g.inspection_status == "unmeasured" {
        format!(
            "MEASURE: rule '{rule}' has never been held against intent '{name}' (no GOVERNS edge \
             here or on any ancestor). One command records the measurement — the verdict CREATES the edge:\n\
             \n  loom rule verdict {rid} {iid} --status passing --criterion \"<what compliance looks like>\" --evidence \"<what you found>\"\
             \n  loom rule verdict {rid} {iid} --status failing --criterion \"<criterion>\" --evidence \"<the violation>\"\
             \n  loom rule verdict {rid} {iid} --status independent --criterion \"<criterion>\" --evidence \"<why the rule has no surface here>\"\
             \nPrefer the highest HONEST altitude: a verdict on a component covers its descendants \
             (check `loom intent show {iid}` — if this rule reads the same for the whole subtree, \
             verdict the parent instead).",
            name = g.intent_name,
            rule = g.rule_name,
            rid = g.rule_id,
            iid = g.intent_id,
        )
    } else {
        format!(
            "Inspect intent '{name}' against rule '{rule}' and record the verdict:\n\
             \n  loom rule verdict {rid} {iid} --status passing --criterion \"<what compliance looks like>\" --evidence \"<what you found>\"\
             \n  loom rule verdict {rid} {iid} --status failing --criterion \"<criterion>\" --evidence \"<the violation>\"",
            name = g.intent_name,
            rule = g.rule_name,
            rid = g.rule_id,
            iid = g.intent_id,
        )
    };

    if printer.json {
        printer.print_json(&serde_json::json!({
            "mode":             "quality",
            "priority_score":   score,
            "governs":          g,
            "intent":           intent,
            "implements":       implements,
            "implements_total": implements_total,
            "notes":            notes,
            "notes_total":      notes_total,
            "suggested_action": action,
            "owner_role":       "quality",
            "effort":           rule_effort,
            "dispatch":         dispatch_line("quality"),
            "graph_state":      gs,
        }));
        return Ok(());
    }

    println!(
        "── Next Quality Item  [{}  priority={:.2}] ─────────────────────────────",
        g.inspection_status, score
    );
    println!();
    println!("  rule:   {}  → intent: {}", g.rule_name, g.intent_name);
    if !g.criterion.is_empty() {
        println!("  criterion: {}", g.criterion);
    }
    if !g.evidence.is_empty() {
        println!("  evidence:  {}", g.evidence);
    }
    if !g.notes.is_empty() {
        // For unmeasured items this carries the rule's detection_logic —
        // exactly what to look for during the inspection.
        println!("  {}", g.notes);
    }
    println!();
    if let Some(ref i) = intent {
        println!("── Intent ──────────────────────────────────────────────────────────");
        println!("{}", fmt_intent(i));
        println!();
    }
    if !implements.is_empty() {
        println!("── Grounded at ─────────────────────────────────────────────────────");
        for im in &implements {
            let loc = if im.locator.is_empty() { String::new() } else { format!("  @ {}", im.locator) };
            println!("  {}{}", im.codefile_path, loc);
        }
        if let Some(m) = more_marker(
            implements_total,
            implements.len(),
            &format!("loom intent show {}", g.intent_id),
        ) {
            println!("  {m}");
        }
        println!();
    }
    println!("── Suggested Action ────────────────────────────────────────────────");
    println!("{}", action);
    println!();
    println!("  Dispatch — {}  [effort: {rule_effort}]", dispatch_line("quality"));
    println!("  {}", fmt_pulse(&gs));
    Ok(())
}

fn build_action(intent: &crate::types::Intent, rollup: bool) -> String {
    match intent.lifecycle.as_str() {
        // A planned parent whose children are all implemented: the work is
        // verification + roll-up, never writing code at this altitude.
        "planned" if rollup => format!(
            "ROLL UP this intent — all its children are implemented; nothing is built \
             at this altitude directly.
\
             1. Check each child fulfils its criterion (`loom intent show {id}` lists children).
  \
             2. If satisfied: loom intent mark {id} --lifecycle implemented
  \
             3. If a child falls short: loom intent mark <child-id> --lifecycle needs_change --reason \"…\"",
            id = intent.id,
        ),
        "planned" => format!(
            "BUILD this intent — its description/criteria are the spec/acceptance check.
\
             1. Write the code.
  \
             2. Register it: loom codefile add <path>
  \
             3. Ground it: loom edge implement {id} <codefile> --locator \"<symbol>\"
  \
             4. Mark done: loom intent mark {id} --lifecycle implemented
  \
             5. Baseline it: loom sync   (stamps the new files; future edits ripple correctly)",
            id = intent.id,
        ),
        "needs_change" => format!(
            "CHANGE the code for this intent (the description/criteria + notes describe the desired end state).
\
             1. Make the minimal change.
  \
             2. Flag the ripple: loom sync   (stales every claim the change touched)
  \
             3. Mark done: loom intent mark {id} --lifecycle implemented
  \
             4. Re-verify what sync flagged: loom next --mode fix",
            id = intent.id,
        ),
        other => format!("Intent '{}' has lifecycle '{}' — review it.", intent.name, other),
    }
}

// ---------------------------------------------------------------------------
// Review mode: the strategic double-check for tiered agents — verdicts whose
// recorded confidence is below REVIEW_CONFIDENCE, highest (1−conf)×centrality
// first. A low-capability scout records honest uncertainty; the graph routes
// exactly those claims to a stronger reviewer. Resolves by RE-RECORDING the
// verdict (confirm with confidence ≥ 0.7, or overturn) via the normal write
// paths — no special review write exists, so every gate still applies.
// ---------------------------------------------------------------------------

fn run_review(db: &GrafeoDb, printer: &Printer) -> Result<()> {
    use crate::db::queries::{review_candidates, ReviewCandidate, REVIEW_CONFIDENCE};
    let candidates = review_candidates(db)?;
    let gs = graph_state(db)?;

    if candidates.is_empty() {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "empty", "mode": "review",
                "message": format!("No verdicts below confidence {REVIEW_CONFIDENCE} — nothing needs a second look."),
                "graph_state": gs,
            }));
        } else {
            println!("✓ No verdicts below confidence {REVIEW_CONFIDENCE} — nothing needs a second look.");
            println!();
            println!("  {}", fmt_pulse(&gs));
            println!("  → Next: {}", gs.next_action);
        }
        return Ok(());
    }

    let (item, score) = &candidates[0];
    let protocol = "INDEPENDENT RE-INSPECTION: form your own hypothesis from the intents and the \
code BEFORE reading the recorded evidence (anchoring on a low-confidence claim defeats the \
review). Then re-record: confirm with your own confidence (≥ 0.7 resolves this item) or overturn.";

    match item {
        ReviewCandidate::RelatesTo(e) => {
            let intent_a = get_intent(db, &e.from_id)?;
            let intent_b = get_intent(db, &e.to_id)?;
            let mut notes = notes_for_target(db, &e.id)?;
            sort_notes_for_role(&mut notes, "analyzer");
            let notes_total = cap_notes(&mut notes, "analyzer");
            let action = format!(
                "{protocol}

  loom edge explore {a} {b} ground --criterion \"…\" --confidence 0.9
  loom edge explore {a} {b} issue --criterion \"…\" --evidence \"…\"
  loom edge explore {a} {b} independent --notes \"…\"",
                a = e.from_id, b = e.to_id,
            );
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "mode": "review", "kind": "relates_to", "priority_score": score,
                    "edge": e, "intent_a": intent_a, "intent_b": intent_b, "notes": notes,
                    "notes_total": notes_total,
                    "suggested_action": action,
                    "owner_role": "analyzer", "effort": "high",
                    "dispatch": dispatch_line("analyzer"),
                    "graph_state": gs,
                }));
                return Ok(());
            }
            println!("── Next Review Item  [relates_to  confidence={:.2}  priority={:.2}] ──────", e.confidence, score);
            println!();
            println!("  {} × {}", e.from_name, e.to_name);
            println!("  recorded verdict: {}  (by {})", e.inspection_status, e.inspected_by);
            println!("  criterion: {}", e.criterion);
            println!();
            println!("── Suggested Action ────────────────────────────────────────────────");
            println!("{action}");
            println!();
            println!("  Dispatch — {}  [effort: high]", dispatch_line("analyzer"));
            println!("  {}", fmt_pulse(&gs));
        }
        ReviewCandidate::Governs(g) => {
            let intent = get_intent(db, &g.intent_id)?;
            let mut notes = notes_for_target(db, &g.id)?;
            sort_notes_for_role(&mut notes, "quality");
            let notes_total = cap_notes(&mut notes, "quality");
            let action = format!(
                "{protocol}

  loom rule verdict {r} {i} --status passing|failing|independent --criterion \"…\" --evidence \"…\" --confidence 0.9",
                r = g.rule_id, i = g.intent_id,
            );
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "mode": "review", "kind": "governs", "priority_score": score,
                    "governs": g, "intent": intent, "notes": notes,
                    "notes_total": notes_total,
                    "suggested_action": action,
                    "owner_role": "quality", "effort": "high",
                    "dispatch": dispatch_line("quality"),
                    "graph_state": gs,
                }));
                return Ok(());
            }
            println!("── Next Review Item  [governs  confidence={:.2}  priority={:.2}] ─────────", g.confidence, score);
            println!();
            println!("  rule {} → intent {}", g.rule_name, g.intent_name);
            println!("  recorded verdict: {}  (by {})", g.inspection_status, g.inspected_by);
            println!("  criterion: {}", g.criterion);
            println!();
            println!("── Suggested Action ────────────────────────────────────────────────");
            println!("{action}");
            println!();
            println!("  Dispatch — {}  [effort: high]", dispatch_line("quality"));
            println!("  {}", fmt_pulse(&gs));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Triage mode: the pre-decision plane's queue — proposed hypotheses awaiting
// their proof, highest target-centrality (blast radius) first. Analyzer work;
// optional like discovery/review (speculation never blocks complete).
// ---------------------------------------------------------------------------

fn run_triage(db: &GrafeoDb, printer: &Printer) -> Result<()> {
    let candidates = crate::db::queries::triage_candidates(db)?;
    let gs = graph_state(db)?;

    if candidates.is_empty() {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "empty", "mode": "triage",
                "message": "No proposed hypotheses and no stale support — the pre-decision plane is clear.",
                "graph_state": gs,
            }));
        } else {
            println!("✓ No proposed hypotheses and no stale support — the pre-decision plane is clear.");
            println!();
            println!("  {}", fmt_pulse(&gs));
            println!("  → Next: {}", gs.next_action);
        }
        return Ok(());
    }

    let (h, score) = &candidates[0];
    let mut targets = crate::db::queries::list_targets_for_hypothesis(db, &h.id)?;
    // The proof reads the targeted intents' code — surface their groundings.
    let mut implements = Vec::new();
    for t in &targets {
        implements.extend(list_implements_for_intent(db, &t.intent_id)?);
    }
    let mut notes = notes_for_target(db, &h.id)?;
    sort_notes_for_role(&mut notes, "analyzer");
    let notes_total = cap_notes(&mut notes, "analyzer");
    // Two item kinds share this queue: a never-proven proposal, and a
    // supported hypothesis whose TARGETS evidence went stale under it
    // (`loom sync` flipped them — the support was earned against old code).
    let stale_targets: Vec<&str> = targets
        .iter()
        .filter(|t| t.inspection_status == "needs_reverification")
        .map(|t| t.intent_name.as_str())
        .collect();
    let action = if h.status == "supported" {
        format!(
            "RE-PROVE this hypothesis — its support was earned against code that has since changed \
             (stale target(s): {stale}; the TARGETS transition notes name the files).\n\
             Re-check the claim against the code as it is NOW, then re-record:\n\
             \n  loom hypothesis prove {id} --verdict supported --evidence \"<what still holds>\"\
             \n  loom hypothesis prove {id} --verdict refuted  --evidence \"<the change resolved it>\"\
             \nRe-proving re-stamps every TARGETS edge, clearing the staleness.",
            id = h.id,
            stale = stale_targets.join(", "),
        )
    } else {
        format!(
            "PROVE this hypothesis — is the claimed problem real in the code as it is NOW?\n\
             Read the targeted intents' groundings, check the claim, record what you found:\n\
             \n  loom hypothesis prove {id} --verdict supported --evidence \"<what you found>\"\
             \n  loom hypothesis prove {id} --verdict refuted  --evidence \"<why the claim doesn't hold>\"\
             \nThe proposer was '{author}' — the prover must be someone else (when roles are declared). \
             A supported verdict hands the adopt/reject decision to the builder lane.",
            id = h.id,
            author = h.author,
        )
    };
    let targets_total = cap_section(&mut targets);
    let implements_total = cap_section(&mut implements);

    if printer.json {
        printer.print_json(&serde_json::json!({
            "mode":             "triage",
            "priority_score":   score,
            "hypothesis":       h,
            "targets":          targets,
            "implements":       implements,
            "notes":            notes,
            "targets_total":    targets_total,
            "implements_total": implements_total,
            "notes_total":      notes_total,
            "suggested_action": action,
            "owner_role":       "analyzer",
            "effort":           "high",
            "dispatch":         dispatch_line("analyzer"),
            "graph_state":      gs,
        }));
        return Ok(());
    }

    println!(
        "── Next Triage Item  [{}  priority={:.2}] ────────────────────────",
        if h.status == "supported" { "stale support" } else { "proposed" },
        score
    );
    println!();
    println!("  hypothesis:        {}  ({})", h.name, h.id);
    println!("  claim:             {}", h.claim);
    println!("  proposal:          {}", h.proposal);
    println!("  predicted_outcome: {}", h.predicted_outcome);
    println!("  proposed by:       {}", h.author);
    println!();
    if !targets.is_empty() {
        println!("── Targets ({}) ─────────────────────────────────────────────────────", targets_total);
        for t in &targets {
            println!("  → {}  ({})", t.intent_name, t.intent_id);
        }
        if let Some(m) = more_marker(
            targets_total,
            targets.len(),
            &format!("loom hypothesis show {}", h.id),
        ) {
            println!("  {m}");
        }
        println!();
    }
    if !implements.is_empty() {
        println!("── Targeted code ───────────────────────────────────────────────────");
        for im in &implements {
            let loc = if im.locator.is_empty() { String::new() } else { format!("  @ {}", im.locator) };
            println!("  {}{}", im.codefile_path, loc);
        }
        if let Some(m) = more_marker(
            implements_total,
            implements.len(),
            &format!("loom hypothesis show {}", h.id),
        ) {
            println!("  {m}");
        }
        println!();
    }
    if !notes.is_empty() {
        if notes_total > notes.len() {
            println!("── Notes ({}, showing {}) ─────────────────────────────────────────", notes_total, notes.len());
        } else {
            println!("── Notes ({}) ──────────────────────────────────────────────────────", notes.len());
        }
        for n in &notes {
            println!("  [{}] {}  ({})", n.kind, n.text, n.author);
        }
        if let Some(m) = more_marker(
            notes_total,
            notes.len(),
            &format!("loom hypothesis show {}", h.id),
        ) {
            println!("  {m}");
        }
        println!();
    }
    println!("── Suggested Action ────────────────────────────────────────────────");
    println!("{action}");
    println!();
    println!("  Dispatch — {}  [effort: high]", dispatch_line("analyzer"));
    println!("  {}", fmt_pulse(&gs));
    Ok(())
}

/// Addressed notes first: a note with `audience == role` is a directed handoff
/// message for whoever is working this item — surface it before the ambient
/// memory. Stable within groups (chronological order preserved).
fn sort_notes_for_role(notes: &mut [crate::types::Note], role: &str) {
    notes.sort_by_key(|n| if n.audience == role { 0 } else { 1 });
}

/// Bound a sub-list rendered inside a work item at SECTION_CAP.
/// Returns the pre-cap total for the caller's marker/`*_total` fields.
fn cap_section<T>(items: &mut Vec<T>) -> usize {
    let total = items.len();
    items.truncate(SECTION_CAP);
    total
}

/// Bound a role-sorted note list at SECTION_CAP. Addressed-to-role notes
/// (already first, via `sort_notes_for_role`) keep priority; remaining
/// slots go to the NEWEST ambient notes (the tail of the chronological
/// remainder). Returns the pre-cap total.
fn cap_notes(notes: &mut Vec<crate::types::Note>, role: &str) -> usize {
    let total = notes.len();
    if total <= SECTION_CAP {
        return total;
    }
    let addressed = notes.iter().take_while(|n| n.audience == role).count();
    if addressed >= SECTION_CAP {
        notes.truncate(SECTION_CAP);
    } else {
        notes.drain(addressed..total - (SECTION_CAP - addressed));
    }
    total
}
