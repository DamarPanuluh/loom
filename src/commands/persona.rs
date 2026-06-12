//! `loom persona` — manage personas and their SERVES/JOURNEYS edges.

use anyhow::Result;
use uuid::Uuid;

use crate::cli::{ExploreSubCmd, PersonaCmd};
use crate::db::queries::{
    get_intent, get_or_create_journeys, get_or_create_serves, get_persona, insert_persona,
    list_journeys_for_persona, list_personas, list_serves_for_persona, resolve_persona,
    resolve_validation, update_serves_ground, update_serves_independent, update_serves_issue,
};
use crate::db::schema::role;
use crate::db::{ensure_initialized, GrafeoDb};
use crate::gate;
use crate::output::{apply_limit, print_anchor, with_anchor, Printer};
use crate::types::Persona;

pub fn run(cmd: PersonaCmd, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    match cmd {
        // ----------------------------------------------------------------
        // loom persona add --name <n> --description <d>
        // ----------------------------------------------------------------
        PersonaCmd::Add { name, description, author } => {
            let agent = gate::acting_in_lane(
                "add a persona",
                &[role::BUILDER],
                author.as_deref(),
            )?;
            gate::require_substantive(
                "description", &description,
                "who this persona is and what distinguishes them from other audience segments",
            )?;

            // Duplicate name guard.
            if let Some(existing) = get_persona(&db, &name)? {
                anyhow::bail!(
                    "Persona '{}' already exists (id: {}).\n  \
                     Use `loom persona show {}` to see its SERVES edges.",
                    existing.name, existing.id, existing.id
                );
            }

            let now = chrono::Utc::now().to_rfc3339();
            let persona = Persona {
                id: Uuid::new_v4().to_string(),
                name: name.clone(),
                description: description.clone(),
                author: agent,
                created_at: now.clone(),
                updated_at: now,
            };
            insert_persona(&db, &persona)?;

            let next_step = format!(
                "Link intents this persona relies on: loom persona serve {} <intent>",
                persona.id
            );
            if printer.json {
                let v = with_anchor(serde_json::to_value(&persona)?, &db, &next_step)?;
                printer.print_json(&v);
            } else {
                println!("✓ Persona added: {} ({})", persona.name, persona.id);
                println!("  \"{}\"", persona.description);
                print_anchor(&db, &next_step)?;
            }
        }

        // ----------------------------------------------------------------
        // loom persona list
        // ----------------------------------------------------------------
        PersonaCmd::List { limit } => {
            let mut personas = list_personas(&db)?;
            let total = apply_limit(&mut personas, limit);
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "personas": personas,
                    "total": total,
                    "truncated": total > personas.len(),
                }));
            } else if personas.is_empty() {
                println!("(no personas — add one: loom persona add --name <name> --description \"<who they are>\")");
            } else {
                for p in &personas {
                    println!("  {:<36}  {}", p.id, p.name);
                    println!("    {}", p.description);
                }
                if total > personas.len() {
                    println!("  … +{} more — loom persona list --limit 0", total - personas.len());
                }
                println!("\n  → loom persona show <id>   to see a persona's SERVES edges");
            }
        }

        // ----------------------------------------------------------------
        // loom persona show <id>
        // ----------------------------------------------------------------
        PersonaCmd::Show { id } => {
            let persona_id = resolve_persona(&db, &id)?;
            let persona = get_persona(&db, &persona_id)?.expect("resolved above");
            let serves = list_serves_for_persona(&db, &persona_id)?;
            let journeys = list_journeys_for_persona(&db, &persona_id)?;

            if printer.json {
                printer.print_json(&serde_json::json!({
                    "persona": persona,
                    "serves": serves,
                    "journeys": journeys,
                }));
            } else {
                println!("── Persona ────────────────────────────────────────────────────────");
                println!("  Name:   {}", persona.name);
                println!("  ID:     {}", persona.id);
                println!("  \"{}\"", persona.description);
                println!("  Added:  {}", persona.created_at);

                println!();
                println!("── SERVES ({} intent{}) ──────────────────────────────────────────",
                    serves.len(), if serves.len() == 1 { "" } else { "s" });
                if serves.is_empty() {
                    println!("  (none — link intents: loom persona serve {} <intent>)", persona.id);
                } else {
                    for e in &serves {
                        let icon = match e.inspection_status.as_str() {
                            "passing"              => "✓",
                            "failing"              => "✗",
                            "independent"          => "–",
                            "needs_reverification" => "~",
                            _                      => "?",
                        };
                        println!("  {icon} [{}]  {}", e.inspection_status, e.intent_name);
                        if !e.criterion.is_empty() {
                            println!("      criterion: {}", e.criterion);
                        }
                    }
                }

                println!();
                println!("── JOURNEYS ({} saga{}) ──────────────────────────────────────────",
                    journeys.len(), if journeys.len() == 1 { "" } else { "s" });
                if journeys.is_empty() {
                    println!("  (none — link a saga: loom persona journey {} <saga-validation-id>)", persona.id);
                } else {
                    for j in &journeys {
                        println!("  ↪ {}", j.validation_name);
                    }
                }

                println!();
                println!("  → loom persona serve {} <intent>   inspect/verify a SERVES edge", persona.id);
            }
        }

        // ----------------------------------------------------------------
        // loom persona serve <persona> <intent> [ground|issue|independent]
        // ----------------------------------------------------------------
        PersonaCmd::Serve { persona_id, intent_id, subcommand } => {
            let persona_id = resolve_persona(&db, &persona_id)?;
            let intent_id  = crate::db::queries::resolve_intent(&db, &intent_id)?;
            let now = chrono::Utc::now().to_rfc3339();

            match subcommand {
                None => {
                    let edge = get_or_create_serves(&db, &persona_id, &intent_id, &now)?;
                    let persona = get_persona(&db, &persona_id)?.expect("resolved above");
                    let intent  = get_intent(&db, &intent_id)?
                        .ok_or_else(|| anyhow::anyhow!("Intent '{}' not found.", intent_id))?;

                    if printer.json {
                        printer.print_json(&serde_json::json!({
                            "edge":    edge,
                            "persona": persona,
                            "intent":  intent,
                        }));
                    } else {
                        println!("── Persona ───────────────────────────────────────────────────────");
                        println!("  {} — {}", persona.name, persona.description);
                        println!();
                        println!("── Intent ────────────────────────────────────────────────────────");
                        println!("  {} ({})", intent.name, intent.abstraction_level);
                        println!("  {}", intent.description);
                        println!();
                        println!("── SERVES edge ───────────────────────────────────────────────────");
                        println!("  status:   {}", edge.inspection_status);
                        if !edge.criterion.is_empty() {
                            println!("  criterion: {}", edge.criterion);
                        }
                        println!();
                        println!("Next steps:");
                        println!(
                            "  loom persona serve {p} {i} ground --criterion \"<what serving this persona looks like>\" --confidence 0.9",
                            p = persona_id, i = intent_id
                        );
                        println!(
                            "  loom persona serve {p} {i} issue  --criterion \"<test>\" --evidence \"<what's wrong>\"",
                            p = persona_id, i = intent_id
                        );
                        println!(
                            "  loom persona serve {p} {i} independent --notes \"<why this intent doesn't serve this persona>\"",
                            p = persona_id, i = intent_id
                        );
                    }
                }

                Some(ExploreSubCmd::Ground { criterion, evidence, evidence_locator, confidence, inspected_by }) => {
                    let by = gate::acting_in_lane(
                        "ground a SERVES edge",
                        &[role::ANALYZER, role::FIXER],
                        inspected_by.as_deref(),
                    )?;
                    gate::require_substantive(
                        "criterion", &criterion,
                        "what 'serving this persona' looks like for this intent",
                    )?;
                    if !evidence.trim().is_empty() {
                        gate::require_substantive("evidence", &evidence, "what was found")?;
                    }
                    let evidence = gate::compose_evidence(&evidence_locator, &evidence)?;
                    gate::require_confidence(confidence)?;

                    let edge = get_or_create_serves(&db, &persona_id, &intent_id, &now)?;
                    update_serves_ground(&db, &persona_id, &intent_id, &criterion, &evidence, confidence, &by, &now)?;
                    let updated = crate::types::ServesEdge {
                        inspection_status: "passing".to_string(),
                        criterion,
                        evidence,
                        confidence,
                        inspected_by: by.clone(),
                        last_inspected: now,
                        ..edge
                    };
                    let next_step = "`loom next` for the next item.";
                    if printer.json {
                        let v = with_anchor(serde_json::to_value(&updated)?, &db, next_step)?;
                        printer.print_json(&v);
                    } else {
                        println!("✓ SERVES edge: {} → {} → passing", updated.persona_name, updated.intent_name);
                        if !updated.criterion.is_empty() {
                            println!("  criterion: {}", updated.criterion);
                        }
                        print_anchor(&db, next_step)?;
                    }
                }

                Some(ExploreSubCmd::Issue { criterion, evidence, evidence_locator, confidence, inspected_by }) => {
                    let by = gate::acting_in_lane(
                        "issue a SERVES edge",
                        &[role::ANALYZER],
                        inspected_by.as_deref(),
                    )?;
                    gate::require_substantive("criterion", &criterion, "the failing serving criterion")?;
                    gate::require_substantive("evidence", &evidence, "what was found to be wrong")?;
                    let evidence = gate::compose_evidence(&evidence_locator, &evidence)?;
                    gate::require_confidence(confidence)?;

                    let edge = get_or_create_serves(&db, &persona_id, &intent_id, &now)?;
                    update_serves_issue(&db, &persona_id, &intent_id, &criterion, &evidence, confidence, &by, &now)?;
                    let updated = crate::types::ServesEdge {
                        inspection_status: "failing".to_string(),
                        criterion,
                        evidence,
                        confidence,
                        inspected_by: by.clone(),
                        last_inspected: now,
                        ..edge
                    };
                    let next_step = "`loom next --mode fix` to see failing edges.";
                    if printer.json {
                        let v = with_anchor(serde_json::to_value(&updated)?, &db, next_step)?;
                        printer.print_json(&v);
                    } else {
                        println!("✗ SERVES edge: {} → {} → failing", updated.persona_name, updated.intent_name);
                        println!("  evidence: {}", updated.evidence);
                        print_anchor(&db, next_step)?;
                    }
                }

                Some(ExploreSubCmd::Independent { notes, inspected_by }) => {
                    let by = gate::acting_in_lane(
                        "mark SERVES independent",
                        &[role::ANALYZER],
                        inspected_by.as_deref(),
                    )?;
                    let edge = get_or_create_serves(&db, &persona_id, &intent_id, &now)?;
                    update_serves_independent(&db, &persona_id, &intent_id, &notes, &by, &now)?;
                    let next_step = "`loom next` for the next item.";
                    if printer.json {
                        let v = with_anchor(serde_json::json!({
                            "edge_id": edge.id,
                            "inspection_status": "independent",
                            "notes": notes,
                        }), &db, next_step)?;
                        printer.print_json(&v);
                    } else {
                        println!("– SERVES edge: {} → {} → independent", edge.persona_name, edge.intent_name);
                        if !notes.is_empty() {
                            println!("  notes: {notes}");
                        }
                        print_anchor(&db, next_step)?;
                    }
                }
            }
        }

        // ----------------------------------------------------------------
        // loom persona journey <persona> <saga>
        // ----------------------------------------------------------------
        PersonaCmd::Journey { persona_id, saga_id } => {
            let persona_id     = resolve_persona(&db, &persona_id)?;
            let validation_id  = resolve_validation(&db, &saga_id)?;
            let now = chrono::Utc::now().to_rfc3339();

            let edge = get_or_create_journeys(&db, &persona_id, &validation_id, &now)?;
            let next_step = format!(
                "Run the saga: loom saga run {}",
                edge.validation_name
            );
            if printer.json {
                let v = with_anchor(serde_json::to_value(&edge)?, &db, &next_step)?;
                printer.print_json(&v);
            } else {
                println!("✓ JOURNEYS: {} ↪ {}", edge.persona_name, edge.validation_name);
                print_anchor(&db, &next_step)?;
            }
        }
    }
    Ok(())
}
