use anyhow::Result;

use crate::cli::{EdgeCmd, ExploreSubCmd};
use crate::db::{ensure_initialized, GraphReadRepository};
use crate::gate;
use crate::output::{
    apply_limit, fmt_edge_detail, fmt_edge_row, fmt_intent, fmt_pulse, more_marker,
    with_read_anchor, Printer, SECTION_CAP,
};
use crate::types::{CodeFile, EdgeType, Intent, RelatesTo};

pub fn run(cmd: EdgeCmd, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    ensure_initialized(&cwd)?;
    match cmd {
        EdgeCmd::Explore {
            intent_a_id,
            intent_b_id,
            subcommand,
        } => run_explore_with_sqlite(&cwd, intent_a_id, intent_b_id, subcommand, printer),
        EdgeCmd::Implement {
            intent_id,
            codefile_id,
            locator,
            notes,
        } => run_implement_with_sqlite(&cwd, intent_id, codefile_id, locator, notes, printer),
        EdgeCmd::Unimplement {
            intent_id,
            codefile_id,
        } => run_unimplement_with_sqlite(&cwd, intent_id, codefile_id, printer),
        EdgeCmd::Govern {
            rule_id,
            intent_id,
            criterion,
        } => run_govern_with_sqlite(&cwd, rule_id, intent_id, criterion, printer),
        EdgeCmd::Hierarchy {
            parent_id,
            child_id,
            notes,
        } => run_hierarchy_with_sqlite(&cwd, parent_id, child_id, notes, printer),
        EdgeCmd::Validates {
            validation_id,
            intent_id,
            notes,
        } => run_validates_with_sqlite(&cwd, validation_id, intent_id, notes, printer),
        EdgeCmd::List { status, limit } => run_list_with_sqlite(&cwd, status, limit, printer),
        EdgeCmd::Show { edge_id } => run_show_with_sqlite(&cwd, edge_id, printer),
        EdgeCmd::Fix {
            edge_id,
            description,
        } => run_fix_with_sqlite(&cwd, edge_id, description, printer),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn run_explore_with_sqlite(
    root: &std::path::Path,
    intent_a_key: String,
    intent_b_key: String,
    subcommand: Option<ExploreSubCmd>,
    printer: &Printer,
) -> Result<()> {
    let mut store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let intent_a_id = resolve_intent_with_db(&store, &intent_a_key)?;
    let intent_b_id = resolve_intent_with_db(&store, &intent_b_key)?;
    let now = chrono::Utc::now().to_rfc3339();

    match subcommand {
        None => {
            let edge = store.get_or_create_relates_to(&intent_a_id, &intent_b_id, &now)?;
            let intent_a = store.get_intent(&intent_a_id)?.ok_or_else(|| {
                anyhow::anyhow!(
                    "Intent '{}' not found — the graph may be inconsistent; run `loom doctor`.",
                    intent_a_id
                )
            })?;
            let intent_b = store.get_intent(&intent_b_id)?.ok_or_else(|| {
                anyhow::anyhow!(
                    "Intent '{}' not found — the graph may be inconsistent; run `loom doctor`.",
                    intent_b_id
                )
            })?;

            if printer.json {
                printer.print_json(&serde_json::json!({
                    "edge": edge,
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
                    a = intent_a_id,
                    b = intent_b_id
                );
                println!(
                    "  loom edge explore {a} {b} issue  --criterion \"<text>\" --evidence \"<text>\"",
                    a = intent_a_id,
                    b = intent_b_id
                );
                println!(
                    "  loom edge explore {a} {b} independent --notes \"<why no relationship>\"",
                    a = intent_a_id,
                    b = intent_b_id
                );
            }
        }
        Some(ExploreSubCmd::Ground {
            criterion,
            evidence,
            evidence_locator,
            confidence,
            inspected_by,
        }) => {
            let now = chrono::Utc::now().to_rfc3339();
            let by = gate::acting_in_lane(&gate::lane::GROUND_RELATES_TO, inspected_by.as_deref())?;
            gate::require_substantive(
                "criterion",
                &criterion,
                "the falsifiable coexistence criterion this edge was checked against",
            )?;
            if !evidence.trim().is_empty() {
                gate::require_substantive(
                    "evidence",
                    &evidence,
                    "what the inspection actually found (file/symbol + the observation)",
                )?;
            }
            let evidence = gate::compose_evidence(&evidence_locator, &evidence)?;
            gate::require_confidence(confidence)?;
            let by = by.as_str();
            let edge = store.get_or_create_relates_to(&intent_a_id, &intent_b_id, &now)?;
            store.update_relates_to_ground(
                &edge.from_id,
                &edge.to_id,
                &criterion,
                &evidence,
                confidence,
                by,
                &now,
            )?;
            let updated = RelatesTo {
                inspection_status: "passing".to_string(),
                criterion,
                evidence,
                confidence,
                inspected_by: by.to_string(),
                last_inspected: now,
                ..edge
            };
            let next_step = "`loom next` for the next item.";
            if printer.json {
                let v = with_read_anchor(serde_json::to_value(&updated)?, &store, next_step)?;
                printer.print_json(&v);
            } else {
                println!("✓ Edge marked as passing (grounded)");
                println!("{}", fmt_edge_detail(&updated));
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
            inspected_by,
        }) => {
            let now = chrono::Utc::now().to_rfc3339();
            let by = gate::acting_in_lane(&gate::lane::ISSUE_RELATES_TO, inspected_by.as_deref())?;
            gate::require_substantive(
                "criterion",
                &criterion,
                "the falsifiable criterion that was violated",
            )?;
            gate::require_substantive(
                "evidence",
                &evidence,
                "what was actually found in the code (file/symbol + the problem)",
            )?;
            let evidence = gate::compose_evidence(&evidence_locator, &evidence)?;
            gate::require_confidence(confidence)?;
            let by = by.as_str();
            let edge = store.get_or_create_relates_to(&intent_a_id, &intent_b_id, &now)?;
            store.update_relates_to_issue(
                &edge.from_id,
                &edge.to_id,
                &criterion,
                &evidence,
                confidence,
                by,
                &now,
            )?;
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
                let v = with_read_anchor(serde_json::to_value(&updated)?, &store, &next_step)?;
                printer.print_json(&v);
            } else {
                println!("✓ Issue recorded — edge marked as failing");
                println!("{}", fmt_edge_detail(&updated));
                let snapshot = store.query_snapshot()?;
                println!("  → Next: {next_step}");
                println!("  {}", fmt_pulse(&store.graph_state(&snapshot)?));
            }
        }
        Some(ExploreSubCmd::Independent {
            notes,
            inspected_by,
        }) => {
            let now = chrono::Utc::now().to_rfc3339();
            let by =
                gate::acting_in_lane(&gate::lane::INDEPENDENT_RELATES_TO, inspected_by.as_deref())?;
            gate::require_substantive(
                "notes",
                &notes,
                "why these two intents have no meaningful relationship",
            )?;
            let by = by.as_str();
            let edge = store.get_or_create_relates_to(&intent_a_id, &intent_b_id, &now)?;
            store.update_relates_to_independent(&edge.from_id, &edge.to_id, &notes, by, &now)?;

            let next_step = "Continue discovery: `loom next`";
            if printer.json {
                let v = with_read_anchor(
                    serde_json::json!({
                        "status": "ok",
                        "edge_id": edge.id,
                        "inspection_status": "independent",
                        "from": intent_a_id,
                        "to": intent_b_id,
                        "notes": notes,
                    }),
                    &store,
                    next_step,
                )?;
                printer.print_json(&v);
            } else {
                println!(
                    "✓ Confirmed independent: {} ↔ {}  (edge id: {})",
                    intent_a_id, intent_b_id, edge.id
                );
                let snapshot = store.query_snapshot()?;
                println!("  → Next: {next_step}");
                println!("  {}", fmt_pulse(&store.graph_state(&snapshot)?));
            }
        }
    }
    Ok(())
}

fn run_implement_with_sqlite(
    root: &std::path::Path,
    intent_key: String,
    codefile_key: String,
    locator: String,
    notes: String,
    printer: &Printer,
) -> Result<()> {
    gate::acting_in_lane(&gate::lane::IMPLEMENT, None)?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let intent_id = resolve_intent_with_db(&store, &intent_key)?;
    let now = chrono::Utc::now().to_rfc3339();
    let targets = resolve_codefiles_with_db(&store, &codefile_key)?;
    let next_step = "ground more (`loom edge implement …`) or, if the leaf is fully grounded, prove it: `loom next --mode validate`";
    if targets.len() > 1 {
        for cf in &targets {
            store.insert_implements(&intent_id, &cf.id, "", &notes, &now)?;
        }
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "ok",
                "intent_id": intent_id,
                "grounded": targets.iter().map(|codefile| codefile.path.clone()).collect::<Vec<_>>(),
                "count": targets.len(),
                "next_step": next_step,
            }));
        } else {
            println!(
                "✓ Grounded intent in {} registered file(s) matching '{}'.",
                targets.len(),
                codefile_key
            );
            println!("  → Next: {}", next_step);
        }
    } else {
        let cf = &targets[0];
        store.insert_implements(&intent_id, &cf.id, &locator, &notes, &now)?;
        let edge_id =
            crate::db::schema::edge_key(crate::db::schema::edge::IMPLEMENTS, &intent_id, &cf.id);
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "ok",
                "edge_id": edge_id,
                "edge_type": EdgeType::Implements.to_string(),
                "intent_id": intent_id,
                "codefile_id": cf.id,
                "locator": locator,
                "next_step": next_step,
            }));
        } else {
            println!("✓ IMPLEMENTS edge created  (id: {})", edge_id);
            println!("  intent   → {}", intent_id);
            println!(
                "  codefile → {}{}",
                cf.path,
                if locator.is_empty() {
                    String::new()
                } else {
                    format!("  @ {}", locator)
                }
            );
            println!("  → Next: {}", next_step);
        }
    }
    Ok(())
}

fn run_unimplement_with_sqlite(
    root: &std::path::Path,
    intent_key: String,
    codefile_key: String,
    printer: &Printer,
) -> Result<()> {
    gate::acting_in_lane(&gate::lane::UNIMPLEMENT, None)?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let intent_id = resolve_intent_with_db(&store, &intent_key)?;
    let targets = resolve_codefiles_with_db(&store, &codefile_key)?;
    let mut removed: Vec<String> = Vec::new();
    for cf in &targets {
        if store.delete_implements(&intent_id, &cf.id)? {
            removed.push(cf.path.clone());
        }
    }
    if removed.is_empty() {
        anyhow::bail!(
            "No IMPLEMENTS edge between intent '{}' and '{}'.\n`loom codefile show <path>` lists the file's owners; `loom edge implement <intent> <path>` creates the grounding.",
            intent_id,
            codefile_key
        );
    }
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "intent_id": intent_id,
            "removed": removed,
            "next_step": "If the intent is a leaf it may be unrealized now — `loom status` will route.",
        }));
    } else {
        println!("✓ Removed {} grounding(s).", removed.len());
    }
    Ok(())
}

fn run_govern_with_sqlite(
    root: &std::path::Path,
    rule_key: String,
    intent_key: String,
    criterion: Option<String>,
    printer: &Printer,
) -> Result<()> {
    gate::acting_in_lane(&gate::lane::APPLY_RULE_EDGE, None)?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let rule_id = resolve_rule_with_db(&store, &rule_key)?;
    let intent_id = resolve_intent_with_db(&store, &intent_key)?;
    let now = chrono::Utc::now().to_rfc3339();
    let criterion = criterion.as_deref().unwrap_or("");
    if !criterion.is_empty() {
        gate::require_substantive(
            "criterion",
            criterion,
            "what compliance looks like for this rule on this intent",
        )?;
    }
    store.insert_governs(&rule_id, &intent_id, criterion, &now)?;
    let edge_id =
        crate::db::schema::edge_key(crate::db::schema::edge::GOVERNS, &rule_id, &intent_id);
    let next_step = format!(
        "make the edge real with a verdict: `loom rule verdict {} {} --status <passing|failing|independent> --criterion \"<text>\" --evidence \"<text>\"`",
        rule_id, intent_id
    );
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "edge_id": edge_id,
            "edge_type": EdgeType::Governs.to_string(),
            "rule_id": rule_id,
            "intent_id": intent_id,
            "next_step": next_step,
        }));
    } else {
        println!("✓ GOVERNS edge created  (id: {})", edge_id);
        println!("  rule   → {}", rule_id);
        println!("  intent → {}", intent_id);
        println!("  → Next: {}", next_step);
    }
    Ok(())
}

fn run_hierarchy_with_sqlite(
    root: &std::path::Path,
    parent_key: String,
    child_key: String,
    notes: Option<String>,
    printer: &Printer,
) -> Result<()> {
    gate::acting_in_lane(&gate::lane::HIERARCHY, None)?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let parent_id = resolve_intent_with_db(&store, &parent_key)?;
    let child_id = resolve_intent_with_db(&store, &child_key)?;
    let now = chrono::Utc::now().to_rfc3339();
    let notes = notes.as_deref().unwrap_or("");
    store.insert_hierarchy(&parent_id, &child_id, notes, &now)?;
    let edge_id =
        crate::db::schema::edge_key(crate::db::schema::edge::HIERARCHY, &parent_id, &child_id);
    let next_step = format!(
        "ground the child if it is a leaf (`loom edge implement {} <codefile> --locator \"<symbol>\"`), or keep decomposing",
        child_id
    );
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "edge_id": edge_id,
            "edge_type": EdgeType::Hierarchy.to_string(),
            "parent_id": parent_id,
            "child_id": child_id,
            "next_step": next_step,
        }));
    } else {
        println!("✓ HIERARCHY edge created  (id: {})", edge_id);
        println!("  parent → {}", parent_id);
        println!("  child  → {}", child_id);
        println!("  → Next: {}", next_step);
    }
    Ok(())
}

fn run_validates_with_sqlite(
    root: &std::path::Path,
    validation_key: String,
    intent_key: String,
    notes: Option<String>,
    printer: &Printer,
) -> Result<()> {
    gate::acting_in_lane(&gate::lane::LINK_VALIDATION, None)?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let validation_id = resolve_validation_with_db(&store, &validation_key)?;
    let intent_id = resolve_intent_with_db(&store, &intent_key)?;
    let now = chrono::Utc::now().to_rfc3339();
    let notes = notes.as_deref().unwrap_or("");
    store.insert_validates(&validation_id, &intent_id, notes, &now)?;
    let edge_id = crate::db::schema::edge_key(
        crate::db::schema::edge::VALIDATES,
        &validation_id,
        &intent_id,
    );
    let next_step = format!("make the proof real: `loom validate {}`", intent_id);
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "edge_id": edge_id,
            "edge_type": EdgeType::Validates.to_string(),
            "validation_id": validation_id,
            "intent_id": intent_id,
            "next_step": next_step,
        }));
    } else {
        println!("✓ VALIDATES edge created  (id: {})", edge_id);
        println!("  validation → {}", validation_id);
        println!("  intent     → {}", intent_id);
        println!("  → Next: {}", next_step);
    }
    Ok(())
}

fn run_list_with_sqlite(
    root: &std::path::Path,
    status: Option<String>,
    limit: usize,
    printer: &Printer,
) -> Result<()> {
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let mut edges = store.query_snapshot()?.relates;
    if let Some(status) = status.as_deref() {
        edges.retain(|edge| edge.inspection_status == status);
    }
    edges.sort_by(|a, b| {
        b.priority_score
            .partial_cmp(&a.priority_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    let total = apply_limit(&mut edges, limit);
    if printer.json {
        printer.print_json(&serde_json::json!({
            "edges": edges,
            "total": total,
            "shown": edges.len(),
            "status_filter": status,
            "more": more_marker(total, edges.len(), "loom edge list --limit 0"),
        }));
    } else {
        for edge in &edges {
            println!("{}", fmt_edge_row(edge));
        }
        if let Some(marker) = more_marker(total, edges.len(), "loom edge list --limit 0") {
            println!("  {marker}");
        }
    }
    Ok(())
}

fn run_show_with_sqlite(root: &std::path::Path, edge_id: String, printer: &Printer) -> Result<()> {
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let snapshot = store.query_snapshot()?;
    let edge = snapshot
        .relates
        .iter()
        .find(|edge| edge.id == edge_id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("RELATES_TO edge '{}' not found.", edge_id))?;
    let from = store
        .get_intent(&edge.from_id)?
        .unwrap_or_else(|| default_intent(&edge.from_id));
    let to = store
        .get_intent(&edge.to_id)?
        .unwrap_or_else(|| default_intent(&edge.to_id));
    let notes = store.notes_for_target(&edge.id)?;

    if printer.json {
        printer.print_json(&serde_json::json!({
            "edge": edge,
            "from_intent": from,
            "to_intent": to,
            "notes": notes,
        }));
    } else {
        println!("── From ─────────────────────────────────────────────────────────");
        println!("{}", fmt_intent(&from));
        println!();
        println!("── To ───────────────────────────────────────────────────────────");
        println!("{}", fmt_intent(&to));
        println!();
        println!("── Edge ─────────────────────────────────────────────────────────");
        println!("{}", fmt_edge_detail(&edge));
        if !notes.is_empty() {
            println!();
            println!("── Notes ────────────────────────────────────────────────────────");
            for note in notes.iter().take(SECTION_CAP) {
                println!("  [{}] {}: {}", note.kind, note.created_at, note.text);
            }
            if let Some(marker) = more_marker(
                notes.len(),
                notes.len().min(SECTION_CAP),
                "loom note list --edge <id>",
            ) {
                println!("  {marker}");
            }
        }
    }
    Ok(())
}

fn run_fix_with_sqlite(
    root: &std::path::Path,
    edge_id: String,
    description: String,
    printer: &Printer,
) -> Result<()> {
    gate::acting_in_lane(&gate::lane::FIX_RELATES_TO, None)?;
    gate::require_substantive(
        "description",
        &description,
        "what changed in code or design to make the failing/stale relationship true again",
    )?;
    let mut store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let snapshot = store.query_snapshot()?;
    let edge = snapshot
        .relates
        .iter()
        .find(|edge| edge.id == edge_id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("RELATES_TO edge '{}' not found.", edge_id))?;
    anyhow::ensure!(
        matches!(
            edge.inspection_status.as_str(),
            "failing" | "needs_reverification"
        ),
        "Edge '{}' is '{}'; `loom edge fix` only applies to failing or needs_reverification edges.",
        edge.id,
        edge.inspection_status
    );
    let now = chrono::Utc::now().to_rfc3339();
    let criterion = if edge.criterion.trim().is_empty() {
        "the relationship remains valid after the repair"
    } else {
        edge.criterion.as_str()
    };
    let evidence = format!("Repair verified: {description}");
    let confidence = if edge.confidence > 0.0 {
        edge.confidence
    } else {
        0.9
    };
    store.update_relates_to_ground(
        &edge.from_id,
        &edge.to_id,
        criterion,
        &evidence,
        confidence,
        "loom",
        &now,
    )?;
    let updated = RelatesTo {
        inspection_status: "passing".to_string(),
        criterion: criterion.to_string(),
        evidence,
        confidence,
        inspected_by: "loom".to_string(),
        last_inspected: now,
        ..edge
    };
    let next_step = "`loom next --mode fix` for the next stale or failing edge.";
    if printer.json {
        let v = with_read_anchor(serde_json::to_value(&updated)?, &store, next_step)?;
        printer.print_json(&v);
    } else {
        println!("✓ Edge marked as passing after repair");
        println!("{}", fmt_edge_detail(&updated));
        let snapshot = store.query_snapshot()?;
        println!("  → Next: {next_step}");
        println!("  {}", fmt_pulse(&store.graph_state(&snapshot)?));
    }
    Ok(())
}

fn resolve_intent_with_db(db: &dyn GraphReadRepository, key: &str) -> Result<String> {
    let intents = db.list_intents(None, None)?;
    if intents.iter().any(|intent| intent.id == key) {
        return Ok(key.to_string());
    }
    let lower = key.to_lowercase();
    let exact: Vec<_> = intents
        .iter()
        .filter(|intent| intent.name.to_lowercase() == lower)
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0].id.clone());
    }
    if exact.len() > 1 {
        anyhow::bail!(
            "Intent name '{}' is not unique ({} intents carry it) — use the id. `loom intent list` to see them.",
            key,
            exact.len()
        );
    }
    let matches: Vec<_> = intents
        .iter()
        .filter(|intent| intent.name.to_lowercase().contains(&lower))
        .collect();
    match matches.len() {
        1 => Ok(matches[0].id.clone()),
        0 => anyhow::bail!(
            "No intent matches '{}' (by id, exact name, or name fragment). Run `loom intent list`.",
            key
        ),
        _ => {
            let total = matches.len();
            let shown = matches
                .iter()
                .take(10)
                .map(|intent| format!("'{}'", intent.name))
                .collect::<Vec<_>>()
                .join(", ");
            if total > 10 {
                anyhow::bail!(
                    "'{}' is ambiguous — it matches: {} … +{} more — narrow the fragment or `loom find \"{}\"`.",
                    key,
                    shown,
                    total - 10,
                    key
                );
            }
            anyhow::bail!(
                "'{}' is ambiguous — it matches: {}. Narrow the fragment or use an id.",
                key,
                shown
            );
        }
    }
}

fn resolve_codefiles_with_db(db: &dyn GraphReadRepository, key: &str) -> Result<Vec<CodeFile>> {
    let codefiles = db.query_snapshot()?.codefiles;
    let is_glob = key.contains('*') || key.contains('?') || key.contains('[');
    if is_glob {
        let pat = glob::Pattern::new(key).map_err(|e| {
            anyhow::anyhow!(
                "Invalid glob '{}': {} — quote it: `loom codefile add 'src/**/*.rs'`",
                key,
                e
            )
        })?;
        let matched: Vec<_> = codefiles
            .into_iter()
            .filter(|codefile| pat.matches(&codefile.path))
            .collect();
        if matched.is_empty() {
            anyhow::bail!(
                "No REGISTERED codefile matches glob '{}'. Register first: loom codefile add '{}'",
                key,
                key
            );
        }
        return Ok(matched);
    }
    codefiles
        .into_iter()
        .find(|codefile| codefile.id == key || codefile.path == key)
        .map(|codefile| vec![codefile])
        .ok_or_else(|| {
            anyhow::anyhow!(
                "CodeFile '{}' not found (by id or path).\nRegister it first: loom codefile add <path>",
                key
            )
        })
}

fn resolve_rule_with_db(db: &dyn GraphReadRepository, key: &str) -> Result<String> {
    let rules = db.list_rules()?;
    if rules.iter().any(|rule| rule.id == key) {
        return Ok(key.to_string());
    }
    let lower = key.to_lowercase();
    let exact: Vec<_> = rules
        .iter()
        .filter(|rule| rule.name.to_lowercase() == lower)
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0].id.clone());
    }
    let matches: Vec<_> = rules
        .iter()
        .filter(|rule| rule.name.to_lowercase().contains(&lower))
        .collect();
    match matches.len() {
        1 => Ok(matches[0].id.clone()),
        0 => anyhow::bail!(
            "No rule matches '{}' (by id, exact name, or name fragment). Run `loom rule list`.",
            key
        ),
        _ => {
            let mut shown = matches
                .iter()
                .take(SECTION_CAP)
                .map(|rule| format!("'{}'", rule.name))
                .collect::<Vec<_>>()
                .join(", ");
            if let Some(marker) = more_marker(
                matches.len(),
                matches.len().min(SECTION_CAP),
                "`loom rule list`",
            ) {
                shown.push_str(", ");
                shown.push_str(&marker);
            }
            anyhow::bail!(
                "'{}' is ambiguous — it matches: {}. Narrow the fragment or use an id.",
                key,
                shown
            )
        }
    }
}

fn resolve_validation_with_db(db: &dyn GraphReadRepository, key: &str) -> Result<String> {
    let validations = db.list_validations()?;
    if validations.iter().any(|validation| validation.id == key) {
        return Ok(key.to_string());
    }
    let lower = key.to_lowercase();
    let exact: Vec<_> = validations
        .iter()
        .filter(|validation| validation.name.to_lowercase() == lower)
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0].id.clone());
    }
    let matches: Vec<_> = validations
        .iter()
        .filter(|validation| validation.name.to_lowercase().contains(&lower))
        .collect();
    match matches.len() {
        1 => Ok(matches[0].id.clone()),
        0 => anyhow::bail!(
            "No validation matches '{}' (by id, name, or fragment). Run `loom validation list`.",
            key
        ),
        _ => anyhow::bail!(
            "'{}' is ambiguous — matches {} validations. Use the id (`loom validation list`).",
            key,
            matches.len()
        ),
    }
}

fn default_intent(id: &str) -> Intent {
    Intent {
        id: id.to_string(),
        name: "(unknown)".to_string(),
        description: String::new(),
        abstraction_level: String::new(),
        domain: String::new(),
        layer: String::new(),
        source_refs: Vec::new(),
        status: String::new(),
        aspect: String::new(),
        tags: Vec::new(),
        visibility: String::new(),
        boundary: String::new(),
        lifecycle: String::new(),
        created_at: String::new(),
        updated_at: String::new(),
    }
}
