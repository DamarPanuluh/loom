//! `loom persona` — manage personas and their SERVES/JOURNEYS edges.

use anyhow::Result;
use uuid::Uuid;

use crate::cli::{ExploreSubCmd, PersonaCmd};
use crate::commands::resolve::{resolve_intent_with_db, resolve_validation_with_db};
use crate::db::{ensure_initialized, GraphReadHandle, GraphReadRepository};
use crate::gate;
use crate::output::{apply_limit, fmt_pulse, with_read_anchor, Printer};
use crate::types::Persona;

/// `ExploreSubCmd` is shared with `loom edge`, which carries relationship
/// `--kind`s. SERVES (Persona→Intent) has no relationship-kind taxonomy, so
/// reject the flag here rather than silently ignore it.
fn reject_relationship_kinds(kinds: &[String]) -> Result<()> {
    if !kinds.is_empty() {
        anyhow::bail!(
            "--kind is a RELATES_TO relationship kind (use `loom edge explore … ground --kind`); \
             SERVES edges carry no relationship kind."
        );
    }
    Ok(())
}

pub fn run(cmd: PersonaCmd, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    ensure_initialized(&cwd)?;
    match cmd {
        PersonaCmd::List { limit } => {
            let db = GraphReadHandle::open(&cwd)?;
            run_list_with_db(&db, limit, printer)
        }
        PersonaCmd::Show { id } => {
            let db = GraphReadHandle::open(&cwd)?;
            run_show_with_db(&db, id, printer)
        }
        PersonaCmd::Add {
            name,
            description,
            author,
        } => run_add_with_sqlite(&cwd, name, description, author, printer),
        PersonaCmd::Serve {
            persona_id,
            intent_id,
            subcommand,
        } => run_serve_with_sqlite(&cwd, persona_id, intent_id, subcommand, printer),
        PersonaCmd::Journey {
            persona_id,
            saga_id,
        } => run_journey_with_sqlite(&cwd, persona_id, saga_id, printer),
        PersonaCmd::Remove { id } => run_remove_with_sqlite(&cwd, id, printer),
    }
}

fn run_remove_with_sqlite(cwd: &std::path::Path, key: String, printer: &Printer) -> Result<()> {
    let mut store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(cwd))?;
    store.ensure_owned("remove a persona")?;
    let persona = resolve_persona_with_db(&store, &key)?;
    let removed = store.delete_persona(&persona.id)?;
    if !removed {
        anyhow::bail!("Persona '{key}' could not be removed (already gone?).");
    }
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "removed": { "id": persona.id, "name": persona.name },
            "next_step": "loom persona list",
        }));
        return Ok(());
    }
    println!(
        "✓ Removed persona '{}' (its SERVES + JOURNEYS edges went with it).",
        persona.name
    );
    Ok(())
}

fn run_list_with_db(db: &dyn GraphReadRepository, limit: usize, printer: &Printer) -> Result<()> {
    let mut personas = db.list_personas()?;
    let total = apply_limit(&mut personas, limit);
    // An ORPHAN persona has no SERVES and no JOURNEYS — dead weight an agent
    // should remove (`loom persona remove`). Surfacing it makes the stale
    // persona DETECTABLE (the removal half already shipped). Bounded: personas
    // is already limited.
    let mut orphans: Vec<String> = Vec::new();
    for p in &personas {
        if db.list_serves_for_persona(&p.id)?.is_empty()
            && db.list_journeys_for_persona(&p.id)?.is_empty()
        {
            orphans.push(p.id.clone());
        }
    }
    if printer.json {
        printer.print_json(&serde_json::json!({
            "personas": personas,
            "total": total,
            "truncated": total > personas.len(),
            "orphans": orphans,
        }));
    } else if personas.is_empty() {
        println!("(no personas — add one: loom persona add --name <name> --description \"<who they are>\")");
    } else {
        for p in &personas {
            let mark = if orphans.contains(&p.id) {
                "   ⚠ orphan (no SERVES/JOURNEYS)"
            } else {
                ""
            };
            println!("  {:<36}  {}{}", p.id, p.name, mark);
            println!("    {}", p.description);
        }
        if let Some(m) =
            crate::output::more_marker(total, personas.len(), "loom persona list --limit 0")
        {
            println!("  {m}");
        }
        if !orphans.is_empty() {
            println!(
                "  ⚠ {} orphan persona(s) — no SERVES/JOURNEYS; remove with `loom persona remove <id>`.",
                orphans.len()
            );
        }
        println!("\n  → loom persona show <id>   to see a persona's SERVES edges");
    }
    Ok(())
}

fn run_show_with_db(db: &dyn GraphReadRepository, id: String, printer: &Printer) -> Result<()> {
    let persona = resolve_persona_with_db(db, &id)?;
    // Bound the sub-sections (invariant 3): SERVES is many-to-many, so a
    // central persona can flood context. Cap each at SECTION_CAP and report
    // the true *_total, matching `loom hypothesis show`.
    let mut serves = db.list_serves_for_persona(&persona.id)?;
    let serves_total = crate::output::apply_limit(&mut serves, crate::output::SECTION_CAP);
    let mut journeys = db.list_journeys_for_persona(&persona.id)?;
    let journeys_total = crate::output::apply_limit(&mut journeys, crate::output::SECTION_CAP);
    let fetch = format!("`loom persona show {} --json`", persona.id);

    if printer.json {
        printer.print_json(&serde_json::json!({
            "persona": persona,
            "serves": serves,
            "serves_total": serves_total,
            "journeys": journeys,
            "journeys_total": journeys_total,
        }));
    } else {
        println!("── Persona ────────────────────────────────────────────────────────");
        println!("  Name:   {}", persona.name);
        println!("  ID:     {}", persona.id);
        println!("  \"{}\"", persona.description);
        println!("  Added:  {}", persona.created_at);

        println!();
        println!(
            "── SERVES ({} intent{}) ──────────────────────────────────────────",
            serves_total,
            if serves_total == 1 { "" } else { "s" }
        );
        if serves.is_empty() {
            println!(
                "  (none — link intents: loom persona serve {} <intent>)",
                persona.id
            );
        } else {
            for e in &serves {
                let icon = match e.inspection_status.as_str() {
                    "passing" => "✓",
                    "failing" => "✗",
                    "independent" => "–",
                    "needs_reverification" => "~",
                    _ => "?",
                };
                println!("  {icon} [{}]  {}", e.inspection_status, e.intent_name);
                if !e.criterion.is_empty() {
                    println!("      criterion: {}", e.criterion);
                }
            }
            if let Some(m) = crate::output::more_marker(serves_total, serves.len(), &fetch) {
                println!("  {m}");
            }
        }

        println!();
        println!(
            "── JOURNEYS ({} saga{}) ──────────────────────────────────────────",
            journeys_total,
            if journeys_total == 1 { "" } else { "s" }
        );
        if journeys.is_empty() {
            println!(
                "  (none — link a saga: loom persona journey {} <saga-validation-id>)",
                persona.id
            );
        } else {
            for j in &journeys {
                println!("  ↪ {}", j.validation_name);
            }
            if let Some(m) = crate::output::more_marker(journeys_total, journeys.len(), &fetch) {
                println!("  {m}");
            }
        }

        println!();
        println!(
            "  → loom persona serve {} <intent>   inspect/verify a SERVES edge",
            persona.id
        );
    }
    Ok(())
}

fn run_add_with_sqlite(
    root: &std::path::Path,
    name: String,
    description: String,
    author: Option<String>,
    printer: &Printer,
) -> Result<()> {
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let persona = prepare_add_persona(&store, name, description, author)?;
    store.insert_persona(&persona)?;
    print_add_result(&store, &persona, printer)
}

fn prepare_add_persona(
    db: &dyn GraphReadRepository,
    name: String,
    description: String,
    author: Option<String>,
) -> Result<Persona> {
    let agent = gate::acting_in_lane(&gate::lane::ADD_PERSONA, author.as_deref())?;
    gate::require_substantive(
        "description",
        &description,
        "who this persona is and what distinguishes them from other audience segments",
    )?;

    // Duplicate name guard. This intentionally preserves the old get_persona
    // behavior: exact id/name first, then unique name fragment.
    if let Some(existing) = find_persona_with_db(db, &name)? {
        anyhow::bail!(
            "Persona '{}' already exists (id: {}).\n  \
             Use `loom persona show {}` to see its SERVES edges.",
            existing.name,
            existing.id,
            existing.id
        );
    }

    let now = chrono::Utc::now().to_rfc3339();
    Ok(Persona {
        id: Uuid::new_v4().to_string(),
        name,
        description,
        author: agent,
        created_at: now.clone(),
        updated_at: now,
    })
}

fn print_add_result(
    db: &dyn GraphReadRepository,
    persona: &Persona,
    printer: &Printer,
) -> Result<()> {
    let next_step = format!(
        "Link intents this persona relies on: loom persona serve {} <intent>",
        persona.id
    );
    if printer.json {
        let v = with_read_anchor(serde_json::to_value(persona)?, db, &next_step)?;
        printer.print_json(&v);
    } else {
        println!("✓ Persona added: {} ({})", persona.name, persona.id);
        println!("  \"{}\"", persona.description);
        let snapshot = db.query_snapshot()?;
        println!("  → Next: {next_step}");
        println!("  {}", fmt_pulse(&db.graph_state(&snapshot)?));
    }
    Ok(())
}

fn resolve_persona_with_db(db: &dyn GraphReadRepository, key: &str) -> Result<Persona> {
    match find_persona_with_db(db, key)? {
        Some(persona) => Ok(persona),
        None => anyhow::bail!(
            "Persona '{}' not found.\n  Run `loom persona list` to see registered personas.",
            key
        ),
    }
}

fn find_persona_with_db(db: &dyn GraphReadRepository, key: &str) -> Result<Option<Persona>> {
    let personas = db.list_personas()?;
    if let Some(persona) = personas.iter().find(|persona| persona.id == key) {
        return Ok(Some(persona.clone()));
    }
    if let Some(persona) = personas.iter().find(|persona| persona.name == key) {
        return Ok(Some(persona.clone()));
    }
    let lower = key.to_lowercase();
    let hits: Vec<Persona> = personas
        .into_iter()
        .filter(|persona| persona.name.to_lowercase().contains(&lower))
        .collect();
    match hits.len() {
        0 => Ok(None),
        1 => Ok(Some(hits[0].clone())),
        _ => anyhow::bail!(
            "Ambiguous persona fragment '{}' matches {} personas — use more of the name or the exact id.\n  {}",
            key,
            hits.len(),
            hits
                .iter()
                .map(|persona| format!("{} ({})", persona.name, persona.id))
                .collect::<Vec<_>>()
                .join("\n  ")
        ),
    }
}

fn run_serve_with_sqlite(
    root: &std::path::Path,
    persona_key: String,
    intent_key: String,
    subcommand: Option<ExploreSubCmd>,
    printer: &Printer,
) -> Result<()> {
    let mut store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let persona = resolve_persona_with_db(&store, &persona_key)?;
    let persona_id = persona.id.clone();
    let intent_id = resolve_intent_with_db(&store, &intent_key)?;
    let now = chrono::Utc::now().to_rfc3339();

    match subcommand {
        None => {
            let edge = store.get_or_create_serves(&persona_id, &intent_id, &now)?;
            let intent = store
                .get_intent(&intent_id)?
                .ok_or_else(|| anyhow::anyhow!("Intent '{}' not found.", intent_id))?;
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "edge": edge,
                    "persona": persona,
                    "intent": intent,
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
                    p = persona_id,
                    i = intent_id
                );
                println!(
                    "  loom persona serve {p} {i} issue  --criterion \"<test>\" --evidence \"<what's wrong>\"",
                    p = persona_id,
                    i = intent_id
                );
                println!(
                    "  loom persona serve {p} {i} independent --notes \"<why this intent doesn't serve this persona>\"",
                    p = persona_id,
                    i = intent_id
                );
            }
        }
        Some(ExploreSubCmd::Ground {
            criterion,
            evidence,
            evidence_locator,
            confidence,
            kinds,
            inspected_by,
        }) => {
            reject_relationship_kinds(&kinds)?;
            let by = gate::acting_in_lane(&gate::lane::GROUND_SERVES, inspected_by.as_deref())?;
            gate::require_substantive(
                "criterion",
                &criterion,
                "what 'serving this persona' looks like for this intent",
            )?;
            if !evidence.trim().is_empty() {
                gate::require_substantive("evidence", &evidence, "what was found")?;
            }
            gate::require_locators_resolve(root, &evidence_locator)?;
            let evidence = gate::compose_evidence(&evidence_locator, &evidence)?;
            gate::require_confidence(confidence)?;

            let edge = store.get_or_create_serves(&persona_id, &intent_id, &now)?;
            store.update_serves_ground(
                &persona_id,
                &intent_id,
                &criterion,
                &evidence,
                confidence,
                &by,
                &now,
            )?;
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
                let v = with_read_anchor(serde_json::to_value(&updated)?, &store, next_step)?;
                printer.print_json(&v);
            } else {
                println!(
                    "✓ SERVES edge: {} → {} → passing",
                    updated.persona_name, updated.intent_name
                );
                if !updated.criterion.is_empty() {
                    println!("  criterion: {}", updated.criterion);
                }
                let snapshot = store.query_snapshot()?;
                println!("  → Next: {next_step}");
                println!("  {}", fmt_pulse(&store.graph_state(&snapshot)?));
            }
        }
        Some(ExploreSubCmd::Issue {
            criterion,
            evidence,
            evidence_locator,
            confidence,
            kinds,
            inspected_by,
        }) => {
            reject_relationship_kinds(&kinds)?;
            let by = gate::acting_in_lane(&gate::lane::ISSUE_SERVES, inspected_by.as_deref())?;
            gate::require_substantive("criterion", &criterion, "the failing serving criterion")?;
            gate::require_substantive("evidence", &evidence, "what was found to be wrong")?;
            gate::require_locators_resolve(root, &evidence_locator)?;
            let evidence = gate::compose_evidence(&evidence_locator, &evidence)?;
            gate::require_confidence(confidence)?;

            let edge = store.get_or_create_serves(&persona_id, &intent_id, &now)?;
            store.update_serves_issue(
                &persona_id,
                &intent_id,
                &criterion,
                &evidence,
                confidence,
                &by,
                &now,
            )?;
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
                let v = with_read_anchor(serde_json::to_value(&updated)?, &store, next_step)?;
                printer.print_json(&v);
            } else {
                println!(
                    "✗ SERVES edge: {} → {} → failing",
                    updated.persona_name, updated.intent_name
                );
                println!("  evidence: {}", updated.evidence);
                let snapshot = store.query_snapshot()?;
                println!("  → Next: {next_step}");
                println!("  {}", fmt_pulse(&store.graph_state(&snapshot)?));
            }
        }
        Some(ExploreSubCmd::Independent {
            notes,
            inspected_by,
        }) => {
            let by =
                gate::acting_in_lane(&gate::lane::INDEPENDENT_SERVES, inspected_by.as_deref())?;
            let edge = store.get_or_create_serves(&persona_id, &intent_id, &now)?;
            store.update_serves_independent(&persona_id, &intent_id, &notes, &by, &now)?;
            let next_step = "`loom next` for the next item.";
            if printer.json {
                let v = with_read_anchor(
                    serde_json::json!({
                        "edge_id": edge.id,
                        "inspection_status": "independent",
                        "notes": notes,
                    }),
                    &store,
                    next_step,
                )?;
                printer.print_json(&v);
            } else {
                println!(
                    "– SERVES edge: {} → {} → independent",
                    edge.persona_name, edge.intent_name
                );
                if !notes.is_empty() {
                    println!("  notes: {notes}");
                }
                let snapshot = store.query_snapshot()?;
                println!("  → Next: {next_step}");
                println!("  {}", fmt_pulse(&store.graph_state(&snapshot)?));
            }
        }
    }
    Ok(())
}

fn run_journey_with_sqlite(
    root: &std::path::Path,
    persona_key: String,
    saga_key: String,
    printer: &Printer,
) -> Result<()> {
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let persona_id = resolve_persona_with_db(&store, &persona_key)?.id;
    let validation_id = resolve_validation_with_db(&store, &saga_key)?;
    let now = chrono::Utc::now().to_rfc3339();
    let edge = store.get_or_create_journeys(&persona_id, &validation_id, &now)?;
    let next_step = format!(
        "Diagnose if needed: loom saga diagnose {}; stamp proof: loom saga run {}",
        edge.validation_name, edge.validation_name
    );
    if printer.json {
        let v = with_read_anchor(serde_json::to_value(&edge)?, &store, &next_step)?;
        printer.print_json(&v);
    } else {
        println!(
            "✓ JOURNEYS: {} ↪ {}",
            edge.persona_name, edge.validation_name
        );
        let snapshot = store.query_snapshot()?;
        println!("  → Next: {next_step}");
        println!("  {}", fmt_pulse(&store.graph_state(&snapshot)?));
    }
    Ok(())
}
