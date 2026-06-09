use anyhow::Result;
use std::env;
use uuid::Uuid;

use crate::cli::ValidationCmd;
use crate::db::{ensure_initialized, GrafeoDb};
use crate::db::queries::{get_validation, insert_validates, insert_validation, list_validations};
use crate::output::Printer;
use crate::types::Validation;

pub fn run(cmd: ValidationCmd, printer: &Printer) -> Result<()> {
    let cwd = env::current_dir()?;
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
