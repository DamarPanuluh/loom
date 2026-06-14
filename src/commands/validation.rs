use anyhow::Result;
use uuid::Uuid;

use crate::cli::ValidationCmd;
use crate::db::queries::{
    delete_validation, get_hypothesis, get_validation, insert_validates, insert_validation,
    list_validations, resolve_validation, set_hypothesis_status,
    set_validates_status_for_validation, update_validation_definition, update_validation_result,
};
use crate::db::{ensure_initialized, GrafeoDb};
use crate::output::Printer;
use crate::types::{Validation, ValidationResult};

pub fn run(cmd: ValidationCmd, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;
    run_with_db(&db, &cwd, cmd, printer)
}

pub fn run_with_db(
    db: &GrafeoDb,
    _root: &std::path::Path,
    cmd: ValidationCmd,
    printer: &Printer,
) -> Result<()> {
    match cmd {
        ValidationCmd::Add {
            name,
            description,
            validation_type,
            command,
            intent,
        } => {
            crate::gate::acting_in_lane(
                "add a validation",
                &[
                    crate::db::schema::role::BUILDER,
                    crate::db::schema::role::VALIDATOR,
                ],
                None,
            )?;
            // Validate type
            validation_type
                .parse::<crate::types::ValidationType>()
                .map_err(|e| anyhow::anyhow!("{}", e))?;

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
            };
            insert_validation(db, &v)?;

            // A validation only proves something once it's attached to an intent.
            // Linking in one step (repeatable --intent) removes the most common
            // friction; otherwise we tell the driver exactly how to link it.
            let mut linked_intents: Vec<String> = Vec::new();
            for iid in &intent {
                let iid = crate::db::queries::resolve_intent(db, iid)?;
                insert_validates(db, &id, &iid, "", &now)?;
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
                            serde_json::json!([format!(
                                "Run it: `loom validate {}`.",
                                linked_intents[0]
                            ),]),
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
        }

        ValidationCmd::Mark {
            id,
            result,
            evidence,
            reason,
        } => {
            let marker = crate::gate::acting_in_lane(
                "mark a validation result",
                &[crate::db::schema::role::VALIDATOR],
                None,
            )?;
            // Verdict must be passed/failed/blocked ("not_run" is not a verdict).
            let res: ValidationResult = result.parse().map_err(|e| anyhow::anyhow!("{}", e))?;
            if res == ValidationResult::NotRun {
                anyhow::bail!(
                    "--result must be 'passed', 'failed', or 'blocked' (not_run is not a verdict)."
                );
            }
            // passed/failed carry evidence (what you checked); blocked carries a
            // reason (why it can't run) — recorded on the VALIDATES edge either
            // way so the state explains itself.
            let edge_note = if res == ValidationResult::Blocked {
                let r = reason.as_deref().unwrap_or("");
                crate::gate::require_substantive(
                    "reason",
                    r,
                    "why this proof cannot run yet (what it is waiting on)",
                )?;
                format!("blocked: {r}")
            } else {
                let ev = evidence.as_deref().unwrap_or("");
                crate::gate::require_substantive(
                    "evidence",
                    ev,
                    "what you checked to reach this verdict",
                )?;
                ev.to_string()
            };
            let vid = resolve_validation(db, &id)?;
            let validation = get_validation(db, &vid)?;
            let now = chrono::Utc::now().to_rfc3339();
            let n = crate::db::with_transaction(db, || {
                update_validation_result(db, &vid, &res.to_string(), &now)?;
                // Mirror the verdict onto the per-intent VALIDATES edges. `blocked`
                // leaves the edge uninspected (no proof was produced — that's
                // honest); the "blocked: <reason>" note distinguishes it from
                // forgotten, and the compass + validator queue skip blocked proofs.
                let status = match res {
                    ValidationResult::Passed => "passing",
                    ValidationResult::Failed => "failing",
                    _ => "uninspected",
                };
                let n = set_validates_status_for_validation(db, &vid, status, &edge_note)?;
                if res == ValidationResult::Passed {
                    if let Some(v) = &validation {
                        if let Some(hid) = v
                            .description
                            .lines()
                            .find_map(|line| line.strip_prefix("hypothesis:"))
                        {
                            let hid = hid.trim();
                            if get_hypothesis(db, hid)?.is_some_and(|h| h.status == "adopted") {
                                set_hypothesis_status(db, hid, "confirmed", &marker, &now)?;
                            }
                        }
                    }
                }
                Ok(n)
            })?;
            // Result-sensitive anchor: a verdict moves the phase, so the
            // output ends with where the driver goes next.
            let next_step = match res {
                ValidationResult::Passed =>
                    "`loom next --mode validate` for the next proof".to_string(),
                ValidationResult::Failed =>
                    "flag the owner: `loom intent mark <intent> --lifecycle needs_change --reason \"<validation failure>\"`".to_string(),
                _ =>
                    "out of the validator queue until re-marked; visible in `loom validation list` / `loom report`".to_string(),
            };
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
                printer.print_json(&crate::output::with_anchor(payload, db, &next_step)?);
            } else {
                println!("✓ Validation {vid} marked {res}  ({n} linked intent(s) updated)");
                if n == 0 {
                    println!("  → Not linked yet: `loom edge validates {vid} <intent-id>`.");
                }
                crate::output::print_anchor(db, &next_step)?;
            }
        }

        ValidationCmd::Update {
            id,
            command,
            description,
        } => {
            crate::gate::acting_in_lane(
                "update a validation definition",
                &[
                    crate::db::schema::role::BUILDER,
                    crate::db::schema::role::VALIDATOR,
                ],
                None,
            )?;
            if command.is_none() && description.is_none() {
                anyhow::bail!("Nothing to update — pass --command and/or --description.");
            }
            let vid = resolve_validation(db, &id)?;
            let command_changed = match (&command, get_validation(db, &vid)?) {
                (Some(c), Some(v)) => *c != v.command,
                _ => false,
            };
            // Atomic: the new definition and the proof reset land together —
            // a new command with the OLD green still attached would be a lie.
            let reset_edges = crate::db::with_transaction(db, || {
                update_validation_definition(db, &vid, command.as_deref(), description.as_deref())?;
                // The old result proved the OLD command — a changed command resets
                // the proof so green is re-earned by actually running the new one.
                let mut reset_edges = 0usize;
                if command_changed {
                    update_validation_result(db, &vid, "not_run", "")?;
                    reset_edges = set_validates_status_for_validation(
                        db,
                        &vid,
                        "uninspected",
                        "command updated — proof must be re-run",
                    )?;
                }
                Ok(reset_edges)
            })?;
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
        }

        ValidationCmd::Delete { id } => {
            crate::gate::acting_in_lane(
                "delete a validation",
                &[
                    crate::db::schema::role::BUILDER,
                    crate::db::schema::role::VALIDATOR,
                ],
                None,
            )?;
            let vid = resolve_validation(db, &id)?;
            // Atomic: node, VALIDATES edges, and their notes go together.
            if !crate::db::with_transaction(db, || delete_validation(db, &vid))? {
                anyhow::bail!(
                    "Validation '{}' not found.\nRun `loom validation list` to see available validations.",
                    vid
                );
            }
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
        }

        ValidationCmd::List { limit } => {
            let mut validations = list_validations(db)?;
            let total = crate::output::apply_limit(&mut validations, limit);
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "validations": validations,
                    "total":       total,
                    "truncated":   validations.len() < total,
                }));
            } else if validations.is_empty() {
                println!("(no validations defined)");
            } else {
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
                if let Some(m) = crate::output::more_marker(
                    total,
                    validations.len(),
                    "`loom validation list --limit 0`",
                ) {
                    println!("  {m}");
                }
            }
        }

        ValidationCmd::Show { id } => {
            // Same addressing as every other subcommand: id, exact name, or
            // unique name fragment.
            let id = resolve_validation(db, &id).unwrap_or(id);
            match get_validation(db, &id)? {
                None => anyhow::bail!(
                    "Validation '{}' not found.\nRun `loom validation list` to see available validations.",
                    id
                ),
                Some(ref v) => {
                    if printer.json {
                        printer.print_json(v);
                    } else {
                        println!("── Validation ─────────────────────────────────────────────────────");
                        println!("  id:          {}", v.id);
                        println!("  name:        {}", v.name);
                        println!("  type:        {}", v.validation_type);
                        println!("  command:     {}", v.command);
                        println!("  last_result: {}", v.last_result);
                        println!("  last_run:    {}",
                            if v.last_run.is_empty() { "(never)" } else { &v.last_run });
                        println!("  description: {}", v.description);
                    }
                }
            }
        }
    }
    Ok(())
}
