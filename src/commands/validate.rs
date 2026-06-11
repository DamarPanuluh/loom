use anyhow::Result;
use std::process::Command as StdCommand;

use crate::db::{ensure_initialized, GrafeoDb, LoomDb};
use crate::db::queries::{
    get_intent, get_validation, list_validates_for_intent,
    update_validation_result,
};
use crate::db::schema::esc;
use crate::output::Printer;
use crate::types::ValidationResult;

pub fn run(intent_id: &str, printer: &Printer) -> Result<()> {
    // Running validations writes last_run/last_result and the VALIDATES
    // verdict — validator lane.
    crate::gate::acting_in_lane(
        "run validations",
        &[crate::db::schema::role::VALIDATOR],
        None,
    )?;
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;
    let intent_id = &crate::db::queries::resolve_intent(&db, intent_id)?;

    // Ensure intent exists
    get_intent(&db, intent_id)?
        .ok_or_else(|| anyhow::anyhow!(
            "Intent '{}' not found.\nRun `loom intent list` to see available intents.",
            intent_id
        ))?;

    // Get all VALIDATES edges for this intent
    let validates_edges = list_validates_for_intent(&db, intent_id)?;

    if validates_edges.is_empty() {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "intent_id": intent_id,
                "results":   [],
                "message":   "No validations linked to this intent.",
                "next_step": format!(
                    "Add one: `loom validation add --name \"...\" --type test --command \"cargo test ...\"`, \
                     then link it: `loom edge validates <validation-id> {}`",
                    intent_id
                ),
            }));
        } else {
            println!("No validation nodes linked to intent '{}'.", intent_id);
            println!("  → Add one:  loom validation add --name \"...\" --type test --command \"cargo test ...\"");
            println!("  → Link it:  loom edge validates <validation-id> {}", intent_id);
        }
        return Ok(());
    }

    // Phase 1 (DB open): resolve every Validation node up front.
    let mut to_run: Vec<crate::types::Validation> = Vec::new();
    for ve in &validates_edges {
        match get_validation(&db, &ve.validation_id)? {
            Some(v) => to_run.push(v),
            None => eprintln!(
                "Warning: Validation node '{}' not found in DB — skipping.",
                ve.validation_id
            ),
        }
    }

    // Phase 2 (DB CLOSED): run the commands with the graph lock released.
    // loom holds one exclusive grafeo session; a validation command may itself
    // invoke loom (e.g. `loom status --json` as a smoke check) or anything else
    // that reads the graph — holding the lock here would deadlock it with
    // GRAFEO-X001. Found by loom validating itself.
    drop(db);

    let now = chrono::Utc::now().to_rfc3339();
    let mut results: Vec<serde_json::Value> = Vec::new();
    let mut outcomes: Vec<(String, String)> = Vec::new(); // (validation_id, result)
    let mut blocked_notes: Vec<(String, String)> = Vec::new(); // (validation_id, edge note)
    let mut passed = 0usize;
    let mut failed = 0usize;

    for validation in &to_run {
        if validation.last_result == "blocked" {
            // A recorded "can't run yet" — don't run it, don't overwrite it.
            // Unblock by re-marking: `loom validation mark <id> --result passed|failed`.
            results.push(serde_json::json!({
                "validation_id": validation.id,
                "name":          validation.name,
                "result":        "blocked",
                "reason":        "marked blocked — see its VALIDATES edge notes for why",
            }));
            if !printer.json {
                println!("  ⊘ {} [blocked — re-mark with `loom validation mark` when unblocked]", validation.name);
            }
            continue;
        }
        if validation.command.is_empty() {
            results.push(serde_json::json!({
                "validation_id": validation.id,
                "name":          validation.name,
                "result":        "skipped",
                "reason":        "no command defined — record by hand: `loom validation mark`",
            }));
            if !printer.json {
                println!("  - {} [skipped — no command; record by hand: `loom validation mark {}`]",
                    validation.name, validation.id);
            }
            continue;
        }

        // A saga consumes a LIVE target via `{{ env.X }}` values passed at
        // invocation. If they're missing here, the proof CANNOT run — that is
        // `blocked` (environment not ready), not `failed` (code wrong). Running
        // the command anyway would record a dishonest failure and send the
        // driver chasing a phantom code bug.
        if validation.validation_type == "saga" {
            if let Some(missing) = saga_missing_env(&cwd, validation) {
                let invocation: String = missing
                    .iter()
                    .map(|v| format!("{v}=<value> "))
                    .chain([format!("loom saga run {}", validation.name)])
                    .collect();
                let reason = format!(
                    "missing env value(s): {} — run `{}` (or re-mark when the target is available)",
                    missing.join(", "), invocation
                );
                outcomes.push((validation.id.clone(), "blocked".to_string()));
                blocked_notes.push((validation.id.clone(), format!("blocked: {reason}")));
                results.push(serde_json::json!({
                    "validation_id": validation.id,
                    "name":          validation.name,
                    "result":        "blocked",
                    "reason":        reason,
                }));
                if !printer.json {
                    println!("  ⊘ {} [blocked — {}]", validation.name, reason);
                }
                continue;
            }
        }

        // Run the command via sh -c so shell features work (e.g. cargo test --test foo)
        let exit_status = StdCommand::new("sh")
            .arg("-c")
            .arg(&validation.command)
            .status();

        let result = match exit_status {
            Ok(s) if s.success() => {
                passed += 1;
                ValidationResult::Passed
            }
            Ok(_) => {
                failed += 1;
                ValidationResult::Failed
            }
            Err(e) => {
                failed += 1;
                eprintln!(
                    "Warning: Could not run command for '{}': {}",
                    validation.name, e
                );
                ValidationResult::Failed
            }
        };
        let new_result = result.to_string();
        outcomes.push((validation.id.clone(), new_result.clone()));

        results.push(serde_json::json!({
            "validation_id": validation.id,
            "name":          validation.name,
            "type":          validation.validation_type,
            "command":       validation.command,
            "result":        &new_result,
            "run_at":        &now,
        }));

        if !printer.json {
            let mark = if new_result == "passed" { "✓" } else { "✗" };
            println!("  {} {} [{}]", mark, validation.name, new_result);
            println!("    cmd: {}", validation.command);
        }
    }

    // Phase 3 (DB reopened): persist results on the Validation nodes and the
    // VALIDATES edges.
    let db = GrafeoDb::open(&db_file)?;
    for (vid, new_result) in &outcomes {
        update_validation_result(&db, vid, new_result, &now)?;
        set_validates_edge_status(&db, vid, new_result)?;
    }
    // Blocked proofs also record WHY on their VALIDATES edges (mirrors
    // `loom validation mark --result blocked`): out of the queue, never
    // looking forgotten, and a code change won't quietly reset them.
    for (vid, note) in &blocked_notes {
        crate::db::queries::set_validates_status_for_validation(&db, vid, "uninspected", note)?;
    }

    // End-of-run summary moves the phase: full anchor, result-sensitive.
    let next_step = if failed > 0 {
        "`loom next --mode fix`"
    } else {
        "`loom status` re-checks the compass"
    };
    if printer.json {
        printer.print_json(&crate::output::with_anchor(serde_json::json!({
            "intent_id": intent_id,
            "passed":    passed,
            "failed":    failed,
            "results":   results,
        }), &db, next_step)?);
    } else {
        println!();
        println!("  Summary: {}/{} passed", passed, passed + failed);
        crate::output::print_anchor(&db, next_step)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Private: saga env pre-flight
// ---------------------------------------------------------------------------

/// For a saga validation: the env vars its spec needs that this process
/// doesn't have, or None when there's nothing missing / the spec can't be
/// read (then the command runs and fails loudly on its own).
fn saga_missing_env(
    root: &std::path::Path,
    v: &crate::types::Validation,
) -> Option<Vec<String>> {
    let rel = crate::commands::saga::spec_path_of(v)?;
    let spec = crate::saga::spec::load_spec_file(&root.join(rel)).ok()?;
    let missing = crate::saga::spec::missing_env(&spec);
    if missing.is_empty() { None } else { Some(missing) }
}

// ---------------------------------------------------------------------------
// Private: map validation result → VALIDATES edge inspection_status
// ---------------------------------------------------------------------------

fn set_validates_edge_status(
    db: &GrafeoDb,
    validation_id: &str,
    validation_result: &str,
) -> Result<()> {
    let new_status = match validation_result {
        "passed"  => "passing",
        "failed"  => "failing",
        _         => "uninspected",
    };
    // The proof run belongs to the VALIDATION, not to the (validation, intent)
    // pair: one command ran once, and its result proves (or fails) every intent
    // the validation has a VALIDATES edge to. Updating only the invoked
    // intent's edge left sibling edges `uninspected` forever — the validator
    // queue (keyed on last_result) went quiet while the compass (keyed on edge
    // states) kept saying phase=validate. Node-anchored match — the
    // validation's id is the edge family's key (schema v4 derived identity).
    db.execute(&format!(
        "MATCH (v:Validation {{id: '{vid}'}})-[e:VALIDATES]->(:Intent) \
         SET e.inspection_status = '{status}'",
        vid = esc(validation_id), status = new_status
    ))?;
    Ok(())
}
