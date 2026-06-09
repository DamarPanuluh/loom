use anyhow::Result;
use std::env;

use crate::db::{ensure_initialized, GrafeoDb};
use crate::db::queries::{
    build_candidates, get_intent, graph_state, list_implements_for_intent, notes_for_target,
    quality_candidates, scored_candidates, unexplored_pairs_scored, validate_candidates,
    validations_for_intent,
};
use crate::output::{fmt_edge_detail, fmt_intent, fmt_pulse, Printer};
use crate::types::{CodeFile, EdgeType, WorkItem};

pub fn run(mode: &str, printer: &Printer) -> Result<()> {
    if !matches!(mode, "discovery" | "fix" | "build" | "validate" | "quality") {
        anyhow::bail!(
            "Unknown mode '{}'. Valid values: discovery, fix, build, validate, quality\n\
             discovery = inspect relationships (analyzer) · fix = resolve failures/stale (fixer) · \
             build = realize planned/needs_change intents (builder) · \
             validate = run/repair proofs (validator) · quality = earn GOVERNS green (quality).",
            mode
        );
    }

    let cwd = env::current_dir()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    match mode {
        "build" => return run_build(&db, printer),
        "validate" => return run_validate(&db, printer),
        "quality" => return run_quality(&db, printer),
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

    let (top_edge, score) = &candidates[0];

    // Fetch rich context for both intents
    let intent_a = get_intent(&db, &top_edge.from_id)?
        .ok_or_else(|| anyhow::anyhow!("Intent '{}' not found in DB", top_edge.from_id))?;
    let intent_b_opt = get_intent(&db, &top_edge.to_id)?;

    // Fetch code files related to intent_a (via IMPLEMENTS)
    let implements_a = list_implements_for_intent(&db, &top_edge.from_id)?;
    let code_files: Vec<CodeFile> = implements_a
        .iter()
        .map(|imp| CodeFile {
            id:            imp.codefile_id.clone(),
            path:          imp.codefile_path.clone(),
            language:      String::new(), // path is the primary identifier
            last_modified: String::new(),
            imports:       String::new(),
        })
        .collect();

    // Fetch validations for intent_a (via VALIDATES)
    let validations = validations_for_intent(&db, &top_edge.from_id)?;

    // Gather accumulated memory: notes on the edge (if it exists yet) and on
    // both intents, so prior reasoning travels with the work item.
    let mut notes = Vec::new();
    if !top_edge.id.is_empty() {
        notes.extend(notes_for_target(&db, &top_edge.id)?);
    }
    notes.extend(notes_for_target(&db, &top_edge.from_id)?);
    notes.extend(notes_for_target(&db, &top_edge.to_id)?);

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
        println!();
    }

    // Validations
    if !validations.is_empty() {
        println!("── Validations on Intent A ─────────────────────────────────────────");
        for v in &validations {
            let result_mark = match v.last_result.as_str() {
                "passed"  => "✓",
                "failed"  => "✗",
                _         => "?",
            };
            println!(
                "  {} {}  [{}]  cmd: {}",
                result_mark, v.name, v.last_result, v.command
            );
        }
        println!();
    }

    // Accumulated memory
    if !notes.is_empty() {
        println!("── Notes ({}) ──────────────────────────────────────────────────────", notes.len());
        for n in &notes {
            println!("  [{}] {}  ({})", n.kind, n.text, n.author);
        }
        println!();
    }

    // Suggested action
    println!("── Suggested Action ────────────────────────────────────────────────");
    println!("{}", suggested_action);
    println!();
    println!("  {}", fmt_pulse(&gs));

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
            "Fix the violation on this edge then mark it passing:\n\
             \n  loom edge fix {} --description \"<what you changed>\"",
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
// Build mode: realize `planned` / `needs_change` intents (greenfield/refactor)
// ---------------------------------------------------------------------------

fn run_build(db: &GrafeoDb, printer: &Printer) -> Result<()> {
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
    let implements = list_implements_for_intent(db, &intent.id)?;
    let validations = validations_for_intent(db, &intent.id)?;
    let notes = notes_for_target(db, &intent.id)?;
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
        println!();
    }
    if !notes.is_empty() {
        println!("── Notes ({}) ──────────────────────────────────────────────────────", notes.len());
        for n in &notes {
            println!("  [{}] {}  ({})", n.kind, n.text, n.author);
        }
        println!();
    }
    println!("── Suggested Action ────────────────────────────────────────────────");
    println!("{}", action);
    println!();
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
    let validations = validations_for_intent(db, &c.intent.id)?;
    let notes = notes_for_target(db, &c.intent.id)?;
    let action = if validations.is_empty() {
        format!(
            "PROVE this intent — it has no validations:\n\
             1. Decide how it can be proven (test | assertion | benchmark | manual_check).\n  \
             2. loom validation add --name \"…\" --type test --command \"…\" --intent {id}\n  \
             3. loom validate {id}",
            id = c.intent.id,
        )
    } else {
        format!(
            "Run this intent's validations and record the verdicts:\n\
             \n  loom validate {id}\n\
             \nIf one fails, the intent is not fulfilled — flag it: \
             loom intent mark {id} --lifecycle needs_change --reason \"<validation failure>\"",
            id = c.intent.id,
        )
    };

    if printer.json {
        printer.print_json(&serde_json::json!({
            "mode":             "validate",
            "reason":           c.reason,
            "priority_score":   c.score,
            "intent":           c.intent,
            "validations":      validations,
            "notes":            notes,
            "suggested_action": action,
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
                _ => "?",
            };
            println!("  {} {}  [{}]  cmd: {}", mark, v.name, v.last_result, v.command);
        }
        println!();
    }
    println!("── Suggested Action ────────────────────────────────────────────────");
    println!("{}", action);
    println!();
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
    let implements = list_implements_for_intent(db, &g.intent_id)?;
    let notes = notes_for_target(db, &g.id)?;
    let action = format!(
        "Inspect intent '{name}' against rule '{rule}' and record the verdict:\n\
         \n  loom rule verdict {rid} {iid} --status passing --criterion \"<what compliance looks like>\" --evidence \"<what you found>\"\
         \n  loom rule verdict {rid} {iid} --status failing --criterion \"<criterion>\" --evidence \"<the violation>\"",
        name = g.intent_name,
        rule = g.rule_name,
        rid = g.rule_id,
        iid = g.intent_id,
    );

    if printer.json {
        printer.print_json(&serde_json::json!({
            "mode":             "quality",
            "priority_score":   score,
            "governs":          g,
            "intent":           intent,
            "implements":       implements,
            "notes":            notes,
            "suggested_action": action,
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
        println!();
    }
    println!("── Suggested Action ────────────────────────────────────────────────");
    println!("{}", action);
    println!();
    println!("  {}", fmt_pulse(&gs));
    Ok(())
}

fn build_action(intent: &crate::types::Intent, rollup: bool) -> String {
    match intent.lifecycle.as_str() {
        // A planned parent whose children are all implemented: the work is
        // verification + roll-up, never writing code at this altitude.
        "planned" if rollup => format!(
            "ROLL UP this intent — all its children are implemented; nothing is built \
             at this altitude directly.\n\
             1. Check each child fulfils its criterion (`loom intent show {id}` lists children).\n  \
             2. If satisfied: loom intent mark {id} --lifecycle implemented\n  \
             3. If a child falls short: loom intent mark <child-id> --lifecycle needs_change --reason \"…\"",
            id = intent.id,
        ),
        "planned" => format!(
            "BUILD this intent — its description/criteria are the spec/acceptance check.\n\
             1. Write the code.\n  \
             2. Register it: loom codefile add <path>\n  \
             3. Ground it: loom edge implement {id} <codefile> --locator \"<symbol>\"\n  \
             4. Mark done: loom intent mark {id} --lifecycle implemented",
            id = intent.id,
        ),
        "needs_change" => format!(
            "CHANGE the code for this intent (the description/criteria + notes describe the desired end state).\n\
             1. Make the minimal change.\n  \
             2. Mark done: loom intent mark {id} --lifecycle implemented\n  \
             3. Re-verify affected relationships: loom next --mode fix",
            id = intent.id,
        ),
        other => format!("Intent '{}' has lifecycle '{}' — review it.", intent.name, other),
    }
}
