use super::scoring::{
    add_dispatch, build_suggested_action, build_suggested_action_compact, dispatch_line,
    effort_rank, relates_dispatch,
};
use super::*;
use crate::db::queries::{
    bucket_disclosure_line, capped_discovery_buckets, inject_capped_buckets, CappedBucket,
};

pub(super) fn run_relates_with_repo(
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

    // Dense discovery facets (same-domain / shared-description-token) above
    // BUCKET_CAP are excluded from `suspected_coupling` candidate generation. We
    // disclose that elision in this lane instead of pruning silently — but ONLY
    // here: other modes don't apply the cap, and `--class all`/`impact-map` do
    // the exhaustive walk. Empty otherwise, so the disclosure stays quiet.
    let capped: Vec<CappedBucket> =
        if mode == "discovery" && discovery_class == DiscoveryClassFilter::SuspectedCoupling {
            capped_discovery_buckets(&snapshot)?
        } else {
            Vec::new()
        };

    if candidates.is_empty() {
        let gs = store.graph_state(&snapshot)?;
        // Honesty for the fix lane: `failing`/`needs_reverification` edges that
        // touch a DEPRECATED intent are dropped from the queue (scoring excludes
        // any edge whose endpoint left the active intent set) — there is nothing
        // to re-verify once the intent is gone. But `loom edge list --status
        // needs_reverification` still counts them, so a bald "✓ No … needs_
        // reverification edges" reads as a contradiction to a driver hunting
        // stale evidence. Disclose the excluded count + the real reason.
        let fix_excluded = if mode == "fix" {
            let active: std::collections::HashSet<&str> =
                snapshot.intents.iter().map(|i| i.id.as_str()).collect();
            snapshot
                .relates
                .iter()
                .filter(|e| {
                    matches!(
                        e.inspection_status.as_str(),
                        "failing" | "needs_reverification"
                    )
                })
                .filter(|e| {
                    !(active.contains(e.from_id.as_str()) && active.contains(e.to_id.as_str()))
                })
                .count()
        } else {
            0
        };
        if printer.json {
            let mut v = serde_json::json!({
                "status":  "empty",
                "mode":    mode,
                "discovery_class": discovery_class.as_cli_value(),
                "message": "No work items found for this mode.",
                "excluded_on_inactive_intents": fix_excluded,
                "next_step": gs.next_action,
                "graph_state": pulse_json(&gs),
            });
            if let Some(obj) = v.as_object_mut() {
                inject_capped_buckets(obj, &capped);
            }
            printer.print_json(&v);
        } else {
            match mode {
                // The fast lane is drained, but dense buckets were never
                // enumerated — don't claim "nothing left to discover".
                "discovery" if !capped.is_empty() => {
                    println!("✓ No suspected-coupling candidates left in the fast lane.")
                }
                "discovery" => println!("✓ No uninspected edges — nothing left to discover."),
                "fix" if fix_excluded > 0 => {
                    println!("✓ No ACTIONABLE failing or needs_reverification edges.")
                }
                "fix" => println!("✓ No failing or needs_reverification edges."),
                _ => println!("✓ No work items found."),
            }
            if fix_excluded > 0 {
                println!(
                    "  ⓘ {fix_excluded} failing/stale edge(s) exist but touch deprecated intents (status=deprecated, retired with `loom intent retire`) — excluded from the fix lane (the intent is gone, nothing to re-verify). `loom edge list --status needs_reverification` lists them."
                );
            }
            if let Some(line) = bucket_disclosure_line(&capped) {
                println!("  {line}");
            }
            println!();
            println!("  {}", fmt_pulse(&gs));
            println!("  → Next: {}", gs.next_action);
        }
        return Ok(());
    }

    if take > 0 {
        let gs = store.graph_state(&snapshot)?;
        return run_take(
            store,
            mode,
            &snapshot,
            &candidates,
            take,
            &gs,
            &capped,
            printer,
        );
    }

    let (top_edge, score) = &candidates[0];

    if compact {
        return run_compact(store, mode, top_edge, *score, &capped, printer);
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
    notes.sort_by(|a, b| a.created_at.cmp(&b.created_at));
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
            // Output contract (output.rs): every output carries a runnable
            // `next_step`. The full menu lives in `suggested_action`; this is the
            // single canonical command so driving is field-driven, not parsed.
            // `mode` parity: the human header prints `[mode=… priority=…]`; the
            // JSON envelope must name the lane too — especially now that bare
            // `loom next` follows the compass phase (#6), so a driver can see
            // WHICH lane was served without cross-referencing `loom status`.
            obj.insert("mode".to_string(), mode.into());
            obj.insert(
                "next_step".to_string(),
                build_suggested_action_compact(top_edge).into(),
            );
            obj.insert("graph_state".to_string(), pulse_json(&gs));
            obj.insert("notes_total".to_string(), notes_total.into());
            obj.insert("implements_total".to_string(), implements_total.into());
            obj.insert("validations_total".to_string(), validations_total.into());
            add_dispatch(obj, role, effort);
            inject_capped_buckets(obj, &capped);
        }
        printer.print_json(&v);
        return Ok(());
    }

    render_relates_human(
        mode,
        *score,
        &item,
        top_edge,
        implements_total,
        validations_total,
        notes_total,
        role,
        effort,
        &gs,
        &capped,
    );

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_relates_human(
    mode: &str,
    score: f64,
    item: &WorkItem,
    top_edge: &crate::types::RelatesTo,
    implements_total: usize,
    validations_total: usize,
    notes_total: usize,
    role: &str,
    effort: &str,
    gs: &GraphState,
    capped: &[CappedBucket],
) {
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
    println!("  {}", fmt_pulse(gs));
    if let Some(line) = bucket_disclosure_line(capped) {
        println!("  {line}");
    }
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

#[allow(clippy::too_many_arguments)]
fn run_take(
    db: &dyn GraphReadRepository,
    mode: &str,
    snapshot: &QuerySnapshot,
    candidates: &[(crate::types::RelatesTo, f64)],
    take: usize,
    gs: &crate::db::queries::GraphState,
    capped: &[CappedBucket],
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

    // Close the read/write asymmetry: the bulk WRITE template needs a criterion
    // per pair, so each item must carry the SAME inspection context the single
    // item does (descriptions + sources + groundings) — otherwise the agent
    // scripts a per-pair `loom intent show` loop to fill the blanks. Indexed
    // once from the snapshot the caller already loaded (no per-item DB calls,
    // same O(N) discipline as the note scan above).
    const GROUNDING_CAP: usize = 4;
    let intent_by_id: std::collections::HashMap<&str, &crate::types::Intent> = snapshot
        .intents
        .iter()
        .map(|i| (i.id.as_str(), i))
        .collect();
    let mut groundings_by_intent: std::collections::HashMap<&str, Vec<&crate::types::Implements>> =
        std::collections::HashMap::new();
    for im in &snapshot.implements {
        groundings_by_intent
            .entry(im.intent_id.as_str())
            .or_default()
            .push(im);
    }
    let endpoint = |id: &str, name: &str| -> serde_json::Value {
        let intent = intent_by_id.get(id);
        let groundings: Vec<serde_json::Value> = groundings_by_intent
            .get(id)
            .map(|v| {
                v.iter()
                    .take(GROUNDING_CAP)
                    .map(|im| {
                        serde_json::json!({
                            "path": im.codefile_path,
                            "locator": im.locator,
                            "status": im.inspection_status,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        serde_json::json!({
            "id": id,
            "name": name,
            "description": intent.map(|i| i.description.as_str()).unwrap_or(""),
            "sources": intent.map(|i| i.source_refs.clone()).unwrap_or_default(),
            "groundings": groundings,
        })
    };
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
            // For UNEXPLORED pairs with no semantic signal (impact_map —
            // centrality-only), `independent` is the expected verdict: the
            // pair has no shared imports, vocab, domain, or files. Emit it
            // as the template op so the agent fills WHY they're independent
            // (the anti-laundering gate requires substantive notes), instead
            // of forcing a coexistence criterion for a relationship that
            // likely doesn't exist. Suspected-coupling pairs (with signals)
            // still get `ground` — the signal means a real coupling to inspect.
            let is_unexplored = edge.inspection_status == "unexplored";
            let has_signals = !edge.discovery_signals.is_empty();
            let line = if is_unexplored && !has_signals {
                serde_json::json!({
                    "op": "independent",
                    "a": edge.from_id,
                    "b": edge.to_id,
                    "notes": "<why these intents don't interact — what specific code or domain boundary keeps them apart>",
                })
            } else {
                let mut l = serde_json::json!({
                    "op": "ground",
                    "a": edge.from_id,
                    "b": edge.to_id,
                    "confidence": "<confidence>",
                });
                if edge.criterion.is_empty() {
                    l["criterion"] = "<criterion>".into();
                }
                l
            };
            batch_lines.push(line.to_string());
        }
        let item = serde_json::json!({
            "edge_id": edge.id,
            "a": endpoint(&edge.from_id, &edge.from_name),
            "b": endpoint(&edge.to_id, &edge.to_name),
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

    // Mode-aware: discovery pairs are UNEXPLORED (no recorded criterion — every
    // template line needs a fresh one the gate will accept), and they carry their
    // own context inline, so the re-verification "read the staling file once /
    // batch reuses the recorded criterion" text is actively wrong for them.
    let guidance = if mode == "discovery" {
        "Per item: these are UNEXPLORED pairs (no relationship recorded yet). Each carries both intents' `description`, `sources`, and code `groundings` (path + locator + status) inline above — read those (open the grounded code at the locators if you need more) and judge whether the two actually interact. Then fill EACH `<criterion>` slot with a falsifiable coexistence criterion you wrote from the code — these are NOT re-verifications, so every line needs a real criterion (the gate rejects the `<criterion>` placeholder) — or change `op` to `issue` (+`evidence`) / `independent` (+`notes`). Apply all lines in ONE call: paste them into a heredoc, `loom batch - <<'EOF' … EOF` (no scratch file, nothing to clean up). You have everything here; you should not need a per-pair `loom intent show`."
    } else {
        "Per group: read the staling file ONCE, decide each claim — keep `ground` if it still holds, rewrite to `issue` (+evidence) on breakage, `independent` if there is no relationship — then apply every verdict in ONE call: paste the edited lines into a heredoc, `loom batch - <<'EOF' … EOF` (no scratch file, no repo pollution, nothing to clean up; a file path works for very large batches). Template lines omit `criterion`: `loom batch` reuses the recorded one; write a criterion only to REVISE the claim. Each item carries its own `owner_role`: re-inspection is analyzer work (these are the template lines); items marked `fixer` are FAILING edges — repair the code at root cause first, then re-ground them by hand. A failing edge is deliberately NOT in the ground template (no marking it passing without a fix)."
    };

    if printer.json {
        let mut v = serde_json::json!({
            "status": "ok",
            "mode": mode,
            "taken": n,
            "queue_total": queue_total,
            "groups": groups
                .iter()
                .map(|(f, items)| serde_json::json!({ "staled_by": f, "items": items }))
                .collect::<Vec<_>>(),
            "batch_template": batch_lines,
            "batch_template_hints": BATCH_TEMPLATE_HINTS.to_vec(),
            "guidance": guidance,
            "dispatch": { "role": batch_role, "effort": max_effort },
            "graph_state": pulse_json(gs),
        });
        if let Some(obj) = v.as_object_mut() {
            inject_capped_buckets(obj, capped);
        }
        printer.print_json(&v);
        return Ok(());
    }

    render_take_human(
        mode,
        n,
        queue_total,
        &groups,
        &batch_lines,
        guidance,
        batch_role,
        max_effort,
        gs,
        capped,
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_take_human(
    mode: &str,
    n: usize,
    queue_total: usize,
    groups: &[(String, Vec<serde_json::Value>)],
    batch_lines: &[String],
    guidance: &str,
    batch_role: &str,
    max_effort: &str,
    gs: &GraphState,
    capped: &[CappedBucket],
) {
    println!(
        "── Next: {n} of {queue_total}  [mode={mode}] — bulk read, grouped by staling file ────",
    );
    for (file, items) in groups {
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
    print_batch_template_header();
    for l in batch_lines {
        println!("  {l}");
    }
    println!();
    println!("  {guidance}");
    println!();
    println!("  Dispatch — {batch_role}  [effort: {max_effort}]   (per-item owner_role above is authoritative)");
    println!("  {}", fmt_pulse(gs));
    if let Some(line) = bucket_disclosure_line(capped) {
        println!("  {line}");
    }
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
    capped: &[CappedBucket],
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
        let mut v = serde_json::json!({
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
        });
        if let Some(obj) = v.as_object_mut() {
            inject_capped_buckets(obj, capped);
        }
        printer.print_json(&v);
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
