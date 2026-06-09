use anyhow::Result;
use std::env;
use uuid::Uuid;

use crate::cli::IntentCmd;
use crate::db::{ensure_initialized, GrafeoDb};
use crate::db::queries::{
    confirm_intent, delete_intent, edges_for_intent, get_intent, insert_intent,
    list_hierarchy_for_intent, list_implements_for_intent, list_intents, notes_for_target,
    set_intent_lifecycle,
};
use crate::db::schema::role;
use crate::gate;
use crate::output::{fmt_edge_row, fmt_intent, fmt_intent_row, Printer};
use crate::types::Intent;

pub fn run(cmd: IntentCmd, printer: &Printer) -> Result<()> {
    let cwd = env::current_dir()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    match cmd {
        IntentCmd::Add { name, description, level, domain, aspect, lifecycle, sources } => {
            gate::acting_in_lane("add an intent", &[role::BUILDER], None)?;
            // Validate abstraction level + lifecycle
            level.parse::<crate::types::AbstractionLevel>()
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            lifecycle.parse::<crate::types::LifecycleState>()
                .map_err(|e| anyhow::anyhow!("{}", e))?;

            let source_refs = serde_json::to_string(&sources)?;
            let now = chrono::Utc::now().to_rfc3339();
            let id  = Uuid::new_v4().to_string();

            let intent = Intent {
                id:                id.clone(),
                name:              name.clone(),
                description,
                abstraction_level: level,
                domain,
                source_refs,
                status:            "proposed".to_string(),
                aspect,
                lifecycle,
                created_at:        now.clone(),
                updated_at:        now,
            };

            insert_intent(&db, &intent)?;

            // Connecting the intent into the HIERARCHY tree is the FIRST step
            // (the vertical spine is what makes the graph complete). A `system`
            // intent is a root and gets decomposed downward; anything else needs
            // a parent. Lead with that so a cold driver never leaves intents
            // floating, then point at grounding.
            let is_root = intent.abstraction_level == "system";
            let tree_step = if is_root {
                format!("Decompose it: add child intents, then link with `loom edge hierarchy {} <child-id>` (this is the tree's root).", id)
            } else {
                format!("Attach it to the tree: `loom edge hierarchy <parent-id> {}` (every non-system intent needs exactly one parent).", id)
            };

            if printer.json {
                let mut v = serde_json::to_value(&intent)?;
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("next_steps".to_string(), serde_json::json!([
                        tree_step,
                        "Ground it to code: `loom edge implement <intent> <codefile> --locator \"fn …\"` (required for leaf intents).",
                        "Relate it to other intents — `loom next` will surface unexplored pairs (optional).",
                        "If this is a feature, add its sad/fallback siblings (--aspect).",
                    ]));
                }
                printer.print_json(&v);
            } else {
                println!("✓ Intent created");
                println!("{}", fmt_intent(&intent));
                println!("  → Next: {}", tree_step);
                println!("          then ground it: `loom edge implement {} <codefile> --locator \"fn …\"`.", id);
            }
        }

        IntentCmd::Confirm { id } => {
            // Confirmation is a *verdict* that the intent is valid — validator
            // lane, so the builder cannot ratify its own proposals.
            gate::acting_in_lane("confirm an intent", &[role::VALIDATOR], None)?;
            let now  = chrono::Utc::now().to_rfc3339();
            let found = confirm_intent(&db, &id, &now)?;
            if !found {
                anyhow::bail!(
                    "Intent '{}' not found.\nRun `loom intent list` to see available intents.",
                    id
                );
            }
            if printer.json {
                printer.print_json(&serde_json::json!({"status":"ok","id":id,"new_status":"confirmed"}));
            } else {
                println!("✓ Intent {} confirmed", id);
            }
        }

        IntentCmd::Mark { id, lifecycle, reason } => {
            // Lifecycle is builder-owned; the fixer transitions it
            // (needs_change → implemented) as part of resolving issues.
            let by = gate::acting_in_lane(
                "set an intent lifecycle",
                &[role::BUILDER, role::FIXER],
                None,
            )?;
            lifecycle.parse::<crate::types::LifecycleState>()
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            let now = chrono::Utc::now().to_rfc3339();
            let found = set_intent_lifecycle(&db, &id, &lifecycle, &now)?;
            if !found {
                anyhow::bail!(
                    "Intent '{}' not found.\nRun `loom intent list` to see available intents.",
                    id
                );
            }
            // Record the rationale as a note (append-only memory).
            if let Some(ref r) = reason {
                let note = crate::types::Note {
                    id:          Uuid::new_v4().to_string(),
                    kind:        "decision".to_string(),
                    text:        format!("lifecycle → {}: {}", lifecycle, r),
                    author:      by.clone(),
                    target_kind: "intent".to_string(),
                    target_id:   id.clone(),
                    created_at:  now.clone(),
                };
                crate::db::queries::insert_note(&db, &note)?;
            }
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status": "ok", "id": id, "lifecycle": lifecycle,
                }));
            } else {
                println!("✓ Intent {} → lifecycle '{}'", id, lifecycle);
                if lifecycle == "planned" || lifecycle == "needs_change" {
                    println!("  → Next: `loom next --mode build` will surface it.");
                }
            }
        }

        IntentCmd::Delete { id } => {
            gate::acting_in_lane("delete an intent", &[role::BUILDER], None)?;
            let deleted = delete_intent(&db, &id)?;
            if !deleted {
                anyhow::bail!(
                    "Intent '{}' not found.\nRun `loom intent list` to see available intents.",
                    id
                );
            }
            if printer.json {
                printer.print_json(&serde_json::json!({"status": "ok", "id": id, "deleted": true}));
            } else {
                println!("✓ Intent {} deleted (with its edges and notes).", id);
            }
        }

        IntentCmd::List { status, level } => {
            // Validate filter values against the domain vocabulary.
            if let Some(ref s) = status {
                s.parse::<crate::types::IntentStatus>().map_err(|e| anyhow::anyhow!("{}", e))?;
            }
            if let Some(ref l) = level {
                l.parse::<crate::types::AbstractionLevel>().map_err(|e| anyhow::anyhow!("{}", e))?;
            }
            let intents = list_intents(&db, status.as_deref(), level.as_deref())?;
            if printer.json {
                printer.print_json(&intents);
            } else {
                if intents.is_empty() {
                    println!("(no intents found)");
                } else {
                    println!(
                        "  {status:>20}   {level:<15}  {name:<40}  id",
                        status = "STATUS",
                        level  = "LEVEL",
                        name   = "NAME",
                    );
                    println!("  {}", "-".repeat(90));
                    for i in &intents {
                        println!("{}", fmt_intent_row(i));
                    }
                }
            }
        }

        IntentCmd::Show { id } => {
            let intent = get_intent(&db, &id)?;
            match intent {
                None => anyhow::bail!(
                    "Intent '{}' not found.\nRun `loom intent list` to see available intents.",
                    id
                ),
                Some(ref i) => {
                    let edges = edges_for_intent(&db, &id)?;
                    let hierarchy = list_hierarchy_for_intent(&db, &id)?;
                    let implements = list_implements_for_intent(&db, &id)?;
                    let notes = notes_for_target(&db, &id)?;
                    if printer.json {
                        printer.print_json(&serde_json::json!({
                            "intent": i,
                            "edges": edges,
                            "hierarchy": hierarchy,
                            "implements": implements,
                            "notes": notes,
                        }));
                    } else {
                        println!("── Intent ─────────────────────────────────────────────────────────");
                        println!("{}", fmt_intent(i));
                        println!();
                        println!("── RELATES_TO edges ({}) ────────────────────────────────────────────", edges.len());
                        if edges.is_empty() {
                            println!("  (none)");
                        } else {
                            for e in &edges {
                                println!("{}", fmt_edge_row(e));
                            }
                        }
                        println!();
                        println!("── Hierarchy ({}) ───────────────────────────────────────────────────", hierarchy.len());
                        if hierarchy.is_empty() {
                            println!("  (none — no parent/child intents)");
                        } else {
                            for h in &hierarchy {
                                if h.parent_id == id {
                                    println!("  ↓ child:  {} ({})", h.child_name, h.child_id);
                                } else {
                                    println!("  ↑ parent: {} ({})", h.parent_name, h.parent_id);
                                }
                            }
                        }
                        println!();
                        println!("── Implements ({}) ──────────────────────────────────────────────────", implements.len());
                        if implements.is_empty() {
                            println!("  (none — intent not yet grounded to code)");
                        } else {
                            for im in &implements {
                                let loc = if im.locator.is_empty() { String::new() } else { format!("  @ {}", im.locator) };
                                println!("  → {}{}  [{}]", im.codefile_path, loc, im.inspection_status);
                            }
                        }
                        println!();
                        println!("── Notes ({}) ───────────────────────────────────────────────────────", notes.len());
                        if notes.is_empty() {
                            println!("  (none)");
                        } else {
                            for n in &notes {
                                println!("  [{}] {}  ({})", n.kind, n.text, n.author);
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
