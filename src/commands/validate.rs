use anyhow::Result;
use std::env;
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
    let cwd = env::current_dir()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

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
    let mut passed = 0usize;
    let mut failed = 0usize;

    for validation in &to_run {
        if validation.command.is_empty() {
            results.push(serde_json::json!({
                "validation_id": validation.id,
                "name":          validation.name,
                "result":        "skipped",
                "reason":        "no command defined",
            }));
            if !printer.json {
                println!("  - {} [skipped — no command defined]", validation.name);
            }
            continue;
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
        set_validates_edge_status(&db, vid, intent_id, new_result)?;
    }

    if printer.json {
        printer.print_json(&serde_json::json!({
            "intent_id": intent_id,
            "passed":    passed,
            "failed":    failed,
            "results":   results,
        }));
    } else {
        println!();
        println!("  Summary: {}/{} passed", passed, passed + failed);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Private: map validation result → VALIDATES edge inspection_status
// ---------------------------------------------------------------------------

fn set_validates_edge_status(
    db: &GrafeoDb,
    validation_id: &str,
    intent_id: &str,
    validation_result: &str,
) -> Result<()> {
    let new_status = match validation_result {
        "passed"  => "passing",
        "failed"  => "failing",
        _         => "uninspected",
    };
    // Identify the VALIDATES edge by its endpoints (validation → intent), which
    // is reliable; matching a relationship by its own id property is not in
    // grafeo 0.5.x.
    db.execute(&format!(
        "MATCH (v:Validation {{id: '{vid}'}})-[e:VALIDATES]->(i:Intent {{id: '{iid}'}}) \
         SET e.inspection_status = '{status}'",
        vid = esc(validation_id), iid = esc(intent_id), status = new_status
    ))?;
    Ok(())
}
