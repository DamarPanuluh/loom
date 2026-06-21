use super::scoring::{add_dispatch, dispatch_line};
use super::*;

// ---------------------------------------------------------------------------
// Build mode: realize `planned` / `needs_change` intents (greenfield/refactor)
// ---------------------------------------------------------------------------

pub(super) fn run_build(
    db: &dyn GraphReadRepository,
    take_note: Option<&str>,
    printer: &Printer,
) -> Result<()> {
    db.ensure_owned("work the build queue (there is nothing to build in someone else's repo)")?;
    // ONE snapshot feeds both the queue and the pulse (production uses the
    // same snapshot scoring as the compass — coherence by construction — and
    // avoids a second full graph load).
    let snapshot = db.query_snapshot()?;
    let candidates = build_candidates_from_snapshot(&snapshot);
    let gs = db.graph_state(&snapshot)?;

    if candidates.is_empty() {
        if printer.json {
            printer.print_json(&inject_take_note(
                serde_json::json!({
                    "status": "empty", "mode": "build",
                    "message": "No planned or needs_change intents — nothing to build.",
                    "next_step": gs.next_action,
                    "graph_state": pulse_json(&gs),
                }),
                take_note,
            ));
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
    let (role, effort) =
        if intent.lifecycle == "needs_change" || intent.lifecycle == "to_be_removed" {
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
            // Output contract: every output carries a runnable `next_step`.
            obj.insert("next_step".to_string(), action.clone().into());
            obj.insert("graph_state".to_string(), pulse_json(&gs));
            obj.insert("notes_total".to_string(), notes_total.into());
            obj.insert("implements_total".to_string(), implements_total.into());
            obj.insert("validations_total".to_string(), validations_total.into());
            add_dispatch(obj, role, effort);
        }
        printer.print_json(&inject_take_note(v, take_note));
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

pub(super) fn run_validate(
    db: &dyn GraphReadRepository,
    take_note: Option<&str>,
    printer: &Printer,
) -> Result<()> {
    // ONE snapshot for both the queue and the pulse (shares the compass's
    // validate_selection scoring; no second full graph load).
    let snapshot = db.query_snapshot()?;
    let candidates = validate_candidates_from_snapshot(&snapshot);
    let gs = db.graph_state(&snapshot)?;

    if candidates.is_empty() {
        if printer.json {
            printer.print_json(&inject_take_note(
                serde_json::json!({
                    "status": "empty", "mode": "validate",
                    "message": "Every intent's proof is green — nothing to validate.",
                    "next_step": gs.next_action,
                    "graph_state": pulse_json(&gs),
                }),
                take_note,
            ));
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
        printer.print_json(&inject_take_note(serde_json::json!({
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
        }), take_note));
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
            "BUILD this intent — its description + criterion ARE the spec and the acceptance check.\n\
             1. Write the code.\n  \
             2. Register it: loom codefile add <path>\n  \
             3. Ground it: loom edge implement {id} <codefile> --locator \"<symbol>\"\n     \
                (the locator is verified against the file NOW — a typo'd or renamed symbol is rejected here, not silently staled at the next sync).\n  \
             4. PROVE the criterion — don't just assert it. Encode the criterion as a check and run it:\n       \
                loom validation add --name \"<criterion, as a check>\" --type test --command \"<cmd>\" --intent {id}\n       \
                loom validate {id}\n     \
                (no runnable proof? record the manual verdict: loom validation mark <id> --result passed --evidence \"…\"; an endpoint-reachable criterion proves best as a consumer saga: loom saga add <spec.yaml>).\n  \
             5. Mark done: loom intent mark {id} --lifecycle implemented   (a leaf marked implemented with NO validation is flagged implemented-but-unproven).\n  \
             6. Baseline it: loom sync   (stamps the new files; future edits ripple correctly).\n  \
             Noticed a relationship while coding (this calls or depends on another intent)? Capture it now so discovery need not re-find it: loom note add --intent {id} --for analyzer --text \"relates to <other intent>: <how>\".",
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
        "to_be_removed" => format!(
            "REMOVE the code for this intent — cleanup is done by ABSENCE (its criterion is \"this is gone\").
\
             1. Delete the code it grounds (`loom intent show {id}` / `loom explain {id}` list the files/symbols).
  \
             2. Unground each file: loom edge unimplement {id} <codefile>
  \
             3. Drop now-dead files from the graph: loom codefile remove <path>
  \
             4. Flag the ripple: loom sync   (stales claims the deletion touched)
  \
             5. When no grounding remains, this intent reads done; retire it: loom intent retire {id} --reason \"removed\"",
            id = intent.id,
        ),
        other => format!("Intent '{}' has lifecycle '{}' — review it.", intent.name, other),
    }
}

// ---------------------------------------------------------------------------
// Prove mode: the pre-decision plane's queue — proposed hypotheses awaiting
// their proof, highest target-centrality (blast radius) first. Analyzer work;
// optional like discovery/review (speculation never blocks complete).
// ---------------------------------------------------------------------------

pub(super) fn run_prove(
    store: &dyn GraphReadRepository,
    take_note: Option<&str>,
    printer: &Printer,
) -> Result<()> {
    let snapshot = store.query_snapshot()?;
    let candidates = store.prove_candidates(&snapshot)?;
    let gs = store.graph_state(&snapshot)?;

    if candidates.is_empty() {
        if printer.json {
            printer.print_json(&inject_take_note(serde_json::json!({
                "status": "empty", "mode": "prove",
                "message": "No proposed hypotheses and no stale support — the pre-decision plane is clear.",
                "next_step": gs.next_action,
                "graph_state": pulse_json(&gs),
            }), take_note));
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
        printer.print_json(&inject_take_note(serde_json::json!({
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
        }), take_note));
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
