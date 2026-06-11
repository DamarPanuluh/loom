use anyhow::Result;
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
use crate::output::{
    apply_limit, fmt_edge_detail, fmt_edge_row, fmt_intent, more_marker, print_anchor,
    with_anchor, Printer, SECTION_CAP,
};
use crate::types::{EdgeType, Intent, RelatesTo};

pub fn run(cmd: EdgeCmd, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    match cmd {
        // ----------------------------------------------------------------
        // RELATES_TO — explore / ground / issue / independent
        // ----------------------------------------------------------------
        EdgeCmd::Explore { intent_a_id, intent_b_id, subcommand } => {
            let intent_a_id = crate::db::queries::resolve_intent(&db, &intent_a_id)?;
            let intent_b_id = crate::db::queries::resolve_intent(&db, &intent_b_id)?;
            let now = chrono::Utc::now().to_rfc3339();

            match subcommand {
                None => {
                    // Create or retrieve edge; print both intent contexts.
                    let edge_id = Uuid::new_v4().to_string();
                    let edge = get_or_create_relates_to(
                        &db, &edge_id, &intent_a_id, &intent_b_id, &now,
                    )?;
                    let intent_a = get_intent(&db, &intent_a_id)?
                        .ok_or_else(|| anyhow::anyhow!("Intent '{}' not found — the graph may be inconsistent; run `loom doctor`.", intent_a_id))?;
                    let intent_b = get_intent(&db, &intent_b_id)?
                        .ok_or_else(|| anyhow::anyhow!("Intent '{}' not found — the graph may be inconsistent; run `loom doctor`.", intent_b_id))?;

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
                    let next_step = "`loom next` for the next item.";
                    if printer.json {
                        let v = with_anchor(serde_json::to_value(&updated)?, &db, next_step)?;
                        printer.print_json(&v);
                    } else {
                        println!("✓ Edge marked as passing (grounded)");
                        println!("{}", fmt_edge_detail(&updated));
                        print_anchor(&db, next_step)?;
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
                    let next_step = format!(
                        "fix it then `loom edge fix {}`, or `loom next --mode fix`.",
                        updated.id
                    );
                    if printer.json {
                        let v = with_anchor(serde_json::to_value(&updated)?, &db, &next_step)?;
                        printer.print_json(&v);
                    } else {
                        println!("✓ Issue recorded — edge marked as failing");
                        println!("{}", fmt_edge_detail(&updated));
                        print_anchor(&db, &next_step)?;
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

                    let next_step = "Continue discovery: `loom next`";
                    if printer.json {
                        let v = with_anchor(
                            serde_json::json!({
                                "status":  "ok",
                                "edge_id": edge.id,
                                "inspection_status": "independent",
                                "from":    intent_a_id,
                                "to":      intent_b_id,
                                "notes":   notes,
                            }),
                            &db,
                            next_step,
                        )?;
                        printer.print_json(&v);
                    } else {
                        println!(
                            "✓ Confirmed independent: {} ↔ {}  (edge id: {})",
                            intent_a_id, intent_b_id, edge.id
                        );
                        print_anchor(&db, next_step)?;
                    }
                }
            }
        }

        // ----------------------------------------------------------------
        // IMPLEMENTS: Intent → CodeFile
        // ----------------------------------------------------------------
        EdgeCmd::Implement { intent_id, codefile_id, locator, notes } => {
            gate::acting_in_lane("create an IMPLEMENTS edge", &[role::BUILDER], None)?;
            let intent_id = crate::db::queries::resolve_intent(&db, &intent_id)?;
            let now = chrono::Utc::now().to_rfc3339();
            let targets = resolve_codefiles(&db, &codefile_id)?;
            let next_step = "ground more (`loom edge implement …`) or, if the leaf is fully grounded, prove it: `loom next --mode validate`";
            if targets.len() > 1 {
                // Bulk (glob) grounding: one edge per matched registered file.
                for cf in &targets {
                    insert_implements(&db, &Uuid::new_v4().to_string(), &intent_id, &cf.id, "", &notes, &now)?;
                }
                if printer.json {
                    printer.print_json(&serde_json::json!({
                        "status": "ok", "intent_id": intent_id,
                        "grounded": targets.iter().map(|c| c.path.clone()).collect::<Vec<_>>(),
                        "count": targets.len(),
                        "next_step": next_step,
                    }));
                } else {
                    println!("✓ Grounded intent in {} registered file(s) matching '{}'.", targets.len(), codefile_id);
                    println!("  → Next: {}", next_step);
                }
            } else {
                let cf = &targets[0];
                let edge_id = Uuid::new_v4().to_string();
                insert_implements(&db, &edge_id, &intent_id, &cf.id, &locator, &notes, &now)?;
                if printer.json {
                    printer.print_json(&serde_json::json!({
                        "status":       "ok",
                        "edge_id":      edge_id,
                        "edge_type":    EdgeType::Implements.to_string(),
                        "intent_id":    intent_id,
                        "codefile_id":  cf.id,
                        "locator":      locator,
                        "next_step":    next_step,
                    }));
                } else {
                    println!("✓ IMPLEMENTS edge created  (id: {})", edge_id);
                    println!("  intent   → {}", intent_id);
                    println!("  codefile → {}{}", cf.path,
                        if locator.is_empty() { String::new() } else { format!("  @ {}", locator) });
                    println!("  → Next: {}", next_step);
                }
            }
        }

        // ----------------------------------------------------------------
        // UNIMPLEMENT: remove grounding (decomposition support)
        // ----------------------------------------------------------------
        EdgeCmd::Unimplement { intent_id, codefile_id } => {
            gate::acting_in_lane("remove an IMPLEMENTS edge", &[role::BUILDER], None)?;
            let intent_id = crate::db::queries::resolve_intent(&db, &intent_id)?;
            let targets = resolve_codefiles(&db, &codefile_id)?;
            let mut removed: Vec<String> = Vec::new();
            for cf in &targets {
                if crate::db::queries::delete_implements(&db, &intent_id, &cf.id)? {
                    removed.push(cf.path.clone());
                }
            }
            if removed.is_empty() {
                anyhow::bail!(
                    "No IMPLEMENTS edge between intent '{}' and '{}'.\n`loom codefile show <path>` lists the file's owners; `loom edge implement <intent> <path>` creates the grounding.",
                    intent_id, codefile_id
                );
            }
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status": "ok", "intent_id": intent_id, "removed": removed,
                    "next_step": "If the intent is a leaf it may be unrealized now — `loom status` will route.",
                }));
            } else {
                println!("✓ Removed {} grounding(s).", removed.len());
                println!("  → If the intent is a leaf it may be unrealized now — `loom status` will route.");
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
            let next_step = format!(
                "make the edge real with a verdict: `loom rule verdict {} {} --status <passing|failing|independent> --criterion \"<text>\" --evidence \"<text>\"`",
                rule_id, intent_id
            );
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status":    "ok",
                    "edge_id":   edge_id,
                    "edge_type": EdgeType::Governs.to_string(),
                    "rule_id":   rule_id,
                    "intent_id": intent_id,
                    "next_step": next_step,
                }));
            } else {
                println!("✓ GOVERNS edge created  (id: {})", edge_id);
                println!("  rule   → {}", rule_id);
                println!("  intent → {}", intent_id);
                println!("  → Next: {}", next_step);
            }
        }

        // ----------------------------------------------------------------
        // HIERARCHY: Intent (parent) → Intent (child)
        // ----------------------------------------------------------------
        EdgeCmd::Hierarchy { parent_id, child_id, notes } => {
            gate::acting_in_lane("create a HIERARCHY edge", &[role::BUILDER], None)?;
            let parent_id = crate::db::queries::resolve_intent(&db, &parent_id)?;
            let child_id = crate::db::queries::resolve_intent(&db, &child_id)?;
            let now = chrono::Utc::now().to_rfc3339();
            let edge_id = Uuid::new_v4().to_string();
            let n = notes.as_deref().unwrap_or("");
            insert_hierarchy(&db, &edge_id, &parent_id, &child_id, n, &now)?;
            let next_step = format!(
                "ground the child if it is a leaf (`loom edge implement {} <codefile> --locator \"<symbol>\"`), or keep decomposing",
                child_id
            );
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status":    "ok",
                    "edge_id":   edge_id,
                    "edge_type": EdgeType::Hierarchy.to_string(),
                    "parent_id": parent_id,
                    "child_id":  child_id,
                    "next_step": next_step,
                }));
            } else {
                println!("✓ HIERARCHY edge created  (id: {})", edge_id);
                println!("  parent → {}", parent_id);
                println!("  child  → {}", child_id);
                println!("  → Next: {}", next_step);
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
            // Validations resolve by id, exact name, or unique fragment —
            // same addressability contract as intents and rules.
            let validation_id = crate::db::queries::resolve_validation(&db, &validation_id)?;
            let intent_id = crate::db::queries::resolve_intent(&db, &intent_id)?;
            let now = chrono::Utc::now().to_rfc3339();
            let edge_id = Uuid::new_v4().to_string();
            let n = notes.as_deref().unwrap_or("");
            insert_validates(&db, &edge_id, &validation_id, &intent_id, n, &now)?;
            let next_step = format!("make the proof real: `loom validate {}`", intent_id);
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status":        "ok",
                    "edge_id":       edge_id,
                    "edge_type":     EdgeType::Validates.to_string(),
                    "validation_id": validation_id,
                    "intent_id":     intent_id,
                    "next_step":     next_step,
                }));
            } else {
                println!("✓ VALIDATES edge created  (id: {})", edge_id);
                println!("  validation → {}", validation_id);
                println!("  intent     → {}", intent_id);
                println!("  → Next: {}", next_step);
            }
        }

        // ----------------------------------------------------------------
        // List RELATES_TO edges
        // ----------------------------------------------------------------
        EdgeCmd::List { status, limit } => {
            let mut edges = list_relates_to(&db, status.as_deref())?;
            let total = apply_limit(&mut edges, limit);
            let shown = edges.len();
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "edges":     edges,
                    "total":     total,
                    "truncated": shown < total,
                }));
            } else if edges.is_empty() {
                println!("(no RELATES_TO edges found)");
            } else {
                for e in &edges {
                    println!("{}", fmt_edge_row(e));
                }
                if let Some(m) = more_marker(total, shown, "loom edge list --limit 0") {
                    println!("{}", m);
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
                    let mut notes = notes_for_target(&db, &e.id)?;
                    // Notes come back oldest-first; keep the NEWEST when capping.
                    let notes_total = notes.len();
                    if notes_total > SECTION_CAP {
                        notes.drain(..notes_total - SECTION_CAP);
                    }
                    if printer.json {
                        printer.print_json(&serde_json::json!({
                            "edge":     e,
                            "intent_a": intent_a,
                            "intent_b": intent_b,
                            "notes":    notes,
                            "notes_total": notes_total,
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
                        println!("── Notes ({}) ──────────────────────────────────────────────────────", notes_total);
                        if notes.is_empty() {
                            println!("  (none)");
                        } else {
                            for n in &notes {
                                println!("  [{}] {}  ({})", n.kind, n.text, n.author);
                            }
                            if let Some(m) = more_marker(notes_total, notes.len(), &format!("loom note list --edge {}", e.id)) {
                                println!("  {}", m);
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
            crate::db::queries::ensure_owned(
                &db, "mark an edge fixed (a claim that you changed the code)",
            )?;
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
            let next_step =
                "Neighbouring passing/independent edges set to needs_reverification — `loom next --mode fix`.";
            if printer.json {
                let v = with_anchor(
                    serde_json::json!({
                        "status":      "ok",
                        "edge_id":     edge_id,
                        "description": description,
                        "message":     "Edge marked passing. Neighbouring passing/independent edges set to needs_reverification.",
                    }),
                    &db,
                    next_step,
                )?;
                printer.print_json(&v);
            } else {
                println!("✓ Edge {} marked as passing (fixed)", edge_id);
                print_anchor(&db, next_step)?;
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve a codefile argument: exact id, exact registered path, or a glob
/// over REGISTERED paths (bulk). Errors when nothing matches.
fn resolve_codefiles(db: &GrafeoDb, key: &str) -> Result<Vec<crate::types::CodeFile>> {
    let is_glob = key.contains('*') || key.contains('?') || key.contains('[');
    if is_glob {
        let pat = glob::Pattern::new(key)
            .map_err(|e| anyhow::anyhow!("Invalid glob '{}': {} — quote it: `loom codefile add 'src/**/*.rs'`", key, e))?;
        let matched: Vec<_> = crate::db::queries::list_codefiles(db)?
            .into_iter()
            .filter(|c| pat.matches(&c.path))
            .collect();
        if matched.is_empty() {
            anyhow::bail!(
                "No REGISTERED codefile matches glob '{}'. Register first: loom codefile add '{}'",
                key, key
            );
        }
        return Ok(matched);
    }
    crate::db::queries::get_codefile_by_id_or_path(db, key)?
        .map(|c| vec![c])
        .ok_or_else(|| anyhow::anyhow!(
            "CodeFile '{}' not found (by id or path).\nRegister it first: loom codefile add <path>",
            key
        ))
}

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
        tags:              String::new(),
        lifecycle:         String::new(),
        created_at:        String::new(),
        updated_at:        String::new(),
    }
}
