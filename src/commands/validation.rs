use anyhow::Result;
use uuid::Uuid;

use crate::cli::ValidationCmd;
use crate::db::{ensure_initialized, GraphReadHandle, GraphReadRepository};
use crate::output::Printer;
use crate::types::{Validation, ValidationResult};

pub fn run(cmd: ValidationCmd, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    match cmd {
        ValidationCmd::List { result, limit } => {
            let db = GraphReadHandle::open(&cwd)?;
            run_list_with_db(&db, result, limit, printer)
        }
        ValidationCmd::Show { id } => {
            let db = GraphReadHandle::open(&cwd)?;
            run_show_with_db(&db, id, printer)
        }
        ValidationCmd::Add {
            name,
            description,
            validation_type,
            command,
            intent,
        } => {
            ensure_initialized(&cwd)?;
            run_add_with_sqlite(
                &cwd,
                name,
                description,
                validation_type,
                command,
                intent,
                printer,
            )
        }
        ValidationCmd::Mark {
            id,
            result,
            evidence,
            reason,
        } => {
            let db_file = ensure_initialized(&cwd)?;
            run_mark_with_sqlite(&cwd, &db_file, id, result, evidence, reason, printer)
        }
        ValidationCmd::Update {
            id,
            command,
            description,
        } => {
            ensure_initialized(&cwd)?;
            run_update_with_sqlite(&cwd, id, command, description, printer)
        }
        ValidationCmd::Delete { id } => {
            ensure_initialized(&cwd)?;
            run_delete_with_sqlite(&cwd, id, printer)
        }
    }
}

/// G3 (the proof-honesty content gate): reject proof commands whose STATIC SHAPE
/// can't falsify the intent — an empty command, an always-pass vacuous command,
/// or a failure-SWALLOWING tail that passes even when the underlying test fails.
/// Such a command would mint a false "proven" green (the EXIT-0 launder at the
/// add boundary). `manual_check` (human gate, no command) and `saga` (command
/// derived from a spec file) are exempt — their honesty is enforced elsewhere.
pub(crate) fn check_proof_command_shape(validation_type: &str, command: &str) -> Result<()> {
    if matches!(validation_type, "manual_check" | "saga") {
        return Ok(());
    }
    let cmd = command.trim();
    if cmd.is_empty() {
        anyhow::bail!(
            "A {validation_type} proof needs a command that runs and ASSERTS something — an empty \
             command can never falsify the intent. Pass --command \"<runner …>\", or use \
             --type manual_check for a human gate."
        );
    }
    // Failure-swallowing tails: the command passes even when the test fails.
    const SWALLOWS: &[&str] = &["|| true", "|| :", "|| exit 0", "|| echo", "; true", "; :"];
    if let Some(bad) = SWALLOWS.iter().find(|s| cmd.contains(**s)) {
        anyhow::bail!(
            "Proof command swallows failure (`{bad}`) — it exits 0 even when the test fails, so it \
             proves nothing. Remove the failure-swallowing tail."
        );
    }
    // Always-pass vacuous commands.
    if matches!(
        cmd,
        "true" | ":" | "exit 0" | "exit" | "/bin/true" | "/usr/bin/true"
    ) {
        anyhow::bail!(
            "Proof command `{cmd}` always exits 0 without asserting anything — it would mint a false \
             'proven'. Give the real runner (e.g. `cargo test <name>`, `pytest -k …`, `go test ./…`)."
        );
    }
    Ok(())
}

fn run_add_with_sqlite(
    root: &std::path::Path,
    name: String,
    description: Option<String>,
    validation_type: String,
    command: Option<String>,
    intent: Vec<String>,
    printer: &Printer,
) -> Result<()> {
    crate::gate::acting_in_lane(&crate::gate::lane::ADD_VALIDATION, None)?;
    validation_type
        .parse::<crate::types::ValidationType>()
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    check_proof_command_shape(&validation_type, command.as_deref().unwrap_or(""))?;

    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let snapshot = store.query_snapshot()?;
    let now = chrono::Utc::now().to_rfc3339();
    let id = Uuid::new_v4().to_string();
    let v = Validation {
        id: id.clone(),
        name: name.clone(),
        description: description.unwrap_or_default(),
        validation_type: validation_type.clone(),
        command: command.unwrap_or_default(),
        last_run: String::new(),
        last_result: "not_run".to_string(),
        last_executed_run: String::new(),
        discrimination_status: String::new(),
    };
    store.insert_validation(&v)?;

    let mut linked_intents: Vec<String> = Vec::new();
    for iid in &intent {
        let iid = crate::db::queries::resolve_intent_from_snapshot(&snapshot, iid)?;
        store.insert_validates(&id, &iid, "", &now)?;
        linked_intents.push(iid);
    }

    if printer.json {
        let mut val = serde_json::to_value(&v)?;
        if let Some(obj) = val.as_object_mut() {
            if linked_intents.is_empty() {
                obj.insert(
                    "next_steps".to_string(),
                    serde_json::json!([
                        format!(
                            "Link it to an intent: `loom edge validates {} <intent-id>`.",
                            id
                        ),
                        "Then run it: `loom validate <intent-id>`.",
                    ]),
                );
            } else {
                obj.insert(
                    "linked_intents".to_string(),
                    serde_json::json!(linked_intents),
                );
                obj.insert(
                    "next_steps".to_string(),
                    serde_json::json!([format!("Run it: `loom validate {}`.", linked_intents[0]),]),
                );
            }
        }
        printer.print_json(&val);
    } else {
        println!("✓ Validation '{}' created  (id: {})", name, id);
        println!("  type:    {}", validation_type);
        println!("  command: {}", v.command);
        if linked_intents.is_empty() {
            println!("  → Next: link it — `loom edge validates {id} <intent-id>` (or re-add with --intent).");
        } else {
            for iid in &linked_intents {
                println!("  → Linked to intent {iid}.");
            }
            println!("  Run: `loom validate {}`.", linked_intents[0]);
        }
    }
    Ok(())
}

fn prepare_mark_result(
    result: &str,
    evidence: Option<&str>,
    reason: Option<&str>,
) -> Result<(String, ValidationResult, String)> {
    let marker = crate::gate::acting_in_lane(&crate::gate::lane::MARK_VALIDATION, None)?;
    let res: ValidationResult = result.parse().map_err(|e| anyhow::anyhow!("{}", e))?;
    if res == ValidationResult::NotRun {
        anyhow::bail!(
            "--result must be 'passed', 'failed', or 'blocked' (not_run is not a verdict)."
        );
    }
    let edge_note = if res == ValidationResult::Blocked {
        let r = reason.unwrap_or("");
        crate::gate::require_substantive(
            "reason",
            r,
            "why this proof cannot run yet (what it is waiting on)",
        )?;
        format!("blocked: {r}")
    } else {
        let ev = evidence.unwrap_or("");
        crate::gate::require_substantive("evidence", ev, "what you checked to reach this verdict")?;
        ev.to_string()
    };
    Ok((marker, res, edge_note))
}

fn validation_mark_next_step(res: &ValidationResult) -> String {
    match res {
        ValidationResult::Passed => "`loom next --mode validate` for the next proof".to_string(),
        ValidationResult::Failed => {
            "flag the owner: `loom intent mark <intent> --lifecycle needs_change --reason \"<validation failure>\"`".to_string()
        }
        _ => {
            "out of the validator queue until re-marked; visible in `loom validation list` / `loom report`".to_string()
        }
    }
}

fn validation_mark_edge_status(res: &ValidationResult) -> &'static str {
    match res {
        ValidationResult::Passed => "passing",
        ValidationResult::Failed => "failing",
        _ => "uninspected",
    }
}

fn run_mark_with_sqlite(
    root: &std::path::Path,
    _db_file: &std::path::Path,
    id: String,
    result: String,
    evidence: Option<String>,
    reason: Option<String>,
    printer: &Printer,
) -> Result<()> {
    let (marker, res, edge_note) =
        prepare_mark_result(&result, evidence.as_deref(), reason.as_deref())?;
    let now = chrono::Utc::now().to_rfc3339();
    let mut store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let (vid, n) = store.mark_validation_result(
        &id,
        &res.to_string(),
        validation_mark_edge_status(&res),
        &edge_note,
        &marker,
        &now,
        // A hand-mark is ASSERTED proof, not machine-executed — pass None so a
        // prior last_executed_run (if the executor ran it before) is preserved
        // and a never-run proof stays empty (asserted, not executed).
        None,
        None,
    )?;
    let next_step = validation_mark_next_step(&res);
    if printer.json {
        let mut payload = serde_json::json!({
            "status": "ok", "validation_id": vid, "result": res.to_string(),
            "intents_updated": n, "note": edge_note,
        });
        if n == 0 {
            payload["hint"] = serde_json::Value::String(format!(
                "Not linked yet: `loom edge validates {vid} <intent-id>`."
            ));
        }
        printer.print_json(&crate::output::with_read_anchor(
            payload, &store, &next_step,
        )?);
    } else {
        println!("✓ Validation {vid} marked {res}  ({n} linked intent(s) updated)");
        if n == 0 {
            println!("  → Not linked yet: `loom edge validates {vid} <intent-id>`.");
        }
        let snapshot = store.query_snapshot()?;
        let graph_state = store.graph_state(&snapshot)?;
        println!("  → Next: {next_step}");
        println!("  {}", crate::output::fmt_pulse(&graph_state));
    }
    Ok(())
}

fn run_update_with_sqlite(
    root: &std::path::Path,
    id: String,
    command: Option<String>,
    description: Option<String>,
    printer: &Printer,
) -> Result<()> {
    crate::gate::acting_in_lane(&crate::gate::lane::UPDATE_VALIDATION, None)?;
    if command.is_none() && description.is_none() {
        anyhow::bail!("Nothing to update — pass --command and/or --description.");
    }
    let mut store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    // A new command must clear the same G3 honesty gate as `add` (look up the
    // proof's type — the gate exempts manual_check/saga).
    if let Some(new_command) = command.as_deref() {
        let vtype = store
            .query_snapshot()?
            .validations
            .iter()
            .find(|v| v.id == id)
            .map(|v| v.validation_type.clone())
            .unwrap_or_else(|| "test".to_string());
        check_proof_command_shape(&vtype, new_command)?;
    }
    let (vid, command_changed, reset_edges) =
        store.update_validation_definition(&id, command.as_deref(), description.as_deref())?;
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok", "validation_id": vid,
            "command_changed": command_changed,
            "proof_reset": command_changed,
            "intents_reset": reset_edges,
        }));
    } else {
        println!("✓ Validation {vid} updated.");
        if command_changed {
            println!("  Command changed → proof reset (last_result=not_run, {reset_edges} VALIDATES edge(s) → uninspected).");
            println!("  → Re-run it: `loom validate <intent>` (or `loom validation mark` for manual proofs).");
        }
    }
    Ok(())
}

fn run_delete_with_sqlite(root: &std::path::Path, id: String, printer: &Printer) -> Result<()> {
    crate::gate::acting_in_lane(&crate::gate::lane::DELETE_VALIDATION, None)?;
    let mut store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let vid = store.delete_validation(&id)?;
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok", "id": vid, "deleted": true,
            "message": "Validation and its VALIDATES edges removed. Intents that lost \
                        their only proof will resurface in `loom next --mode validate`.",
        }));
    } else {
        println!("✓ Validation {vid} deleted (with its VALIDATES edges).");
        println!("  Intents that lost their only proof resurface in `loom next --mode validate`.");
    }
    Ok(())
}

fn run_list_with_db(
    db: &dyn GraphReadRepository,
    result: Option<String>,
    limit: usize,
    printer: &Printer,
) -> Result<()> {
    let mut validations = db.query_snapshot()?.validations;
    let result_filter = if let Some(result) = result {
        let parsed: ValidationResult = result.parse().map_err(|e| anyhow::anyhow!("{}", e))?;
        let result = parsed.to_string();
        validations.retain(|validation| validation.last_result == result);
        Some(result)
    } else {
        None
    };
    let total = crate::output::apply_limit(&mut validations, limit);
    if printer.json {
        printer.print_json(&serde_json::json!({
            "validations": validations,
            "total":       total,
            "truncated":   validations.len() < total,
            "result_filter": result_filter,
        }));
    } else if validations.is_empty() {
        if let Some(result) = result_filter {
            println!("(no validations with result={result})");
        } else {
            println!("(no validations defined)");
        }
    } else {
        if let Some(result) = &result_filter {
            println!("  result filter: {result}");
        }
        println!(
            "  {result:<8}  {vtype:<14}  {name:<40}  id",
            result = "RESULT",
            vtype = "TYPE",
            name = "NAME",
        );
        println!("  {}", "-".repeat(100));
        for v in &validations {
            println!(
                "  [{result:<8}]  {vtype:<14}  {name:<40}  {id}",
                result = v.last_result,
                vtype = v.validation_type,
                name = v.name,
                id = v.id,
            );
        }
        if let Some(m) =
            crate::output::more_marker(total, validations.len(), "`loom validation list --limit 0`")
        {
            println!("  {m}");
        }
    }
    Ok(())
}

fn run_show_with_db(db: &dyn GraphReadRepository, id: String, printer: &Printer) -> Result<()> {
    let validations = db.query_snapshot()?.validations;
    // Same addressing as every other subcommand: id, exact name, or unique
    // name fragment. Preserve the existing command behavior: a failed resolve
    // falls back to exact-id lookup before reporting not found.
    let id = resolve_validation_from_list(&validations, &id).unwrap_or(id);
    let Some(v) = validations.iter().find(|validation| validation.id == id) else {
        anyhow::bail!(
            "Validation '{}' not found.\nRun `loom validation list` to see available validations.",
            id
        );
    };
    if printer.json {
        printer.print_json(v);
    } else {
        println!("── Validation ─────────────────────────────────────────────────────");
        println!("  id:          {}", v.id);
        println!("  name:        {}", v.name);
        println!("  type:        {}", v.validation_type);
        println!("  command:     {}", v.command);
        println!("  last_result: {}", v.last_result);
        println!(
            "  last_run:    {}",
            if v.last_run.is_empty() {
                "(never)"
            } else {
                &v.last_run
            }
        );
        println!("  description: {}", v.description);
    }
    Ok(())
}

fn resolve_validation_from_list(validations: &[Validation], key: &str) -> Result<String> {
    if validations.iter().any(|validation| validation.id == key) {
        return Ok(key.to_string());
    }
    let kl = key.to_lowercase();
    let exact: Vec<_> = validations
        .iter()
        .filter(|validation| validation.name.to_lowercase() == kl)
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0].id.clone());
    }
    let subs: Vec<_> = validations
        .iter()
        .filter(|validation| validation.name.to_lowercase().contains(&kl))
        .collect();
    match subs.len() {
        1 => Ok(subs[0].id.clone()),
        0 => anyhow::bail!(
            "No validation matches '{}' (by id, name, or fragment). Run `loom validation list`.",
            key
        ),
        _ => anyhow::bail!(
            "'{}' is ambiguous — matches {} validations. Use the id (`loom validation list`).",
            key,
            subs.len()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn g3_rejects_unfalsifiable_proof_commands() {
        // Vacuous / always-pass commands can't falsify anything.
        for bad in ["", "  ", "true", ":", "exit 0", "/bin/true"] {
            assert!(
                check_proof_command_shape("test", bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
        // Failure-swallowing tails pass even when the test fails.
        for bad in [
            "cargo test || true",
            "pytest || :",
            "go test ./... || exit 0",
            "jest ; true",
        ] {
            assert!(
                check_proof_command_shape("test", bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
        // Real runner commands pass for every executable type.
        for ok in [
            "cargo test foo",
            "pytest -k auth",
            "go test ./...",
            "npm test",
        ] {
            assert!(
                check_proof_command_shape("assertion", ok).is_ok(),
                "{ok:?} should pass"
            );
        }
        // manual_check (human gate) and saga (spec-derived) are exempt, even empty.
        assert!(check_proof_command_shape("manual_check", "").is_ok());
        assert!(check_proof_command_shape("saga", "").is_ok());
    }
}
