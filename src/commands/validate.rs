use anyhow::Result;
use std::process::Command as StdCommand;
use std::thread;
use std::time::{Duration, Instant};

use crate::db::{ensure_initialized, GrafeoDb, LoomDb};
use crate::db::queries::{
    get_intent, get_validation, list_validates_for_intent,
    update_validation_result,
};
use crate::db::schema::esc;
use crate::output::Printer;
use crate::types::ValidationResult;

pub fn run(intent_id: &str, timeout_secs: u64, printer: &Printer) -> Result<()> {
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

    execute_and_record(
        &db_file, &cwd, db, &to_run, timeout_secs, printer,
        ("intent_id", serde_json::json!(intent_id)),
    )
}

/// `loom validate --all`: run every PENDING proof — last_result == not_run,
/// i.e. never run or invalidated by a sync flood. One verb instead of
/// enumerating intents by hand after `loom sync` resets N proofs at once.
/// Passed/failed results are settled verdicts (re-run them per intent when you
/// mean to); blocked proofs carry a recorded reason and stay out everywhere.
pub fn run_all(timeout_secs: u64, printer: &Printer) -> Result<()> {
    crate::gate::acting_in_lane(
        "run validations",
        &[crate::db::schema::role::VALIDATOR],
        None,
    )?;
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    let to_run: Vec<crate::types::Validation> = crate::db::queries::list_validations(&db)?
        .into_iter()
        .filter(|v| v.last_result == "not_run")
        .collect();

    if to_run.is_empty() {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "scope":   "all",
                "results": [],
                "message": "Nothing pending — every proof has a recorded result (passed/failed/blocked).",
            }));
        } else {
            println!("✓ Nothing pending — every proof has a recorded result (passed/failed/blocked).");
        }
        return Ok(());
    }
    if !printer.json {
        println!("Running {} pending validation(s)…", to_run.len());
    }

    execute_and_record(
        &db_file, &cwd, db, &to_run, timeout_secs, printer,
        ("scope", serde_json::json!("all")),
    )
}

/// Phases 2+3 shared by `run` and `run_all`: execute commands with the DB
/// CLOSED (the graph lock must be released — a validation may itself invoke
/// loom; found by loom validating itself), then reopen and persist results +
/// VALIDATES verdicts in one transaction. `scope` is the JSON envelope key
/// identifying what was run (intent_id vs all).
fn execute_and_record(
    db_file: &std::path::Path,
    cwd: &std::path::Path,
    db: GrafeoDb,
    to_run: &[crate::types::Validation],
    timeout_secs: u64,
    printer: &Printer,
    scope: (&str, serde_json::Value),
) -> Result<()> {
    drop(db);

    let now = chrono::Utc::now().to_rfc3339();
    let mut results: Vec<serde_json::Value> = Vec::new();
    let mut outcomes: Vec<(String, String)> = Vec::new(); // (validation_id, result)
    let mut blocked_notes: Vec<(String, String)> = Vec::new(); // (validation_id, edge note)
    let mut passed = 0usize;
    let mut failed = 0usize;

    for validation in to_run {
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
        let exit_status = run_validation_command(&validation.command, &cwd, timeout_secs);

        let (result, detail) = match exit_status {
            Ok(CommandOutcome::Exited(s)) if s.success() => {
                passed += 1;
                (ValidationResult::Passed, None)
            }
            Ok(CommandOutcome::Exited(s)) => {
                failed += 1;
                (ValidationResult::Failed, Some(format!("exited with {s}")))
            }
            Ok(CommandOutcome::TimedOut) => {
                failed += 1;
                (ValidationResult::Failed, Some(format!("timed out after {timeout_secs}s")))
            }
            Err(e) => {
                failed += 1;
                eprintln!(
                    "Warning: Could not run command for '{}': {}",
                    validation.name, e
                );
                (ValidationResult::Failed, Some(e.to_string()))
            }
        };
        let new_result = result.to_string();
        outcomes.push((validation.id.clone(), new_result.clone()));

        let mut entry = serde_json::json!({
            "validation_id": validation.id,
            "name":          validation.name,
            "type":          validation.validation_type,
            "command":       validation.command,
            "result":        &new_result,
            "run_at":        &now,
        });
        if let Some(detail) = &detail {
            entry["detail"] = serde_json::Value::String(detail.clone());
        }
        results.push(entry);

        if !printer.json {
            let mark = if new_result == "passed" { "✓" } else { "✗" };
            println!("  {} {} [{}]", mark, validation.name, new_result);
            println!("    cmd: {}", validation.command);
            if let Some(detail) = &detail {
                println!("    detail: {detail}");
            }
        }
    }

    // Phase 3 (DB reopened): persist results on the Validation nodes and the
    // VALIDATES edges.
    let db = GrafeoDb::open(&db_file)?;
    crate::db::with_transaction(&db, || {
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
        Ok(())
    })?;

    // End-of-run summary moves the phase: full anchor, result-sensitive.
    let next_step = if failed > 0 {
        "`loom next --mode fix`"
    } else {
        "`loom status` re-checks the compass"
    };
    if printer.json {
        let (scope_key, scope_val) = scope;
        printer.print_json(&crate::output::with_anchor(serde_json::json!({
            scope_key:   scope_val,
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

enum CommandOutcome {
    Exited(std::process::ExitStatus),
    TimedOut,
}

fn run_validation_command(
    command: &str,
    cwd: &std::path::Path,
    timeout_secs: u64,
) -> Result<CommandOutcome> {
    let mut child = StdCommand::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);

    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(CommandOutcome::Exited(status));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(CommandOutcome::TimedOut);
        }
        thread::sleep(Duration::from_millis(100));
    }
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
