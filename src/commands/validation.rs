use anyhow::Result;
use uuid::Uuid;

use crate::cli::ValidationCmd;
use crate::db::{ensure_initialized, GrafeoDb};
use crate::db::queries::{
    delete_validation, get_validation, insert_validates, insert_validation, list_validations,
    resolve_validation, set_validates_status_for_validation, update_validation_definition,
    update_validation_result,
};
use crate::output::Printer;
use crate::types::{Validation, ValidationResult};

pub fn run(cmd: ValidationCmd, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    match cmd {
        ValidationCmd::Add { name, description, validation_type, command, intent } => {
            crate::gate::acting_in_lane(
                "add a validation",
                &[crate::db::schema::role::BUILDER, crate::db::schema::role::VALIDATOR],
                None,
            )?;
            // Validate type
            validation_type.parse::<crate::types::ValidationType>()
                .map_err(|e| anyhow::anyhow!("{}", e))?;

            let now = chrono::Utc::now().to_rfc3339();
            let id = Uuid::new_v4().to_string();
            let v = Validation {
                id:              id.clone(),
                name:            name.clone(),
                description:     description.unwrap_or_default(),
                validation_type: validation_type.clone(),
                command:         command.unwrap_or_default(),
                last_run:        String::new(),
                last_result:     "not_run".to_string(),
            };
            insert_validation(&db, &v)?;

            // A validation only proves something once it's attached to an intent.
            // Linking in one step (when --intent is given) removes the most common
            // friction; otherwise we tell the driver exactly how to link it.
            let mut linked_intent: Option<String> = None;
            if let Some(iid) = intent {
                let iid = crate::db::queries::resolve_intent(&db, &iid)?;
                let edge_id = Uuid::new_v4().to_string();
                insert_validates(&db, &edge_id, &id, &iid, "", &now)?;
                linked_intent = Some(iid);
            }

            if printer.json {
                let mut val = serde_json::to_value(&v)?;
                if let Some(obj) = val.as_object_mut() {
                    match &linked_intent {
                        Some(iid) => {
                            obj.insert("linked_intent".to_string(), serde_json::json!(iid));
                            obj.insert("next_steps".to_string(), serde_json::json!([
                                format!("Run it: `loom validate {}`.", iid),
                            ]));
                        }
                        None => {
                            obj.insert("next_steps".to_string(), serde_json::json!([
                                format!("Link it to an intent: `loom edge validates {} <intent-id>`.", id),
                                "Then run it: `loom validate <intent-id>`.",
                            ]));
                        }
                    }
                }
                printer.print_json(&val);
            } else {
                println!("✓ Validation '{}' created  (id: {})", name, id);
                println!("  type:    {}", validation_type);
                println!("  command: {}", v.command);
                match &linked_intent {
                    Some(iid) => println!("  → Linked to intent {iid}. Run it: `loom validate {iid}`."),
                    None => println!("  → Next: link it — `loom edge validates {id} <intent-id>` (or re-add with --intent)."),
                }
            }
        }

        ValidationCmd::Mark { id, result, evidence, reason } => {
            crate::gate::acting_in_lane(
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
                    "reason", r, "why this proof cannot run yet (what it is waiting on)",
                )?;
                format!("blocked: {r}")
            } else {
                let ev = evidence.as_deref().unwrap_or("");
                crate::gate::require_substantive(
                    "evidence", ev, "what you checked to reach this verdict",
                )?;
                ev.to_string()
            };
            let vid = resolve_validation(&db, &id)?;
            let now = chrono::Utc::now().to_rfc3339();
            update_validation_result(&db, &vid, &res.to_string(), &now)?;
            // Mirror the verdict onto the per-intent VALIDATES edges. `blocked`
            // leaves the edge uninspected (no proof was produced — that's
            // honest); the "blocked: <reason>" note distinguishes it from
            // forgotten, and the compass + validator queue skip blocked proofs.
            let status = match res {
                ValidationResult::Passed => "passing",
                ValidationResult::Failed => "failing",
                _ => "uninspected",
            };
            let n = set_validates_status_for_validation(&db, &vid, status, &edge_note)?;
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status": "ok", "validation_id": vid, "result": res.to_string(),
                    "intents_updated": n, "note": edge_note,
                }));
            } else {
                println!("✓ Validation {vid} marked {res}  ({n} linked intent(s) updated)");
                if res == ValidationResult::Blocked {
                    println!("  Out of the validator queue until re-marked; visible in `loom validation list` / `loom report`.");
                }
                if n == 0 {
                    println!("  → Not linked yet: `loom edge validates {vid} <intent-id>`.");
                }
            }
        }

        ValidationCmd::Update { id, command, description } => {
            crate::gate::acting_in_lane(
                "update a validation definition",
                &[crate::db::schema::role::BUILDER, crate::db::schema::role::VALIDATOR],
                None,
            )?;
            if command.is_none() && description.is_none() {
                anyhow::bail!("Nothing to update — pass --command and/or --description.");
            }
            let vid = resolve_validation(&db, &id)?;
            let command_changed = match (&command, get_validation(&db, &vid)?) {
                (Some(c), Some(v)) => *c != v.command,
                _ => false,
            };
            update_validation_definition(&db, &vid, command.as_deref(), description.as_deref())?;
            // The old result proved the OLD command — a changed command resets
            // the proof so green is re-earned by actually running the new one.
            let mut reset_edges = 0usize;
            if command_changed {
                update_validation_result(&db, &vid, "not_run", "")?;
                reset_edges = set_validates_status_for_validation(
                    &db, &vid, "uninspected", "command updated — proof must be re-run",
                )?;
            }
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
                &[crate::db::schema::role::BUILDER, crate::db::schema::role::VALIDATOR],
                None,
            )?;
            let vid = resolve_validation(&db, &id)?;
            if !delete_validation(&db, &vid)? {
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

        ValidationCmd::List => {
            let validations = list_validations(&db)?;
            if printer.json {
                printer.print_json(&validations);
            } else if validations.is_empty() {
                println!("(no validations defined)");
            } else {
                println!(
                    "  {result:<8}  {vtype:<14}  {name:<40}  id",
                    result = "RESULT",
                    vtype  = "TYPE",
                    name   = "NAME",
                );
                println!("  {}", "-".repeat(100));
                for v in &validations {
                    println!(
                        "  [{result:<8}]  {vtype:<14}  {name:<40}  {id}",
                        result = v.last_result,
                        vtype  = v.validation_type,
                        name   = v.name,
                        id     = v.id,
                    );
                }
            }
        }

        ValidationCmd::Show { id } => {
            match get_validation(&db, &id)? {
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
