use anyhow::Result;
use std::env;
use uuid::Uuid;

use crate::cli::{EdgeCmd, ExploreSubCmd};
use crate::db::{ensure_initialized, GrafeoDb};
use crate::db::queries::{
    fix_edge, get_intent, get_or_create_relates_to, get_relates_to,
    insert_governs, insert_hierarchy, insert_implements,
    insert_validates, list_relates_to, notes_for_target,
    update_relates_to_ground, update_relates_to_independent, update_relates_to_issue,
};
use crate::db::schema::role;
use crate::gate;
use crate::output::{fmt_edge_detail, fmt_edge_row, fmt_intent, Printer};
use crate::types::{EdgeType, Intent, RelatesTo};

pub fn run(cmd: EdgeCmd, printer: &Printer) -> Result<()> {
    let cwd = env::current_dir()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    match cmd {
        // ----------------------------------------------------------------
        // RELATES_TO — explore / ground / issue / independent
        // ----------------------------------------------------------------
        EdgeCmd::Explore { intent_a_id, intent_b_id, subcommand } => {
            let now = chrono::Utc::now().to_rfc3339();

            match subcommand {
                None => {
                    // Create or retrieve edge; print both intent contexts.
                    let edge_id = Uuid::new_v4().to_string();
                    let edge = get_or_create_relates_to(
                        &db, &edge_id, &intent_a_id, &intent_b_id, &now,
                    )?;
                    let intent_a = get_intent(&db, &intent_a_id)?
                        .ok_or_else(|| anyhow::anyhow!("Intent '{}' not found", intent_a_id))?;
                    let intent_b = get_intent(&db, &intent_b_id)?
                        .ok_or_else(|| anyhow::anyhow!("Intent '{}' not found", intent_b_id))?;

                    if printer.json {
                        printer.print_json(&serde_json::json!({
                            "edge":     edge,
                            "intent_a": intent_a,
                            "intent_b": intent_b,
                        }));
                    } else {
                        println!("── Intent A ──────────────────────────────────────────────────────");
                        println!("{}", fmt_intent(&intent_a));
                        println!();
                        println!("── Intent B ──────────────────────────────────────────────────────");
                        println!("{}", fmt_intent(&intent_b));
                        println!();
                        println!("── Edge ──────────────────────────────────────────────────────────");
                        println!("{}", fmt_edge_detail(&edge));
                        println!();
                        println!("Next steps:");
                        println!(
                            "  loom edge explore {a} {b} ground --criterion \"<text>\" --confidence 0.9",
                            a = intent_a_id, b = intent_b_id
                        );
                        println!(
                            "  loom edge explore {a} {b} issue  --criterion \"<text>\" --evidence \"<text>\"",
                            a = intent_a_id, b = intent_b_id
                        );
                        println!(
                            "  loom edge explore {a} {b} independent --notes \"<why no relationship>\"",
                            a = intent_a_id, b = intent_b_id
                        );
                    }
                }

                Some(ExploreSubCmd::Ground { criterion, confidence, inspected_by }) => {
                    let now = chrono::Utc::now().to_rfc3339();
                    // Grounding is inspection work: analyzer lane (fixer too —
                    // it re-grounds edges it has just repaired).
                    let by = gate::acting_in_lane(
                        "ground a RELATES_TO edge",
                        &[role::ANALYZER, role::FIXER],
                        inspected_by.as_deref(),
                    )?;
                    gate::require_substantive(
                        "criterion", &criterion,
                        "the falsifiable coexistence criterion this edge was checked against",
                    )?;
                    gate::require_confidence(confidence)?;
                    let by = by.as_str();
                    // Create the edge if it does not exist yet, so a discovery
                    // suggestion (`explore A B ground ...`) works in one step —
                    // consistent with the `independent` subcommand.
                    let edge_id = Uuid::new_v4().to_string();
                    let edge = get_or_create_relates_to(&db, &edge_id, &intent_a_id, &intent_b_id, &now)?;
                    update_relates_to_ground(&db, &edge.from_id, &edge.to_id, &criterion, confidence, by, &now)?;
                    // Construct the result from the values we just wrote rather than
                    // re-reading the relationship — grafeo 0.5.x does not reliably
                    // return a relationship by property immediately after mutating it
                    // in the same session.
                    let updated = RelatesTo {
                        inspection_status: "passing".to_string(),
                        criterion,
                        confidence,
                        inspected_by: by.to_string(),
                        last_inspected: now,
                        ..edge
                    };
                    if printer.json {
                        printer.print_json(&updated);
                    } else {
                        println!("✓ Edge marked as passing (grounded)");
                        println!("{}", fmt_edge_detail(&updated));
                        println!("  → Next: `loom next` for the next item.");
                    }
                }

                Some(ExploreSubCmd::Issue { criterion, evidence, confidence, inspected_by }) => {
                    let now = chrono::Utc::now().to_rfc3339();
                    let by = gate::acting_in_lane(
                        "record an issue on a RELATES_TO edge",
                        &[role::ANALYZER, role::FIXER],
                        inspected_by.as_deref(),
                    )?;
                    gate::require_substantive(
                        "criterion", &criterion,
                        "the falsifiable criterion that was violated",
                    )?;
                    gate::require_substantive(
                        "evidence", &evidence,
                        "what was actually found in the code (file/symbol + the problem)",
                    )?;
                    gate::require_confidence(confidence)?;
                    let by = by.as_str();
                    let edge_id = Uuid::new_v4().to_string();
                    let edge = get_or_create_relates_to(&db, &edge_id, &intent_a_id, &intent_b_id, &now)?;
                    update_relates_to_issue(&db, &edge.from_id, &edge.to_id, &criterion, &evidence, confidence, by, &now)?;
                    // See note in the Ground arm: construct rather than re-read.
                    let updated = RelatesTo {
                        inspection_status: "failing".to_string(),
                        criterion,
                        evidence,
                        confidence,
                        inspected_by: by.to_string(),
                        last_inspected: now,
                        ..edge
                    };
                    if printer.json {
                        printer.print_json(&updated);
                    } else {
                        println!("✓ Issue recorded — edge marked as failing");
                        println!("{}", fmt_edge_detail(&updated));
                        println!("  → Next: fix it then `loom edge fix {}`, or `loom next --mode fix`.", updated.id);
                    }
                }

                Some(ExploreSubCmd::Independent { notes, inspected_by }) => {
                    // independent is now a status on the RELATES_TO edge, not a separate edge type
                    let now = chrono::Utc::now().to_rfc3339();
                    let by = gate::acting_in_lane(
                        "confirm two intents independent",
                        &[role::ANALYZER],
                        inspected_by.as_deref(),
                    )?;
                    // Independence is a *verified claim*, as strong as passing —
                    // it must record why no relationship exists.
                    gate::require_substantive(
                        "notes", &notes,
                        "why these two intents have no meaningful relationship",
                    )?;
                    let by = by.as_str();

                    // Ensure RELATES_TO edge exists first
                    let edge_id = Uuid::new_v4().to_string();
                    let edge = get_or_create_relates_to(
                        &db, &edge_id, &intent_a_id, &intent_b_id, &now,
                    )?;
                    update_relates_to_independent(&db, &edge.from_id, &edge.to_id, &notes, by, &now)?;

                    if printer.json {
                        printer.print_json(&serde_json::json!({
                            "status":  "ok",
                            "edge_id": edge.id,
                            "inspection_status": "independent",
                            "from":    intent_a_id,
                            "to":      intent_b_id,
                            "notes":   notes,
                        }));
                    } else {
                        println!(
                            "✓ Confirmed independent: {} ↔ {}  (edge id: {})",
                            intent_a_id, intent_b_id, edge.id
                        );
                    }
                }
            }
        }

        // ----------------------------------------------------------------
        // IMPLEMENTS: Intent → CodeFile
        // ----------------------------------------------------------------
        EdgeCmd::Implement { intent_id, codefile_id, locator, notes } => {
            gate::acting_in_lane("create an IMPLEMENTS edge", &[role::BUILDER], None)?;
            // Accept a path as well as an id — the path is the natural key a
            // driver already has in hand (dogfood finding: id-only forced a
            // `codefile list` + lookup round-trip per grounding).
            let cf = crate::db::queries::get_codefile_by_id_or_path(&db, &codefile_id)?
                .ok_or_else(|| anyhow::anyhow!(
                    "CodeFile '{}' not found (by id or path).\nRegister it first: loom codefile add <path>",
                    codefile_id
                ))?;
            let codefile_id = cf.id;
            let now = chrono::Utc::now().to_rfc3339();
            let edge_id = Uuid::new_v4().to_string();
            insert_implements(&db, &edge_id, &intent_id, &codefile_id, &locator, &notes, &now)?;
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status":       "ok",
                    "edge_id":      edge_id,
                    "edge_type":    EdgeType::Implements.to_string(),
                    "intent_id":    intent_id,
                    "codefile_id":  codefile_id,
                    "locator":      locator,
                }));
            } else {
                println!("✓ IMPLEMENTS edge created  (id: {})", edge_id);
                println!("  intent   → {}", intent_id);
                println!("  codefile → {}{}", codefile_id,
                    if locator.is_empty() { String::new() } else { format!("  @ {}", locator) });
            }
        }

        // ----------------------------------------------------------------
        // GOVERNS: QualityRule → Intent
        // ----------------------------------------------------------------
        EdgeCmd::Govern { rule_id, intent_id, criterion } => {
            gate::acting_in_lane("apply a quality rule (GOVERNS)", &[role::QUALITY], None)?;
            let now = chrono::Utc::now().to_rfc3339();
            let edge_id = Uuid::new_v4().to_string();
            let crit = criterion.as_deref().unwrap_or("");
            if !crit.is_empty() {
                gate::require_substantive(
                    "criterion", crit,
                    "what compliance looks like for this rule on this intent",
                )?;
            }
            insert_governs(&db, &edge_id, &rule_id, &intent_id, crit, &now)?;
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status":    "ok",
                    "edge_id":   edge_id,
                    "edge_type": EdgeType::Governs.to_string(),
                    "rule_id":   rule_id,
                    "intent_id": intent_id,
                }));
            } else {
                println!("✓ GOVERNS edge created  (id: {})", edge_id);
                println!("  rule   → {}", rule_id);
                println!("  intent → {}", intent_id);
            }
        }

        // ----------------------------------------------------------------
        // HIERARCHY: Intent (parent) → Intent (child)
        // ----------------------------------------------------------------
        EdgeCmd::Hierarchy { parent_id, child_id, notes } => {
            gate::acting_in_lane("create a HIERARCHY edge", &[role::BUILDER], None)?;
            let now = chrono::Utc::now().to_rfc3339();
            let edge_id = Uuid::new_v4().to_string();
            let n = notes.as_deref().unwrap_or("");
            insert_hierarchy(&db, &edge_id, &parent_id, &child_id, n, &now)?;
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status":    "ok",
                    "edge_id":   edge_id,
                    "edge_type": EdgeType::Hierarchy.to_string(),
                    "parent_id": parent_id,
                    "child_id":  child_id,
                }));
            } else {
                println!("✓ HIERARCHY edge created  (id: {})", edge_id);
                println!("  parent → {}", parent_id);
                println!("  child  → {}", child_id);
            }
        }

        // ----------------------------------------------------------------
        // VALIDATES: Validation → Intent
        // ----------------------------------------------------------------
        EdgeCmd::Validates { validation_id, intent_id, notes } => {
            gate::acting_in_lane(
                "link a validation (VALIDATES)",
                &[role::BUILDER, role::VALIDATOR],
                None,
            )?;
            let now = chrono::Utc::now().to_rfc3339();
            let edge_id = Uuid::new_v4().to_string();
            let n = notes.as_deref().unwrap_or("");
            insert_validates(&db, &edge_id, &validation_id, &intent_id, n, &now)?;
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status":        "ok",
                    "edge_id":       edge_id,
                    "edge_type":     EdgeType::Validates.to_string(),
                    "validation_id": validation_id,
                    "intent_id":     intent_id,
                }));
            } else {
                println!("✓ VALIDATES edge created  (id: {})", edge_id);
                println!("  validation → {}", validation_id);
                println!("  intent     → {}", intent_id);
            }
        }

        // ----------------------------------------------------------------
        // List RELATES_TO edges
        // ----------------------------------------------------------------
        EdgeCmd::List { status } => {
            let edges = list_relates_to(&db, status.as_deref())?;
            if printer.json {
                printer.print_json(&edges);
            } else if edges.is_empty() {
                println!("(no RELATES_TO edges found)");
            } else {
                for e in &edges {
                    println!("{}", fmt_edge_row(e));
                }
            }
        }

        // ----------------------------------------------------------------
        // Show full detail of one RELATES_TO edge
        // ----------------------------------------------------------------
        EdgeCmd::Show { edge_id } => {
            let edge = get_relates_to(&db, &edge_id)?;
            match edge {
                None => anyhow::bail!(
                    "Edge '{}' not found.\nRun `loom edge list` to see available edges.",
                    edge_id
                ),
                Some(ref e) => {
                    let intent_a = get_intent(&db, &e.from_id)?
                        .unwrap_or_else(|| default_intent(&e.from_id));
                    let intent_b = get_intent(&db, &e.to_id)?
                        .unwrap_or_else(|| default_intent(&e.to_id));
                    let notes = notes_for_target(&db, &e.id)?;
                    if printer.json {
                        printer.print_json(&serde_json::json!({
                            "edge":     e,
                            "intent_a": intent_a,
                            "intent_b": intent_b,
                            "notes":    notes,
                        }));
                    } else {
                        println!("── Edge ──────────────────────────────────────────────────────────");
                        println!("{}", fmt_edge_detail(e));
                        println!();
                        println!(
                            "── Intent A ({}) ──────────────────────────────────────────────────",
                            e.from_name
                        );
                        println!("{}", fmt_intent(&intent_a));
                        println!();
                        println!(
                            "── Intent B ({}) ──────────────────────────────────────────────────",
                            e.to_name
                        );
                        println!("{}", fmt_intent(&intent_b));
                        println!();
                        println!("── Notes ({}) ──────────────────────────────────────────────────────", notes.len());
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

        // ----------------------------------------------------------------
        // Fix a failing RELATES_TO edge → sets inspection_status = passing
        // ----------------------------------------------------------------
        EdgeCmd::Fix { edge_id, description } => {
            let by = gate::acting_in_lane("mark a failing edge fixed", &[role::FIXER], None)?;
            gate::require_substantive(
                "description", &description,
                "what was changed in the code to resolve the violation",
            )?;
            let now = chrono::Utc::now().to_rfc3339();
            let found = fix_edge(&db, &edge_id, &description, &by, &now)?;
            if !found {
                anyhow::bail!(
                    "Edge '{}' not found.\nRun `loom edge list` to see available edges.",
                    edge_id
                );
            }
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status":      "ok",
                    "edge_id":     edge_id,
                    "description": description,
                    "message":     "Edge marked passing. Neighbouring passing/independent edges set to needs_reverification.",
                }));
            } else {
                println!("✓ Edge {} marked as passing (fixed)", edge_id);
                println!("  Neighbouring passing/independent edges set to needs_reverification.");
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn default_intent(id: &str) -> Intent {
    Intent {
        id:                id.to_string(),
        name:              "(unknown)".to_string(),
        description:       String::new(),
        abstraction_level: String::new(),
        domain:            String::new(),
        source_refs:       "[]".to_string(),
        status:            String::new(),
        aspect:            String::new(),
        lifecycle:         String::new(),
        created_at:        String::new(),
        updated_at:        String::new(),
    }
}
