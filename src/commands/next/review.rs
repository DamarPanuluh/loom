use super::scoring::dispatch_line;
use super::*;

// ---------------------------------------------------------------------------
// Review mode: the strategic double-check for tiered agents — verdicts whose
// recorded confidence is below REVIEW_CONFIDENCE, highest (1−conf)×centrality
// first. A low-capability scout records honest uncertainty; the graph routes
// exactly those claims to a stronger reviewer. Resolves by RE-RECORDING the
// verdict (confirm with confidence ≥ 0.7, or overturn) via the normal write
// paths — no special review write exists, so every gate still applies.
// ---------------------------------------------------------------------------

/// The empty-review-queue line, shared by the interactive `run_review` and the
/// bulk `run_take_review` so both entry points read identically.
fn no_review_needed_message() -> String {
    use crate::db::queries::REVIEW_CONFIDENCE;
    format!("No verdicts below confidence {REVIEW_CONFIDENCE} — nothing needs a second look.")
}

pub(super) fn run_review(store: &dyn GraphReadRepository, printer: &Printer) -> Result<()> {
    use crate::db::queries::ReviewCandidate;

    let snapshot = store.query_snapshot()?;
    let candidates = review_candidates_from_snapshot(&snapshot);
    let gs = store.graph_state(&snapshot)?;

    if candidates.is_empty() {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "empty", "mode": "review",
                "message": no_review_needed_message(),
                "next_step": gs.next_action,
                "graph_state": pulse_json(&gs),
            }));
        } else {
            println!("✓ {}", no_review_needed_message());
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
// Review mode, BULK: the same low-confidence queue as run_review, served as a
// compact batch with a re-record template — so a flood of flagged verdicts (the
// honesty pass routes copied-evidence / uniform-confidence clusters here) drains
// in chunks instead of one CLI call each (the offline-mega-batch anti-pattern).
// ---------------------------------------------------------------------------

pub(super) fn run_take_review(
    store: &dyn GraphReadRepository,
    take: usize,
    printer: &Printer,
) -> Result<()> {
    use crate::db::queries::ReviewCandidate;
    // BOUNDED like the other --take queues — a high-tier review chunk stays
    // reviewable in one sitting.
    const TAKE_CAP: usize = 50;

    let snapshot = store.query_snapshot()?;
    let candidates = review_candidates_from_snapshot(&snapshot);
    let gs = store.graph_state(&snapshot)?;
    let queue_total = candidates.len();

    if candidates.is_empty() {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "empty", "mode": "review", "taken": 0, "queue_total": 0,
                "message": no_review_needed_message(),
                "next_step": gs.next_action,
                "graph_state": pulse_json(&gs),
            }));
        } else {
            println!("✓ {}", no_review_needed_message());
            println!("  {}", fmt_pulse(&gs));
        }
        return Ok(());
    }

    let n = take.min(TAKE_CAP).min(candidates.len());
    let mut items: Vec<serde_json::Value> = Vec::new();
    let mut batch_lines: Vec<String> = Vec::new();
    let mut human_lines: Vec<String> = Vec::new();
    let mut has_analyzer = false;
    let mut has_quality = false;
    for (cand, score) in candidates.iter().take(n) {
        match cand {
            ReviewCandidate::RelatesTo(e) => {
                has_analyzer = true;
                // Re-affirm reuses the stored criterion (it passed the gate); the
                // reviewer edits to issue/independent to overturn.
                batch_lines.push(
                    serde_json::json!({"op": "ground", "a": e.from_id, "b": e.to_id, "confidence": "<confidence>"})
                        .to_string(),
                );
                human_lines.push(format!(
                    "    [relates_to conf={:>4.2} analyzer]  {} × {}  ({})",
                    e.confidence, e.from_name, e.to_name, e.id
                ));
                items.push(serde_json::json!({
                    "kind": "relates_to", "edge_id": e.id,
                    "a": {"id": e.from_id, "name": e.from_name},
                    "b": {"id": e.to_id, "name": e.to_name},
                    "recorded_status": e.inspection_status, "recorded_confidence": e.confidence,
                    "inspected_by": e.inspected_by, "criterion": e.criterion,
                    "priority_score": score, "owner_role": "analyzer", "effort": "high",
                }));
            }
            ReviewCandidate::Governs(g) => {
                has_quality = true;
                // rule_verdict needs evidence — the placeholder is rejected
                // unedited, forcing the reviewer to record what they found.
                batch_lines.push(
                    serde_json::json!({"op": "rule_verdict", "rule": g.rule_id, "intent": g.intent_id,
                        "status": g.inspection_status, "evidence": "<what the re-inspection found>", "confidence": "<confidence>"})
                        .to_string(),
                );
                human_lines.push(format!(
                    "    [governs    conf={:>4.2} quality ]  {} → {}  ({})",
                    g.confidence, g.rule_name, g.intent_name, g.id
                ));
                items.push(serde_json::json!({
                    "kind": "governs", "edge_id": g.id,
                    "rule": {"id": g.rule_id, "name": g.rule_name},
                    "intent": {"id": g.intent_id, "name": g.intent_name},
                    "recorded_status": g.inspection_status, "recorded_confidence": g.confidence,
                    "inspected_by": g.inspected_by, "criterion": g.criterion,
                    "priority_score": score, "owner_role": "quality", "effort": "high",
                }));
            }
        }
    }
    let role = match (has_analyzer, has_quality) {
        (true, true) => "mixed",
        (false, true) => "quality",
        _ => "analyzer",
    };
    let guidance = "INDEPENDENT RE-INSPECTION, per item: form your OWN hypothesis from the intents and code BEFORE reading the recorded evidence (anchoring on a low-confidence claim defeats the review). Then re-record via the template line — confirm with YOUR confidence (≥ 0.7 resolves it) or overturn (rewrite relates_to to `issue`/`independent`, governs to `failing`/`independent`). relates_to lines reuse the stored criterion; governs lines need real evidence (the placeholder is rejected unedited). Apply the edited lines in ONE call: `loom batch - <<'EOF' … EOF`.";

    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok", "mode": "review", "taken": n, "queue_total": queue_total,
            "items": items, "batch_template": batch_lines,
            "batch_template_hints": BATCH_TEMPLATE_HINTS.to_vec(),
            "guidance": guidance,
            "dispatch": {"role": role, "effort": "high"},
            "graph_state": pulse_json(&gs),
        }));
        return Ok(());
    }

    println!("── Review: {n} of {queue_total} low-confidence verdict(s) — bulk re-inspection ──");
    println!();
    for line in &human_lines {
        println!("{line}");
    }
    println!();
    print_batch_template_header();
    for l in &batch_lines {
        println!("  {l}");
    }
    println!();
    println!("  {guidance}");
    println!("  Dispatch — {role}  [effort: high]   (per-item owner_role above is authoritative)");
    println!("  {}", fmt_pulse(&gs));
    Ok(())
}
