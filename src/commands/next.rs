use anyhow::Result;

use crate::db::queries::{
    blocked_validation_summary_from_snapshot, build_candidates_from_snapshot, parse_sync_cause,
    quality_candidates_from_snapshot, review_candidates_from_snapshot,
    scored_candidates_from_snapshot, unexplored_pairs_scored_from_snapshot,
    validate_candidates_from_snapshot, vertical_completeness_from_snapshot, AlignCandidate,
    DiscoveryClassFilter, DoctorReport, GraphState, QuerySnapshot, Smell,
};
use crate::db::{GraphReadHandle, GraphReadRepository};
use crate::output::{
    fmt_edge_detail, fmt_intent_surface, fmt_pulse, more_marker, note_list_intent_command,
    pulse_json, Printer, SECTION_CAP,
};
use crate::types::{
    EdgeType, GroundingSurface, Hypothesis, IntentSurface, ValidationSurface, WorkItem,
};

const QUALITY_EMPTY_MESSAGE: &str =
    "No uninspected, failing, or stale GOVERNS edges — the green gate holds.";
const ALIGN_EMPTY_MESSAGE: &str =
    "No drift suspected — nothing churned under a fresh meaning, no wording past its re-affirmation grace. The interview is done.";
const BATCH_TEMPLATE_TITLE: &str =
    "── Batch template (edit per finding, then paste into `loom batch - <<'EOF' … EOF`) ──";

struct NextOpts<'a> {
    mode: &'a str,
    all: bool,
    take: usize,
    discovery_class: Option<&'a str>,
    compact: bool,
}

pub fn run(
    mode: &str,
    all: bool,
    take: usize,
    discovery_class: Option<&str>,
    compact: bool,
    printer: &Printer,
) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let store = GraphReadHandle::open(&cwd)?;
    run_with_repo(
        &store,
        &cwd,
        &NextOpts {
            mode,
            all,
            take,
            discovery_class,
            compact,
        },
        printer,
    )
}

fn run_with_repo(
    db: &dyn GraphReadRepository,
    root: &std::path::Path,
    opts: &NextOpts<'_>,
    printer: &Printer,
) -> Result<()> {
    let NextOpts {
        mode,
        all,
        take,
        discovery_class,
        compact,
    } = *opts;
    if all {
        return run_all(db, root, printer);
    }
    if mode == "triage" {
        anyhow::bail!(
            "Mode 'triage' was renamed to 'prove' (it proves proposed hypotheses; \
             'triage' now belongs to Inbox — `loom door \"<utterance>\"` captures \
             user input, then `loom inbox triage` routes it). Run: loom next --mode prove"
        );
    }
    if !matches!(
        mode,
        "discovery"
            | "fix"
            | "build"
            | "populate"
            | "validate"
            | "align"
            | "quality"
            | "review"
            | "prove"
    ) {
        anyhow::bail!(
            "Unknown mode '{}'. Valid values: discovery, fix, build, populate, validate, align, quality, review, prove
\
             discovery = inspect relationships (analyzer) · fix = resolve failures/stale · \
             build = realize planned/needs_change intents (builder) · \
             populate = backfill derived graph structure (builder) · \
             validate = run/repair proofs (validator) · \
             align = re-affirm intent meaning against the USER (validator; serves intents whose code churned since the user last confirmed their meaning — the user↔intent drift check) · \
             quality = earn GOVERNS green (quality) · review = re-inspect LOW-CONFIDENCE verdicts (the tiered double-check; resolves by \
             re-recording with confidence ≥ 0.7 or overturning) · \
             prove = prove PROPOSED hypotheses (analyzer; the pre-decision plane — optional).",
            mode
        );
    }

    if take > 0 && !matches!(mode, "discovery" | "fix" | "quality" | "align") {
        anyhow::bail!(
            "--take is a bulk read of the discovery/fix/quality queues (post-sync/post-rule batch reads) \
             and the align queue (a human-interview agenda). \
             The other modes resolve one command per item — use `loom next --mode {mode}`."
        );
    }

    if discovery_class.is_some() && mode != "discovery" {
        anyhow::bail!(
            "--class only applies to generated discovery pairs. Use it with \
             `loom next --mode discovery --class suspected-coupling|impact-map|all`."
        );
    }
    let discovery_class = DiscoveryClassFilter::parse(discovery_class)?;

    if compact && !matches!(mode, "discovery" | "fix") {
        anyhow::bail!(
            "--compact projects a RELATES_TO work item down to its verdict coordinates \
             (intents, edge id, grounded paths, the command) — it serves the discovery/fix \
             queues. The other modes' items are already mode-shaped — use `loom next --mode {mode}`."
        );
    }

    match mode {
        "build" => return run_build(db, printer),
        "populate" => return crate::commands::populate::render_next(db, root, printer),
        "validate" => return run_validate(db, printer),
        "align" => {
            return if take > 0 {
                run_take_align(db, take, printer)
            } else {
                run_align(db, printer)
            }
        }
        "quality" => {
            return if take > 0 {
                run_take_quality(db, take, printer)
            } else {
                run_quality(db, printer)
            }
        }
        "review" => return run_review(db, printer),
        "prove" => return run_prove(db, printer),
        _ => {}
    }

    run_relates_with_repo(db, mode, take, discovery_class, compact, printer)
}

fn run_relates_with_repo(
    store: &dyn GraphReadRepository,
    mode: &str,
    take: usize,
    discovery_class: DiscoveryClassFilter,
    compact: bool,
    printer: &Printer,
) -> Result<()> {
    let snapshot = store.query_snapshot()?;
    let mut candidates = scored_candidates_from_snapshot(&snapshot, mode);

    if candidates.is_empty() && mode == "discovery" {
        candidates = unexplored_pairs_scored_from_snapshot(&snapshot, discovery_class)?;
    }

    if candidates.is_empty() {
        let gs = store.graph_state(&snapshot)?;
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status":  "empty",
                "mode":    mode,
                "discovery_class": discovery_class.as_cli_value(),
                "message": "No work items found for this mode.",
                "next_step": gs.next_action,
                "graph_state": pulse_json(&gs),
            }));
        } else {
            match mode {
                "discovery" => println!("✓ No uninspected edges — nothing left to discover."),
                "fix" => println!("✓ No failing or needs_reverification edges."),
                _ => println!("✓ No work items found."),
            }
            println!();
            println!("  {}", fmt_pulse(&gs));
            println!("  → Next: {}", gs.next_action);
        }
        return Ok(());
    }

    if take > 0 {
        let gs = store.graph_state(&snapshot)?;
        return run_take(store, mode, &candidates, take, &gs, printer);
    }

    let (top_edge, score) = &candidates[0];

    if compact {
        return run_compact(store, mode, top_edge, *score, printer);
    }

    let intent_a = store.get_intent(&top_edge.from_id)?.ok_or_else(|| {
        anyhow::anyhow!(
            "Intent '{}' not found in DB — graph inconsistency; run `loom doctor`.",
            top_edge.from_id
        )
    })?;
    let intent_b_opt = store.get_intent(&top_edge.to_id)?;

    let mut implements_a = store.list_implements_for_intent(&top_edge.from_id)?;
    let implements_total = cap_section(&mut implements_a);
    let mut validations = store.validations_for_intent(&top_edge.from_id)?;
    let validations_total = cap_section(&mut validations);

    let (role, effort) = relates_dispatch(mode, top_edge, *score);

    let mut notes = Vec::new();
    if !top_edge.id.is_empty() {
        notes.extend(store.notes_for_target(&top_edge.id)?);
    }
    notes.extend(store.notes_for_target(&top_edge.from_id)?);
    notes.extend(store.notes_for_target(&top_edge.to_id)?);
    let (notes, notes_total) = note_surfaces(notes, role);
    let suggested_action = build_suggested_action(top_edge, score);

    let item = WorkItem {
        edge_type: EdgeType::RelatesTo.to_string(),
        edge_id: top_edge.id.clone(),
        inspection_status: top_edge.inspection_status.clone(),
        criterion: top_edge.criterion.clone(),
        evidence: top_edge.evidence.clone(),
        priority_score: *score,
        discovery_class: top_edge.discovery_class.clone(),
        discovery_signals: top_edge.discovery_signals.clone(),
        discovery_centrality: top_edge.discovery_centrality.clone(),
        intent_a: IntentSurface::from(&intent_a),
        intent_b: intent_b_opt.as_ref().map(IntentSurface::from),
        implements: implements_a.iter().map(GroundingSurface::from).collect(),
        validations: validations.iter().map(ValidationSurface::from).collect(),
        notes,
        suggested_action,
    };

    let gs = store.graph_state(&snapshot)?;

    if printer.json {
        let mut v = serde_json::to_value(&item)?;
        if let Some(obj) = v.as_object_mut() {
            obj.insert("graph_state".to_string(), pulse_json(&gs));
            obj.insert("notes_total".to_string(), notes_total.into());
            obj.insert("implements_total".to_string(), implements_total.into());
            obj.insert("validations_total".to_string(), validations_total.into());
            add_dispatch(obj, role, effort);
        }
        printer.print_json(&v);
        return Ok(());
    }

    println!(
        "── Next Work Item  [mode={}  priority={:.2}] ─────────────────────────────",
        mode, score
    );
    println!();

    println!("── Intent A ────────────────────────────────────────────────────────");
    println!("{}", fmt_intent_surface(&item.intent_a));
    println!();

    if let Some(ref intent_b) = item.intent_b {
        println!("── Intent B ────────────────────────────────────────────────────────");
        println!("{}", fmt_intent_surface(intent_b));
        println!();
    }

    println!("── Edge  [RELATES_TO] ──────────────────────────────────────────────");
    println!("{}", fmt_edge_detail(top_edge));
    println!();

    if !item.implements.is_empty() {
        println!("── Related Code Files ──────────────────────────────────────────────");
        for imp in &item.implements {
            if imp.locator.is_empty() {
                println!("  {}", imp.path);
            } else {
                println!("  {}  @ {}", imp.path, imp.locator);
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

    if !item.validations.is_empty() {
        println!("── Validations on Intent A ─────────────────────────────────────────");
        for v in &item.validations {
            let result_mark = match v.result.as_str() {
                "passed" => "✓",
                "failed" => "✗",
                "blocked" => "⊘",
                _ => "?",
            };
            println!(
                "  {} {}  [{}]  cmd: {}",
                result_mark, v.name, v.result, v.command
            );
        }
        if let Some(m) = more_marker(
            validations_total,
            item.validations.len(),
            "loom validation list",
        ) {
            println!("  {m}");
        }
        println!();
    }

    if !item.notes.is_empty() {
        if notes_total > item.notes.len() {
            println!(
                "── Notes ({}, showing {}) ─────────────────────────────────────────",
                notes_total,
                item.notes.len()
            );
        } else {
            println!(
                "── Notes ({}) ──────────────────────────────────────────────────────",
                item.notes.len()
            );
        }
        for n in &item.notes {
            if n.times > 1 {
                println!("  [{}] {}  ({}, ×{})", n.kind, n.text, n.author, n.times);
            } else {
                println!("  [{}] {}  ({})", n.kind, n.text, n.author);
            }
        }
        let fetch = if top_edge.id.is_empty() {
            note_list_intent_command(&top_edge.from_id)
        } else {
            format!("loom note list --edge {}", top_edge.id)
        };
        if let Some(m) = more_marker(notes_total, item.notes.len(), &fetch) {
            println!("  {m}");
        }
        println!();
    }

    println!("── Suggested Action ────────────────────────────────────────────────");
    println!("{}", item.suggested_action);
    println!();
    println!("  Dispatch — {}  [effort: {effort}]", dispatch_line(role));
    println!("  {}", fmt_pulse(&gs));

    Ok(())
}

fn run_take_quality(store: &dyn GraphReadRepository, take: usize, printer: &Printer) -> Result<()> {
    let snapshot = store.query_snapshot()?;
    let candidates = quality_candidates_from_snapshot(&snapshot);
    let gs = store.graph_state(&snapshot)?;

    if candidates.is_empty() {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "empty", "mode": "quality",
                "message": QUALITY_EMPTY_MESSAGE,
                "next_step": gs.next_action,
                "graph_state": pulse_json(&gs),
            }));
        } else {
            println!("✓ {QUALITY_EMPTY_MESSAGE}");
            println!();
            println!("  {}", fmt_pulse(&gs));
            println!("  → Next: {}", gs.next_action);
        }
        return Ok(());
    }

    const TAKE_CAP: usize = 50;
    let n = take.min(TAKE_CAP).min(candidates.len());
    let queue_total = candidates.len();
    let rule_effort: std::collections::HashMap<String, String> = store
        .list_rules()?
        .into_iter()
        .map(|r| (r.id, r.inspection_effort))
        .collect();

    let mut groups: Vec<(String, String, Vec<serde_json::Value>)> = Vec::new();
    let mut batch_lines: Vec<String> = Vec::new();
    for (g, score) in candidates.iter().take(n) {
        let mut line = serde_json::json!({
            "op": "rule_verdict",
            "rule": g.rule_id,
            "intent": g.intent_id,
            "status": "passing",
            "evidence": "<evidence>",
            "confidence": 0.9,
        });
        if g.criterion.is_empty() {
            line["criterion"] = "<criterion>".into();
        }
        batch_lines.push(line.to_string());
        let effort = rule_effort
            .get(&g.rule_id)
            .map(String::as_str)
            .filter(|e| !e.is_empty())
            .unwrap_or("mid");
        let item = serde_json::json!({
            "rule": { "id": g.rule_id, "name": g.rule_name },
            "inspection_status": g.inspection_status,
            "criterion": g.criterion,
            "notes": g.notes,
            "priority_score": score,
            "effort": effort,
        });
        match groups.iter_mut().find(|(iid, _, _)| *iid == g.intent_id) {
            Some((_, _, items)) => items.push(item),
            None => groups.push((g.intent_id.clone(), g.intent_name.clone(), vec![item])),
        }
    }
    groups.sort_by(|a, b| {
        (std::cmp::Reverse(a.2.len()), &a.1).cmp(&(std::cmp::Reverse(b.2.len()), &b.1))
    });

    let guidance = "Per group: read the intent's grounded code ONCE (`loom intent show <id>` lists files + locators), hold each rule against it, edit its template line — keep `passing` with real evidence, `failing` with the violation, `independent` when the rule has no surface here (as valuable as passing — never fake it) — then apply the whole group in ONE call: paste the edited lines into a heredoc, `loom batch - <<'EOF' … EOF` (no scratch file, nothing to clean up; a file path works for very large batches). Template lines omit `criterion` when one is recorded: `loom batch` reuses it; write a criterion only to revise it. A verdict at component altitude covers descendants: if every rule reads the same for the whole subtree, verdict the parent instead and drop the children's lines.";

    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "mode": "quality",
            "taken": n,
            "queue_total": queue_total,
            "groups": groups
                .iter()
                .map(|(iid, iname, items)| serde_json::json!({
                    "intent": { "id": iid, "name": iname },
                    "items": items,
                }))
                .collect::<Vec<_>>(),
            "batch_template": batch_lines,
            "guidance": guidance,
            "dispatch": { "role": "quality", "effort": "per-item (see items[].effort)" },
            "graph_state": pulse_json(&gs),
        }));
        return Ok(());
    }

    println!("── Next: {n} of {queue_total}  [mode=quality] — bulk read, grouped by intent ────",);
    for (iid, iname, items) in &groups {
        println!();
        println!(
            "  {} ({} verdict(s) to earn)  [{}]",
            iname,
            items.len(),
            iid
        );
        for it in items {
            println!(
                "    [{:<21} {:>5.2}  effort={}]  {}",
                it["inspection_status"].as_str().unwrap_or(""),
                it["priority_score"].as_f64().unwrap_or(0.0),
                it["effort"].as_str().unwrap_or("mid"),
                it["rule"]["name"].as_str().unwrap_or(""),
            );
        }
    }
    println!();
    println!("{BATCH_TEMPLATE_TITLE}");
    for l in &batch_lines {
        println!("  {l}");
    }
    println!();
    println!("  {guidance}");
    println!();
    println!("  Dispatch — quality lane; effort is per item (the rule's annotation).");
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
    db: &dyn GraphReadRepository,
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
    //
    // ONE note scan for the whole take, indexed by target. `notes_for_target`
    // per item re-scans the entire Note label (notes dominate a mature graph —
    // thousands of nodes), turning a 50-item take into 50 full scans: tens of
    // seconds for a READ. Same O(N·M) trap migrate.rs documents; the bulk
    // paths (sync, align_candidates) already index — so does this one.
    // Iteration is oldest→newest, so later inserts overwrite = latest flip wins.
    let mut latest_cause: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for nt in db.notes_by_kind("transition")? {
        if let Some(cause) = parse_sync_cause(&nt.text) {
            latest_cause.insert(nt.target_id, cause.to_string());
        }
    }
    let mut groups: Vec<(String, Vec<serde_json::Value>)> = Vec::new();
    let mut batch_lines: Vec<String> = Vec::new();
    let mut has_fixer = false;
    let mut has_analyzer = false;
    let mut max_effort = "low";
    for (edge, score) in candidates.iter().take(n) {
        // Per-item, authoritative dispatch — the fix queue mixes lanes
        // (failing → fixer code repair, needs_reverification → analyzer
        // re-inspection), so the role/effort belongs on each item, not flat
        // over the batch.
        let (role, effort) = relates_dispatch(mode, edge, *score);
        if role == "fixer" {
            has_fixer = true;
        } else {
            has_analyzer = true;
        }
        if effort_rank(effort) > effort_rank(max_effort) {
            max_effort = effort;
        }
        let staled_by = latest_cause.get(&edge.id).cloned().unwrap_or_default();
        // A `failing` edge is fixer work — it needs a code repair at root cause,
        // not a verdict flip. Offering an `op: ground` line for it would invite
        // marking a known-failing edge passing with no fix (laundering green).
        // So only re-inspection items (analyzer) get a ground template; failing
        // items are listed for repair and re-grounded by hand after the code is
        // fixed. Re-inspection usually re-affirms the recorded criterion, so the
        // line OMITS it (`loom batch` reuses the stored text); a bare edge gets a
        // placeholder the gates reject unedited.
        if role != "fixer" {
            let mut line = serde_json::json!({
                "op": "ground",
                "a": edge.from_id,
                "b": edge.to_id,
                "confidence": 0.9,
            });
            if edge.criterion.is_empty() {
                line["criterion"] = "<criterion>".into();
            }
            batch_lines.push(line.to_string());
        }
        let item = serde_json::json!({
            "edge_id": edge.id,
            "a": { "id": edge.from_id, "name": edge.from_name },
            "b": { "id": edge.to_id, "name": edge.to_name },
            "inspection_status": edge.inspection_status,
            "owner_role": role,
            "effort": effort,
            "criterion": edge.criterion,
            "notes": edge.notes,
            "discovery_class": edge.discovery_class,
            "discovery_signals": edge.discovery_signals,
            "discovery_centrality": edge.discovery_centrality,
            "priority_score": score,
        });
        match groups.iter_mut().find(|(f, _)| *f == staled_by) {
            Some((_, items)) => items.push(item),
            None => groups.push((staled_by, vec![item])),
        }
    }
    // Batch-level dispatch reflects the actual mix, never a single false label.
    let batch_role = match (has_analyzer, has_fixer) {
        (true, true) => "mixed",
        (false, true) => "fixer",
        _ => "analyzer",
    };
    // Biggest file-group first (one read pays for the most verdicts);
    // ungrouped ("") last.
    groups.sort_by(|a, b| {
        (a.0.is_empty(), std::cmp::Reverse(a.1.len()), &a.0).cmp(&(
            b.0.is_empty(),
            std::cmp::Reverse(b.1.len()),
            &b.0,
        ))
    });

    let guidance = "Per group: read the staling file ONCE, decide each claim — keep `ground` if it still holds, rewrite to `issue` (+evidence) on breakage, `independent` if there is no relationship — then apply every verdict in ONE call: paste the edited lines into a heredoc, `loom batch - <<'EOF' … EOF` (no scratch file, no repo pollution, nothing to clean up; a file path works for very large batches). Template lines omit `criterion`: `loom batch` reuses the recorded one; write a criterion only to REVISE the claim. Each item carries its own `owner_role`: re-inspection is analyzer work (these are the template lines); items marked `fixer` are FAILING edges — repair the code at root cause first, then re-ground them by hand. A failing edge is deliberately NOT in the ground template (no marking it passing without a fix).";

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
            "dispatch": { "role": batch_role, "effort": max_effort },
            "graph_state": pulse_json(gs),
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
            let mut suffix = String::new();
            if let Some(class) = it["discovery_class"]
                .as_str()
                .filter(|class| !class.is_empty())
            {
                suffix.push_str(&format!(" — class: {class}"));
            }
            if let Some(notes) = it["notes"].as_str().filter(|notes| !notes.is_empty()) {
                suffix.push_str(&format!(" — {notes}"));
            }
            println!(
                "    [{:<21} {:>8} {:>5.2}]  {} × {}  ({}){}",
                it["inspection_status"].as_str().unwrap_or(""),
                it["owner_role"].as_str().unwrap_or(""),
                it["priority_score"].as_f64().unwrap_or(0.0),
                it["a"]["name"].as_str().unwrap_or(""),
                it["b"]["name"].as_str().unwrap_or(""),
                if it["edge_id"].as_str().unwrap_or("").is_empty() {
                    "no edge yet"
                } else {
                    it["edge_id"].as_str().unwrap_or("")
                },
                suffix,
            );
        }
    }
    println!();
    println!("{BATCH_TEMPLATE_TITLE}");
    for l in &batch_lines {
        println!("  {l}");
    }
    println!();
    println!("  {guidance}");
    println!();
    println!("  Dispatch — {batch_role}  [effort: {max_effort}]   (per-item owner_role above is authoritative)");
    println!("  {}", fmt_pulse(gs));
    Ok(())
}

// ---------------------------------------------------------------------------
// `--compact` — the single-item PROJECTION: verdict coordinates only.
//
// The full work item front-loads everything an agent COULD need (intent
// descriptions, validations, notes, pulse); an agent that already knows the
// loop only needs where to look and what to run. Compact serves exactly
// that — intent ids/names, edge id, top grounded paths, a one-line command —
// and names the dig commands for everything it elides. It also skips the
// graph_state computation entirely (the pulse can be O(N²) in the audit
// phase), so the cheap read is cheap end-to-end.
// ---------------------------------------------------------------------------

fn run_compact(
    db: &dyn GraphReadRepository,
    mode: &str,
    edge: &crate::types::RelatesTo,
    score: f64,
    printer: &Printer,
) -> Result<()> {
    // Top grounded paths only — half the full item's section cap.
    const COMPACT_PATHS: usize = 5;
    let mut grounded: Vec<String> = db
        .list_implements_for_intent(&edge.from_id)?
        .into_iter()
        .map(|im| {
            if im.locator.is_empty() {
                im.codefile_path
            } else {
                format!("{} @ {}", im.codefile_path, im.locator)
            }
        })
        .collect();
    let implements_total = grounded.len();
    grounded.truncate(COMPACT_PATHS);

    let (role, effort) = relates_dispatch(mode, edge, score);
    let suggested_action = build_suggested_action_compact(edge);
    let dig = if edge.id.is_empty() {
        format!(
            "loom next --mode {mode} (full item) · loom intent show {}",
            edge.from_id
        )
    } else {
        format!(
            "loom next --mode {mode} (full item) · loom edge show {}",
            edge.id
        )
    };

    if printer.json {
        printer.print_json(&serde_json::json!({
            "mode": mode,
            "edge_id": edge.id,
            "inspection_status": edge.inspection_status,
            "priority_score": score,
            "discovery_class": edge.discovery_class,
            "discovery_signals": edge.discovery_signals,
            "discovery_centrality": edge.discovery_centrality,
            "a": { "id": edge.from_id, "name": edge.from_name },
            "b": { "id": edge.to_id, "name": edge.to_name },
            "implements": grounded,
            "implements_total": implements_total,
            "suggested_action": suggested_action,
            "owner_role": role,
            "effort": effort,
            "dig": dig,
        }));
        return Ok(());
    }

    println!("── Next (compact)  [mode={mode}  priority={score:.2}] ───────────────────────");
    println!(
        "  '{}' × '{}'  [{}]{}",
        edge.from_name,
        edge.to_name,
        edge.inspection_status,
        if edge.id.is_empty() {
            String::new()
        } else {
            format!("  edge: {}", edge.id)
        },
    );
    if !grounded.is_empty() {
        let more = if implements_total > grounded.len() {
            format!("  (+{} more)", implements_total - grounded.len())
        } else {
            String::new()
        };
        println!("  code: {}{}", grounded.join(" · "), more);
    }
    if !edge.discovery_class.is_empty() {
        println!(
            "  discovery: {}{}",
            edge.discovery_class,
            if edge.notes.is_empty() {
                String::new()
            } else {
                format!(" — {}", edge.notes)
            }
        );
    }
    println!("  → {suggested_action}");
    println!("  Dispatch — {role}  [effort: {effort}]   dig: {dig}");
    Ok(())
}

/// The one-line counterpart of `build_suggested_action`: the same decision,
/// stripped to a single runnable template (compact mode and machine drivers).
fn build_suggested_action_compact(edge: &crate::types::RelatesTo) -> String {
    match edge.inspection_status.as_str() {
        "failing" => format!(
            "fix the code, `loom sync`, then `loom edge fix {} --description \"<what changed>\"`",
            edge.id
        ),
        "needs_reverification" => format!(
            "re-inspect: loom edge explore {from} {to} ground --criterion \"<updated>\" --confidence 0.9  (or: issue / independent)",
            from = edge.from_id, to = edge.to_id,
        ),
        _ => format!(
            "loom edge explore {from} {to} ground --criterion \"<text>\" --confidence 0.9  (or: issue --evidence \"…\" / independent --notes \"…\")",
            from = edge.from_id, to = edge.to_id,
        ),
    }
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
        "analyzer" => "criterion, evidence, confidence, inspection_status (the verdict)",
        "builder" => {
            "write code → `loom codefile add` → `loom edge implement` (locator) → mark implemented"
        }
        "fixer" => "the minimal change → `loom edge fix` / mark implemented",
        "validator" => "run the proof (or `loom validation mark`) → `loom intent confirm`",
        "quality" => "the GOVERNS verdict — criterion, evidence, confidence (`loom rule verdict`)",
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

fn relates_dispatch(
    mode: &str,
    edge: &crate::types::RelatesTo,
    score: f64,
) -> (&'static str, &'static str) {
    let role = match (mode, edge.inspection_status.as_str()) {
        ("fix", "failing") => "fixer",
        _ => "analyzer",
    };
    (role, relates_effort(edge, score))
}

/// Order the effort tiers so a bulk take can report the highest it contains.
fn effort_rank(effort: &str) -> u8 {
    match effort {
        "high" => 2,
        "mid" => 1,
        _ => 0,
    }
}

fn relates_effort(edge: &crate::types::RelatesTo, score: f64) -> &'static str {
    let centrality =
        edge.discovery_centrality.a_degree.max(0) + edge.discovery_centrality.b_degree.max(0);
    let signal_count = edge.discovery_signals.len();
    let structural_weight = if centrality > 0 {
        centrality as f64 + (signal_count as f64 * 3.0)
    } else {
        score
    };

    if structural_weight >= 20.0 || signal_count >= 3 {
        "high"
    } else if structural_weight >= 8.0 || signal_count > 0 {
        "mid"
    } else {
        "low"
    }
}

/// Inject `owner_role` + `effort` + `dispatch` into a work-item JSON object.
/// `effort` (low | mid | high) names how much capability THIS item's work
/// needs — a statement about the work, computed from structure; the harness
/// maps it to whatever models exist. Never a model name.
fn add_dispatch(obj: &mut serde_json::Map<String, serde_json::Value>, role: &str, effort: &str) {
    obj.insert("owner_role".to_string(), serde_json::json!(role));
    obj.insert("effort".to_string(), serde_json::json!(effort));
    obj.insert(
        "dispatch".to_string(),
        serde_json::json!(dispatch_line(role)),
    );
}

// ---------------------------------------------------------------------------
// --all: the CLOSEOUT view — every role queue at once. One prioritized answer
// to "what's left?" instead of five `next` calls + status + doctor reconciled
// by hand. Read-only; each line carries the exact command that works it.
// ---------------------------------------------------------------------------

fn run_all(
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
    let build = build_candidates_from_snapshot(&snapshot);
    let fix = scored_candidates_from_snapshot(&snapshot, "fix");
    let discovery_uninspected = snapshot
        .relates
        .iter()
        .filter(|e| e.inspection_status == "uninspected")
        .count() as i64;
    let validate = validate_candidates_from_snapshot(&snapshot);
    let quality = quality_candidates_from_snapshot(&snapshot);
    let blocked = blocked_validation_summary_from_snapshot(&snapshot);
    let blocked_validation_audit = blocked.autonomous_validation_count();
    let human_blocked_validations = blocked.human_validation_count();
    let smells_computed = all_smells.is_some();
    let all_smells = all_smells.unwrap_or_default();
    let smells_total = all_smells.len();
    let smells_top: Vec<_> = all_smells.into_iter().take(3).collect();
    let (inbox_untriaged, inbox_triaged, inbox_deferred) = inbox_counts(&inbox_items);

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
            "command": "loom inbox triage --take 20",
            "top": format!("{} untriaged, {} triaged intake card(s); candidates, not graph truth", inbox_untriaged, inbox_triaged),
        }));
    }
    if !build.is_empty() {
        let c = &build[0];
        queues.push(serde_json::json!({
            "queue": "build", "role": if c.intent.lifecycle == "needs_change" { "fixer" } else { "builder" },
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
    let review = review_candidates_from_snapshot(&snapshot);
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
            "queue": "optional-enrichment", "role": "analyzer", "gate": "autonomous", "optional": true,
            "count": discovery_backlog, "command": "loom next",
            "top": "optional graph enrichment: horizontal N×N discovery, not required for done",
        }));
    }

    // The oscillation summary: how much of the remainder needs the user.
    let human_gated: i64 = queues
        .iter()
        .filter(|q| q["gate"].as_str() == Some("human"))
        .map(|q| q["count"].as_i64().unwrap_or(0))
        .sum();

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
        let optional_enrichment: i64 = queues
            .iter()
            .filter(|q| q["queue"].as_str() == Some("optional-enrichment"))
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
            "optional_graph_enrichment".to_string(),
            serde_json::json!(optional_enrichment),
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
            serde_json::json!(blocked.affected_proof_edges),
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

// ---------------------------------------------------------------------------
// Build mode: realize `planned` / `needs_change` intents (greenfield/refactor)
// ---------------------------------------------------------------------------

fn run_build(db: &dyn GraphReadRepository, printer: &Printer) -> Result<()> {
    db.ensure_owned("work the build queue (there is nothing to build in someone else's repo)")?;
    // ONE snapshot feeds both the queue and the pulse (production uses the
    // same snapshot scoring as the compass — coherence by construction — and
    // avoids a second full graph load).
    let snapshot = db.query_snapshot()?;
    let candidates = build_candidates_from_snapshot(&snapshot);
    let gs = db.graph_state(&snapshot)?;

    if candidates.is_empty() {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "empty", "mode": "build",
                "message": "No planned or needs_change intents — nothing to build.",
                "next_step": gs.next_action,
                "graph_state": pulse_json(&gs),
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
    let mut implements = db.list_implements_for_intent(&intent.id)?;
    let implements_total = cap_section(&mut implements);
    let mut validations = db.validations_for_intent(&intent.id)?;
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
    let (notes, notes_total) = note_surfaces(db.notes_for_target(&intent.id)?, role);
    let action = build_action(intent, c.rollup);

    let item = WorkItem {
        edge_type: "BUILD".to_string(),
        edge_id: String::new(),
        inspection_status: intent.lifecycle.clone(),
        criterion: String::new(),
        evidence: String::new(),
        priority_score: *score,
        discovery_class: String::new(),
        discovery_signals: Vec::new(),
        discovery_centrality: Default::default(),
        intent_a: IntentSurface::from(intent),
        intent_b: None,
        implements: implements.iter().map(GroundingSurface::from).collect(),
        validations: validations.iter().map(ValidationSurface::from).collect(),
        notes,
        suggested_action: action.clone(),
    };

    if printer.json {
        let mut v = serde_json::to_value(&item)?;
        if let Some(obj) = v.as_object_mut() {
            obj.insert("graph_state".to_string(), pulse_json(&gs));
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
    println!("{}", fmt_intent_surface(&item.intent_a));
    println!();
    if !implements.is_empty() {
        println!("── Currently grounded at ───────────────────────────────────────────");
        for im in &implements {
            let loc = if im.locator.is_empty() {
                String::new()
            } else {
                format!("  @ {}", im.locator)
            };
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
    if !item.notes.is_empty() {
        if notes_total > item.notes.len() {
            println!(
                "── Notes ({}, showing {}) ─────────────────────────────────────────",
                notes_total,
                item.notes.len()
            );
        } else {
            println!(
                "── Notes ({}) ──────────────────────────────────────────────────────",
                item.notes.len()
            );
        }
        for n in &item.notes {
            if n.times > 1 {
                println!("  [{}] {}  ({}, ×{})", n.kind, n.text, n.author, n.times);
            } else {
                println!("  [{}] {}  ({})", n.kind, n.text, n.author);
            }
        }
        if let Some(m) = more_marker(
            notes_total,
            item.notes.len(),
            &note_list_intent_command(&intent.id),
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

fn run_validate(db: &dyn GraphReadRepository, printer: &Printer) -> Result<()> {
    // ONE snapshot for both the queue and the pulse (shares the compass's
    // validate_selection scoring; no second full graph load).
    let snapshot = db.query_snapshot()?;
    let candidates = validate_candidates_from_snapshot(&snapshot);
    let gs = db.graph_state(&snapshot)?;

    if candidates.is_empty() {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "empty", "mode": "validate",
                "message": "Every intent's proof is green — nothing to validate.",
                "next_step": gs.next_action,
                "graph_state": pulse_json(&gs),
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
    let mut validations = db.validations_for_intent(&c.intent.id)?;
    let (notes, notes_total) = note_surfaces(db.notes_for_target(&c.intent.id)?, "validator");
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
        let empty_validations: Vec<_> = validations
            .iter()
            .filter(|v| {
                v.command.trim().is_empty()
                    && (v.last_result == "not_run" || v.last_result.is_empty())
            })
            .collect();
        if !empty_validations.is_empty() {
            let fixes = empty_validations
                .iter()
                .map(|v| {
                    format!(
                        "  - {}: loom validation update {} --command \"…\"  OR  loom validation mark {} --result passed|failed --evidence \"…\"",
                        v.name, v.id, v.id
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "This intent has validation(s) with no command; `loom validate {id}` will skip them.\n\
                 Add commands or record manual verdicts:\n{fixes}",
                id = c.intent.id,
            )
        } else {
            let saga_hint = validations
                .iter()
                .find(|v| v.validation_type == "saga")
                .map(|v| format!(
                    "\nA saga proof is linked — triage failures without stamping via `loom saga diagnose {}`, then stamp proof with `loom saga run {}`.",
                    v.name,
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
        }
    };
    let validations_total = cap_section(&mut validations);

    if printer.json {
        printer.print_json(&serde_json::json!({
            "mode":             "validate",
            "reason":           c.reason,
            "priority_score":   c.score,
            "intent":           IntentSurface::from(&c.intent),
            "validations":      validations.iter().map(ValidationSurface::from).collect::<Vec<_>>(),
            "validations_total": validations_total,
            "notes":            notes,
            "notes_total":      notes_total,
            "suggested_action": action,
            "owner_role":       "validator",
            "effort":           if validations.is_empty() { "mid" } else { "low" },
            "dispatch":         dispatch_line("validator"),
            "graph_state":      pulse_json(&gs),
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
    println!("{}", fmt_intent_surface(&IntentSurface::from(&c.intent)));
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
            println!(
                "  {} {}  [{}]  cmd: {}",
                mark, v.name, v.last_result, v.command
            );
        }
        if let Some(m) = more_marker(validations_total, validations.len(), "loom validation list") {
            println!("  {m}");
        }
        println!();
    }
    // Notes targeting this intent (parity with json, which already ships them;
    // addressed-to-validator handoffs surface first via note_surfaces).
    if !notes.is_empty() {
        if notes_total > notes.len() {
            println!(
                "── Notes ({}, showing {}) ─────────────────────────────────────────",
                notes_total,
                notes.len()
            );
        } else {
            println!(
                "── Notes ({}) ──────────────────────────────────────────────────────",
                notes.len()
            );
        }
        for n in &notes {
            if n.times > 1 {
                println!("  [{}] {}  ({}, ×{})", n.kind, n.text, n.author, n.times);
            } else {
                println!("  [{}] {}  ({})", n.kind, n.text, n.author);
            }
        }
        if let Some(m) = more_marker(
            notes_total,
            notes.len(),
            &note_list_intent_command(&c.intent.id),
        ) {
            println!("  {m}");
        }
        println!();
    }
    println!("── Suggested Action ────────────────────────────────────────────────");
    println!("{}", action);
    println!();
    println!(
        "  Dispatch — {}  [effort: {}]",
        dispatch_line("validator"),
        if validations.is_empty() { "mid" } else { "low" }
    );
    println!("  {}", fmt_pulse(&gs));
    Ok(())
}

// ---------------------------------------------------------------------------
// Align mode: the validator's user↔intent drift queue — meaning to re-affirm
// ---------------------------------------------------------------------------

fn run_take_align(store: &dyn GraphReadRepository, take: usize, printer: &Printer) -> Result<()> {
    let snapshot = store.query_snapshot()?;
    let candidates = store.align_candidates(&snapshot)?;
    let gs = store.graph_state(&snapshot)?;

    if candidates.is_empty() {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "empty", "mode": "align",
                "message": ALIGN_EMPTY_MESSAGE,
                "next_step": gs.next_action,
                "graph_state": pulse_json(&gs),
            }));
        } else {
            println!("✓ No drift suspected — nothing to batch for alignment.");
            println!();
            println!("  {}", fmt_pulse(&gs));
            println!("  → Next: {}", gs.next_action);
        }
        return Ok(());
    }

    const TAKE_CAP: usize = 50;
    let n = take.min(TAKE_CAP).min(candidates.len());
    let queue_total = candidates.len();
    let items: Vec<_> = candidates
        .iter()
        .take(n)
        .map(|c| {
            let visibility = if c.intent.visibility.is_empty() {
                "untriaged"
            } else {
                c.intent.visibility.as_str()
            };
            let audience = match visibility {
                "user_visible" => "user-visible capability",
                "internal" => "internal machinery",
                _ => "untriaged: ask whether this is user-visible capability or internal machinery",
            };
            let id = c.intent.id.as_str();
            serde_json::json!({
                "intent": {
                    "id": id,
                    "name": c.intent.name,
                    "level": c.intent.abstraction_level,
                    "visibility": visibility,
                    "description": c.intent.description,
                },
                "last_confirmed": c.last_confirmed,
                "churn_since_confirm": c.churn_since_confirm,
                "degree": c.degree,
                "score": c.score,
                "ask": format!("Does '{}' still match what you expect loom to do here?", c.intent.name),
                "audience_prompt": audience,
                "commands": {
                    "confirm": format!("loom intent confirm {id}"),
                    "confirm_internal": format!("loom intent confirm {id} --visibility internal"),
                    "reword": format!("loom intent update {id} --description \"…\" --reword --reason \"user clarified wording during align\""),
                    "update_meaning": format!("loom intent update {id} --description \"…\" --reason \"user changed expected behavior during align\""),
                    "retire": format!("loom intent retire {id} --reason \"superseded during align\" --replaced-by <successor>"),
                },
            })
        })
        .collect();
    let guidance = "Use this as ONE human agenda. For each item, align the concept in plain language, not implementation wording. Record exactly one outcome: confirm, confirm --visibility internal, reword, update meaning, retire, or add a newly revealed missing concept. After recording outcomes, rerun `loom next --mode align --take <N>` until it is empty.";

    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "mode": "align",
            "taken": n,
            "queue_total": queue_total,
            "items": items,
            "guidance": guidance,
            "dispatch": { "role": "validator", "effort": "mid", "gate": "human" },
            "graph_state": pulse_json(&gs),
        }));
        return Ok(());
    }

    println!("── Align agenda: {n} of {queue_total} human-gated meaning check(s) ────");
    for (idx, item) in items.iter().enumerate() {
        println!();
        println!(
            "  {}. {}  [{} · {}]",
            idx + 1,
            item["intent"]["name"].as_str().unwrap_or(""),
            item["intent"]["visibility"].as_str().unwrap_or(""),
            item["intent"]["id"].as_str().unwrap_or("")
        );
        println!(
            "     {}",
            item["intent"]["description"].as_str().unwrap_or("")
        );
        println!("     ask: {}", item["ask"].as_str().unwrap_or(""));
        println!(
            "     confirm: {}",
            item["commands"]["confirm"].as_str().unwrap_or("")
        );
        if item["intent"]["visibility"].as_str() == Some("untriaged") {
            println!(
                "     internal: {}",
                item["commands"]["confirm_internal"].as_str().unwrap_or("")
            );
        }
    }
    println!();
    println!("  {guidance}");
    println!("  {}", fmt_pulse(&gs));
    Ok(())
}

fn run_align(store: &dyn GraphReadRepository, printer: &Printer) -> Result<()> {
    let snapshot = store.query_snapshot()?;
    let candidates = store.align_candidates(&snapshot)?;
    let gs = store.graph_state(&snapshot)?;

    if candidates.is_empty() {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "empty", "mode": "align",
                "message": ALIGN_EMPTY_MESSAGE,
                "next_step": gs.next_action,
                "graph_state": pulse_json(&gs),
            }));
        } else {
            println!("✓ {ALIGN_EMPTY_MESSAGE}");
            println!();
            println!("  {}", fmt_pulse(&gs));
            println!("  → Next: {}", gs.next_action);
        }
        return Ok(());
    }

    let c = &candidates[0];
    let mut groundings = store.list_implements_for_intent(&c.intent.id)?;
    let groundings_total = cap_section(&mut groundings);
    let (notes, notes_total) = note_surfaces(store.notes_for_target(&c.intent.id)?, "validator");
    let last_confirmed = c.last_confirmed.as_deref().unwrap_or("never");

    let mut parent_chain: Vec<String> = Vec::new();
    let mut immediate_parent_id: Option<String> = None;
    let mut cursor = c.intent.id.clone();
    for _ in 0..6 {
        let Some(h) = store
            .list_hierarchy_for_intent(&cursor)?
            .into_iter()
            .find(|h| h.child_id == cursor)
        else {
            break;
        };
        if immediate_parent_id.is_none() {
            immediate_parent_id = Some(h.parent_id.clone());
        }
        parent_chain.push(h.parent_name.clone());
        cursor = h.parent_id;
    }
    parent_chain.reverse();
    let mut siblings: Vec<String> = Vec::new();
    if let Some(pid) = &immediate_parent_id {
        siblings = store
            .list_hierarchy_for_intent(pid)?
            .into_iter()
            .filter(|h| h.parent_id == *pid && h.child_id != c.intent.id)
            .map(|h| h.child_name)
            .take(5)
            .collect();
    }
    let independent_of: Vec<String> = store
        .edges_for_intent(&c.intent.id)?
        .into_iter()
        .filter(|e| e.inspection_status == "independent")
        .map(|e| {
            if e.from_id == c.intent.id {
                e.to_name
            } else {
                e.from_name
            }
        })
        .take(5)
        .collect();
    let visibility = if c.intent.visibility.is_empty() {
        "untriaged"
    } else {
        c.intent.visibility.as_str()
    };
    let audience_brief = match visibility {
        "internal" => "internal machinery (serves other parts; the user never touches it directly)"
            .to_string(),
        "user_visible" => "a user-visible capability (the user can see or feel it)".to_string(),
        _ => "untriaged — decide from the groundings whether this is user-visible capability or \
              internal machinery, OPEN with that framing, and let the user correct you"
            .to_string(),
    };
    let where_it_sits = if parent_chain.is_empty() {
        c.intent.name.clone()
    } else {
        format!("{} → {}", parent_chain.join(" → "), c.intent.name)
    };
    let mut not_this = Vec::new();
    if !siblings.is_empty() {
        not_this.push(format!(
            "sibling concepts (distinct on purpose): {}",
            siblings.join(" · ")
        ));
    }
    if !independent_of.is_empty() {
        not_this.push(format!(
            "verified independent of: {}",
            independent_of.join(" · ")
        ));
    }
    let not_this_block = if not_this.is_empty() {
        String::new()
    } else {
        format!("\n  what it is NOT: {}", not_this.join("; "))
    };
    let action = format!(
        "Interview move — align the CONCEPT, not the wording. Present, in the user's plain \
         language:\n  \
         1. what the product can DO because this exists — one or two sentences, no file \
         paths, no internal nouns (jargon test: would a non-coder nod?)\n  \
         2. why it matters: its place in the design — {where_it_sits}\n  \
         3. its audience, UP FRONT: {audience_brief}{not_this}\n\
         Vocabulary stays out of the question unless the user asks, gets confused, or uses \
         a term that conflicts with the graph — then reconcile terms, not before.\n\
         \n  meaning on record (graph-speak — source material, never read aloud): {description}\n\
         \nAsk: \"does that match what you expect this product to do here?\" \
         Record exactly ONE outcome:\n  \
         - concept still right → loom intent confirm {id}\n  \
         - words confusing, concept right → loom intent update {id} --description \"…\" \
         --reword --reason \"…\"  (no ripple; the clock still resets)\n  \
         - concept evolved → translate their answer into a falsifiable description: \
         loom intent update {id} --description \"…\" --reason \"…\"  (ripples; audience re-triaged)\n  \
         - internal machinery, stop asking → loom intent confirm {id} --visibility internal  \
         (leaves this queue until the meaning is redefined)\n  \
         - superseded → loom intent retire {id} --reason \"…\" --replaced-by <successor>\n  \
         - missing concept revealed → loom intent add … --lifecycle planned\n\
         \nThe user rules on BEHAVIOR, never on wording.",
        description = c.intent.description.as_str(),
        id = c.intent.id.as_str(),
        not_this = not_this_block,
    );

    let dispatch = "this is validator work — fills the alignment outcome: present the CONCEPT \
         to the USER, then record exactly ONE of `loom intent confirm` (optionally \
         `--visibility internal`) / `update` (`--reword` for wording-only) / `retire` / \
         `add` (update/retire/add are builder-lane: a solo agent records them directly; a \
         role-split validator hands the user's words to a builder). Whoever takes it declares \
         `LOOM_AGENT=llm:validator` (or stay bare `llm` for solo); its queue is \
         `loom next --mode align`. One item per question — after recording the outcome, pull \
         the queue AGAIN: it admits only drift suspects and every outcome resets that intent's \
         clock, so it drains to empty. Iterate until it reports clean; never stop after one \
         answer, never interrogate beyond what it serves.";

    if printer.json {
        let mut obj = serde_json::Map::new();
        obj.insert("mode".to_string(), serde_json::json!("align"));
        obj.insert(
            "intent".to_string(),
            serde_json::json!(IntentSurface::from(&c.intent)),
        );
        obj.insert(
            "last_confirmed".to_string(),
            serde_json::json!(c.last_confirmed),
        );
        obj.insert(
            "churn_since_confirm".to_string(),
            serde_json::json!(c.churn_since_confirm),
        );
        obj.insert("degree".to_string(), serde_json::json!(c.degree));
        obj.insert("score".to_string(), serde_json::json!(c.score));
        obj.insert(
            "queue_depth".to_string(),
            serde_json::json!(candidates.len()),
        );
        obj.insert("visibility".to_string(), serde_json::json!(visibility));
        obj.insert(
            "where_it_sits".to_string(),
            serde_json::json!(where_it_sits),
        );
        obj.insert(
            "not_to_confuse_with".to_string(),
            serde_json::json!({
                "siblings": siblings,
                "verified_independent": independent_of,
            }),
        );
        obj.insert(
            "groundings".to_string(),
            serde_json::json!(groundings
                .iter()
                .map(GroundingSurface::from)
                .collect::<Vec<_>>()),
        );
        obj.insert(
            "groundings_total".to_string(),
            serde_json::json!(groundings_total),
        );
        obj.insert("notes".to_string(), serde_json::json!(notes));
        obj.insert("notes_total".to_string(), serde_json::json!(notes_total));
        obj.insert("suggested_action".to_string(), serde_json::json!(action));
        obj.insert("graph_state".to_string(), pulse_json(&gs));
        obj.insert("owner_role".to_string(), serde_json::json!("validator"));
        obj.insert("effort".to_string(), serde_json::json!("mid"));
        obj.insert("dispatch".to_string(), serde_json::json!(dispatch));
        printer.print_json(&serde_json::Value::Object(obj));
        return Ok(());
    }

    println!(
        "── Next Align Item  [score={:.2}]  ({} drift suspect(s) queued) ─────",
        c.score,
        candidates.len()
    );
    println!();
    println!("── Intent ──────────────────────────────────────────────────────────");
    println!("  {}  ({})", c.intent.name, c.intent.id);
    println!("  level: {}", c.intent.abstraction_level);
    println!("  description: {}", c.intent.description);
    println!(
        "  lifecycle: {}  status: {}",
        c.intent.lifecycle, c.intent.status
    );
    println!("  audience: {}", audience_brief);
    println!("  sits under: {}", where_it_sits);
    if !not_this.is_empty() {
        println!("  not this: {}", not_this.join("; "));
    }
    println!("  last_confirmed: {}", last_confirmed);
    println!(
        "  churn since confirm: {} staled-claim flip(s)",
        c.churn_since_confirm
    );
    println!();
    if !groundings.is_empty() {
        println!("── Groundings ──────────────────────────────────────────────────────");
        for im in &groundings {
            let loc = if im.locator.is_empty() {
                String::new()
            } else {
                format!("  @ {}", im.locator)
            };
            println!("  {}{}", im.codefile_path, loc);
        }
        if let Some(m) = more_marker(
            groundings_total,
            groundings.len(),
            &format!("loom intent show {}", c.intent.id),
        ) {
            println!("  {m}");
        }
        println!();
    }
    if !notes.is_empty() {
        if notes_total > notes.len() {
            println!(
                "── Notes ({}, showing {}) ─────────────────────────────────────────",
                notes_total,
                notes.len()
            );
        } else {
            println!(
                "── Notes ({}) ──────────────────────────────────────────────────────",
                notes.len()
            );
        }
        for n in &notes {
            if n.times > 1 {
                println!("  [{}] {}  ({}, ×{})", n.kind, n.text, n.author, n.times);
            } else {
                println!("  [{}] {}  ({})", n.kind, n.text, n.author);
            }
        }
        if let Some(m) = more_marker(
            notes_total,
            notes.len(),
            &note_list_intent_command(&c.intent.id),
        ) {
            println!("  {m}");
        }
        println!();
    }
    println!("── Suggested Action ────────────────────────────────────────────────");
    println!("{}", action);
    println!();
    println!("  Dispatch — {dispatch}  [effort: mid]");
    println!("  {}", fmt_pulse(&gs));
    Ok(())
}

// ---------------------------------------------------------------------------
// Quality mode: the quality agent's queue — GOVERNS edges whose green is unearned
// ---------------------------------------------------------------------------

fn run_quality(store: &dyn GraphReadRepository, printer: &Printer) -> Result<()> {
    let snapshot = store.query_snapshot()?;
    let candidates = quality_candidates_from_snapshot(&snapshot);
    let gs = store.graph_state(&snapshot)?;

    if candidates.is_empty() {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "empty", "mode": "quality",
                "message": QUALITY_EMPTY_MESSAGE,
                "next_step": gs.next_action,
                "graph_state": pulse_json(&gs),
            }));
        } else {
            println!("✓ {QUALITY_EMPTY_MESSAGE}");
            println!();
            println!("  {}", fmt_pulse(&gs));
            println!("  → Next: {}", gs.next_action);
        }
        return Ok(());
    }

    let (g, score) = &candidates[0];
    let intent = store.get_intent(&g.intent_id)?;
    let mut implements = store.list_implements_for_intent(&g.intent_id)?;
    let implements_total = cap_section(&mut implements);
    let edge_notes = if g.id.is_empty() {
        Vec::new()
    } else {
        store.notes_for_target(&g.id)?
    };
    let (notes, notes_total) = note_surfaces(edge_notes, "quality");
    let rule_effort = store
        .list_rules()?
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
            "intent":           intent.as_ref().map(IntentSurface::from),
            "implements":       implements.iter().map(GroundingSurface::from).collect::<Vec<_>>(),
            "implements_total": implements_total,
            "notes":            notes,
            "notes_total":      notes_total,
            "suggested_action": action,
            "owner_role":       "quality",
            "effort":           rule_effort,
            "dispatch":         dispatch_line("quality"),
            "graph_state":      pulse_json(&gs),
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
        println!("  {}", g.notes);
    }
    println!();
    if let Some(ref i) = intent {
        println!("── Intent ──────────────────────────────────────────────────────────");
        println!("{}", fmt_intent_surface(&IntentSurface::from(i)));
        println!();
    }
    if !implements.is_empty() {
        println!("── Grounded at ─────────────────────────────────────────────────────");
        for im in &implements {
            let loc = if im.locator.is_empty() {
                String::new()
            } else {
                format!("  @ {}", im.locator)
            };
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
    println!(
        "  Dispatch — {}  [effort: {rule_effort}]",
        dispatch_line("quality")
    );
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

fn run_review(store: &dyn GraphReadRepository, printer: &Printer) -> Result<()> {
    use crate::db::queries::{ReviewCandidate, REVIEW_CONFIDENCE};

    let snapshot = store.query_snapshot()?;
    let candidates = review_candidates_from_snapshot(&snapshot);
    let gs = store.graph_state(&snapshot)?;

    if candidates.is_empty() {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "empty", "mode": "review",
                "message": format!("No verdicts below confidence {REVIEW_CONFIDENCE} — nothing needs a second look."),
                "next_step": gs.next_action,
                "graph_state": pulse_json(&gs),
            }));
        } else {
            println!(
                "✓ No verdicts below confidence {REVIEW_CONFIDENCE} — nothing needs a second look."
            );
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
            let intent_a = store.get_intent(&e.from_id)?;
            let intent_b = store.get_intent(&e.to_id)?;
            let (notes, notes_total) = note_surfaces(store.notes_for_target(&e.id)?, "analyzer");
            let action = format!(
                "{protocol}

  loom edge explore {a} {b} ground --criterion \"…\" --confidence 0.9
  loom edge explore {a} {b} issue --criterion \"…\" --evidence \"…\"
  loom edge explore {a} {b} independent --notes \"…\"",
                a = e.from_id,
                b = e.to_id,
            );
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "mode": "review", "kind": "relates_to", "priority_score": score,
                    "edge": e, "intent_a": intent_a.as_ref().map(IntentSurface::from),
                    "intent_b": intent_b.as_ref().map(IntentSurface::from), "notes": notes,
                    "notes_total": notes_total,
                    "suggested_action": action,
                    "owner_role": "analyzer", "effort": "high",
                    "dispatch": dispatch_line("analyzer"),
                    "graph_state": pulse_json(&gs),
                }));
                return Ok(());
            }
            println!(
                "── Next Review Item  [relates_to  confidence={:.2}  priority={:.2}] ──────",
                e.confidence, score
            );
            println!();
            println!("  {} × {}", e.from_name, e.to_name);
            println!(
                "  recorded verdict: {}  (by {})",
                e.inspection_status, e.inspected_by
            );
            println!("  criterion: {}", e.criterion);
            println!();
            println!("── Suggested Action ────────────────────────────────────────────────");
            println!("{action}");
            println!();
            println!("  Dispatch — {}  [effort: high]", dispatch_line("analyzer"));
            println!("  {}", fmt_pulse(&gs));
        }
        ReviewCandidate::Governs(g) => {
            let intent = store.get_intent(&g.intent_id)?;
            let (notes, notes_total) = note_surfaces(store.notes_for_target(&g.id)?, "quality");
            let action = format!(
                "{protocol}

  loom rule verdict {r} {i} --status passing|failing|independent --criterion \"…\" --evidence \"…\" --confidence 0.9",
                r = g.rule_id, i = g.intent_id,
            );
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "mode": "review", "kind": "governs", "priority_score": score,
                    "governs": g, "intent": intent.as_ref().map(IntentSurface::from), "notes": notes,
                    "notes_total": notes_total,
                    "suggested_action": action,
                    "owner_role": "quality", "effort": "high",
                    "dispatch": dispatch_line("quality"),
                    "graph_state": pulse_json(&gs),
                }));
                return Ok(());
            }
            println!(
                "── Next Review Item  [governs  confidence={:.2}  priority={:.2}] ─────────",
                g.confidence, score
            );
            println!();
            println!("  rule {} → intent {}", g.rule_name, g.intent_name);
            println!(
                "  recorded verdict: {}  (by {})",
                g.inspection_status, g.inspected_by
            );
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
// Prove mode: the pre-decision plane's queue — proposed hypotheses awaiting
// their proof, highest target-centrality (blast radius) first. Analyzer work;
// optional like discovery/review (speculation never blocks complete).
// ---------------------------------------------------------------------------

fn run_prove(store: &dyn GraphReadRepository, printer: &Printer) -> Result<()> {
    let snapshot = store.query_snapshot()?;
    let candidates = store.prove_candidates(&snapshot)?;
    let gs = store.graph_state(&snapshot)?;

    if candidates.is_empty() {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "empty", "mode": "prove",
                "message": "No proposed hypotheses and no stale support — the pre-decision plane is clear.",
                "next_step": gs.next_action,
                "graph_state": pulse_json(&gs),
            }));
        } else {
            println!(
                "✓ No proposed hypotheses and no stale support — the pre-decision plane is clear."
            );
            println!();
            println!("  {}", fmt_pulse(&gs));
            println!("  → Next: {}", gs.next_action);
        }
        return Ok(());
    }

    let (h, score) = &candidates[0];
    let mut targets = store.list_targets_for_hypothesis(&h.id)?;
    let mut implements = Vec::new();
    for t in &targets {
        implements.extend(store.list_implements_for_intent(&t.intent_id)?);
    }
    let (notes, notes_total) = note_surfaces(store.notes_for_target(&h.id)?, "analyzer");
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
             \n  loom hypothesis prove {id} --verdict supported --evidence \"<what still holds>\" --confidence 0.9\
             \n  loom hypothesis prove {id} --verdict refuted  --evidence \"<the change resolved it>\" --confidence 0.9\
             \nRe-proving re-stamps every TARGETS edge, clearing the staleness.",
            id = h.id,
            stale = stale_targets.join(", "),
        )
    } else {
        format!(
            "PROVE this hypothesis — is the claimed problem real in the code as it is NOW?\n\
             Read the targeted intents' groundings, check the claim, record what you found:\n\
             \n  loom hypothesis prove {id} --verdict supported --evidence \"<what you found>\" --confidence 0.9\
             \n  loom hypothesis prove {id} --verdict refuted  --evidence \"<why the claim doesn't hold>\" --confidence 0.9\
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
            "mode":             "prove",
            "priority_score":   score,
            "hypothesis":       h,
            "targets":          targets,
            "implements":       implements.iter().map(GroundingSurface::from).collect::<Vec<_>>(),
            "notes":            notes,
            "targets_total":    targets_total,
            "implements_total": implements_total,
            "notes_total":      notes_total,
            "suggested_action": action,
            "owner_role":       "analyzer",
            "effort":           "high",
            "dispatch":         dispatch_line("analyzer"),
            "graph_state":      pulse_json(&gs),
        }));
        return Ok(());
    }

    println!(
        "── Next Prove Item  [{}  priority={:.2}] ─────────────────────────",
        if h.status == "supported" {
            "stale support"
        } else {
            "proposed"
        },
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
        println!(
            "── Targets ({}) ─────────────────────────────────────────────────────",
            targets_total
        );
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
            let loc = if im.locator.is_empty() {
                String::new()
            } else {
                format!("  @ {}", im.locator)
            };
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
            println!(
                "── Notes ({}, showing {}) ─────────────────────────────────────────",
                notes_total,
                notes.len()
            );
        } else {
            println!(
                "── Notes ({}) ──────────────────────────────────────────────────────",
                notes.len()
            );
        }
        for n in &notes {
            if n.times > 1 {
                println!("  [{}] {}  ({}, ×{})", n.kind, n.text, n.author, n.times);
            } else {
                println!("  [{}] {}  ({})", n.kind, n.text, n.author);
            }
        }
        if let Some(m) = more_marker(notes_total, notes.len(), &note_list_intent_command(&h.id)) {
            println!("  {m}");
        }
        println!();
    }
    println!("── Suggested Action ────────────────────────────────────────────────");
    println!("{}", action);
    println!();
    println!("  Dispatch — {}  [effort: high]", dispatch_line("analyzer"));
    println!("  {}", fmt_pulse(&gs));
    Ok(())
}

/// Bound a sub-list rendered inside a work item at SECTION_CAP.
/// Returns the pre-cap total for the caller's marker/`*_total` fields.
fn cap_section<T>(items: &mut Vec<T>) -> usize {
    let total = items.len();
    items.truncate(SECTION_CAP);
    total
}

/// The work-item note pipeline: collapse repeated (kind, text) notes into one
/// surface carrying a count (sync re-flips spam identical transition text —
/// the count IS the information, the copies are not), put notes addressed to
/// `role` first (directed handoffs beat ambient memory; stable within groups,
/// chronological order preserved), cap at SECTION_CAP (addressed notes keep
/// priority; remaining slots go to the NEWEST ambient notes). Returns the
/// surfaces + the pre-cap unique total for the caller's marker/`*_total`.
fn note_surfaces(
    notes: Vec<crate::types::Note>,
    role: &str,
) -> (Vec<crate::types::NoteSurface>, usize) {
    // Dedup: the first occurrence keeps the slot (input is chronological).
    let mut uniq: Vec<(crate::types::Note, u32)> = Vec::new();
    for n in notes {
        match uniq
            .iter_mut()
            .find(|(u, _)| u.kind == n.kind && u.text == n.text)
        {
            Some((_, c)) => *c += 1,
            None => uniq.push((n, 1)),
        }
    }
    let total = uniq.len();
    uniq.sort_by_key(|(n, _)| if n.audience == role { 0 } else { 1 });
    if total > SECTION_CAP {
        let addressed = uniq.iter().take_while(|(n, _)| n.audience == role).count();
        if addressed >= SECTION_CAP {
            uniq.truncate(SECTION_CAP);
        } else {
            uniq.drain(addressed..total - (SECTION_CAP - addressed));
        }
    }
    let surfaces = uniq
        .into_iter()
        .map(|(n, times)| crate::types::NoteSurface {
            kind: n.kind,
            text: n.text,
            author: n.author,
            audience: n.audience,
            times,
        })
        .collect();
    (surfaces, total)
}

#[cfg(test)]
mod tests {
    use super::{build_suggested_action_compact, note_surfaces};

    fn note(kind: &str, text: &str, audience: &str) -> crate::types::Note {
        crate::types::Note {
            id: format!("{kind}:{text}"),
            kind: kind.to_string(),
            text: text.to_string(),
            author: "loom".to_string(),
            target_kind: "edge".to_string(),
            target_id: "e".to_string(),
            audience: audience.to_string(),
            created_at: "t".to_string(),
        }
    }

    #[test]
    fn repeated_notes_collapse_into_a_count() {
        let notes = vec![
            note(
                "transition",
                "passing → needs_reverification (sync: a.rs changed)",
                "",
            ),
            note("transition", "needs_reverification → passing", ""),
            note(
                "transition",
                "passing → needs_reverification (sync: a.rs changed)",
                "",
            ),
            note(
                "transition",
                "passing → needs_reverification (sync: a.rs changed)",
                "",
            ),
        ];
        let (surfaces, total) = note_surfaces(notes, "analyzer");
        assert_eq!(total, 2, "total counts UNIQUE notes");
        assert_eq!(surfaces.len(), 2);
        assert_eq!(surfaces[0].times, 3, "the flap count is the signal");
        assert_eq!(surfaces[1].times, 1);
    }

    #[test]
    fn addressed_notes_survive_the_cap() {
        // 12 ambient notes + 1 directed handoff buried at the end.
        let mut notes: Vec<_> = (0..12)
            .map(|i| note("commentary", &format!("ambient {i}"), ""))
            .collect();
        notes.push(note("decision", "directed handoff", "analyzer"));
        let (surfaces, total) = note_surfaces(notes, "analyzer");
        assert_eq!(total, 13);
        assert_eq!(surfaces.len(), crate::output::SECTION_CAP);
        assert_eq!(
            surfaces[0].text, "directed handoff",
            "addressed-to-role notes surface first"
        );
        assert_eq!(
            surfaces.last().unwrap().text,
            "ambient 11",
            "remaining slots go to the newest ambient notes"
        );
    }

    #[test]
    fn compact_action_is_one_runnable_line() {
        let mut edge = crate::types::RelatesTo {
            id: "rt:a:b".to_string(),
            from_id: "a".to_string(),
            to_id: "b".to_string(),
            from_name: "A".to_string(),
            to_name: "B".to_string(),
            inspection_status: String::new(),
            criterion: String::new(),
            confidence: 0.0,
            evidence: String::new(),
            last_inspected: String::new(),
            inspected_by: String::new(),
            priority_score: 0.0,
            notes: String::new(),
            discovery_class: String::new(),
            discovery_signals: Vec::new(),
            discovery_centrality: Default::default(),
        };
        for status in [
            "unexplored",
            "uninspected",
            "failing",
            "needs_reverification",
        ] {
            edge.inspection_status = status.to_string();
            let action = build_suggested_action_compact(&edge);
            assert!(
                !action.contains('\n'),
                "[{status}] compact action must be one line: {action}"
            );
            assert!(
                action.contains("loom "),
                "[{status}] must carry a runnable command: {action}"
            );
        }
        edge.inspection_status = "failing".to_string();
        assert!(
            build_suggested_action_compact(&edge).contains("rt:a:b"),
            "failing routes through `loom edge fix <id>`"
        );
    }
}
