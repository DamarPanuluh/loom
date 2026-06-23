use super::scoring::dispatch_line;
use super::*;

pub(super) fn run_take_quality(
    store: &dyn GraphReadRepository,
    take: usize,
    kind: Option<&str>,
    printer: &Printer,
) -> Result<()> {
    let snapshot = store.query_snapshot()?;
    let mut candidates = quality_candidates_from_snapshot(&snapshot);
    let gs = store.graph_state(&snapshot)?;
    let filtered_kind = kind;
    if let Some(k) = filtered_kind {
        let rule_kinds: std::collections::HashMap<&str, &str> = snapshot
            .rules
            .iter()
            .map(|r| (r.id.as_str(), r.kind.as_str()))
            .collect();
        candidates.retain(|(g, _)| rule_kinds.get(g.rule_id.as_str()).copied() == Some(k));
    }

    if candidates.is_empty() {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "empty", "mode": "quality",
                "filtered_kind": kind,
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
            "confidence": "<confidence>",
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
            "filtered_kind": kind,
            "groups": groups
                .iter()
                .map(|(iid, iname, items)| serde_json::json!({
                    "intent": { "id": iid, "name": iname },
                    "items": items,
                }))
                .collect::<Vec<_>>(),
            "batch_template": batch_lines,
            "batch_template_hints": BATCH_TEMPLATE_HINTS.to_vec(),
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
    print_batch_template_header();
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
// Quality mode: the quality agent's queue — GOVERNS edges whose green is unearned
// ---------------------------------------------------------------------------

pub(super) fn run_quality(
    store: &dyn GraphReadRepository,
    kind: Option<&str>,
    printer: &Printer,
) -> Result<()> {
    let snapshot = store.query_snapshot()?;
    let mut candidates = quality_candidates_from_snapshot(&snapshot);
    let gs = store.graph_state(&snapshot)?;
    if let Some(k) = kind {
        let rule_kinds: std::collections::HashMap<&str, &str> = snapshot
            .rules
            .iter()
            .map(|r| (r.id.as_str(), r.kind.as_str()))
            .collect();
        candidates.retain(|(g, _)| rule_kinds.get(g.rule_id.as_str()).copied() == Some(k));
    }
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
            "filtered_kind":     kind,
            "priority_score":   score,
            "governs":          g,
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
