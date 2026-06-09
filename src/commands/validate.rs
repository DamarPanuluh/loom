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

    let now = chrono::Utc::now().to_rfc3339();
    let mut results: Vec<serde_json::Value> = Vec::new();
    let mut passed = 0usize;
    let mut failed = 0usize;

    for ve in &validates_edges {
        let validation = match get_validation(&db, &ve.validation_id)? {
            Some(v) => v,
            None => {
                eprintln!(
                    "Warning: Validation node '{}' not found in DB — skipping.",
                    ve.validation_id
                );
                continue;
            }
        };

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

        // Persist result on the Validation node
        update_validation_result(&db, &validation.id, &new_result, &now)?;

        // Update the VALIDATES edge inspection_status to reflect the outcome
        set_validates_edge_status(&db, &ve.validation_id, intent_id, &new_result)?;

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
