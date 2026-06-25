use anyhow::Result;

use crate::cli::{EdgeCmd, ExploreSubCmd};
use crate::commands::resolve::{resolve_intent_with_db, resolve_validation_with_db};
use crate::db::{ensure_initialized, GraphReadRepository};
use crate::gate;
use crate::output::{
    apply_limit, fmt_edge_detail, fmt_edge_row, fmt_intent, fmt_pulse, more_marker,
    with_read_anchor, Printer, SECTION_CAP,
};
use crate::types::{CodeFile, EdgeType, Intent, RelatesTo};

/// The single "RELATES_TO edge not found" contract, shared by `edge show` and
/// `edge fix` so the message stays identical when a lookup by id fails.
fn relates_edge_not_found(edge_id: &str) -> String {
    format!("RELATES_TO edge '{edge_id}' not found.")
}

/// Validate analyzer-asserted relationship kinds and merge them onto an edge:
/// the provided JUDGMENT kinds replace the edge's judgment tier; MECHANICAL
/// kinds (populate-derived) are preserved. Rejects a mechanical kind here —
/// those are derived, not asserted. Returns the new full kind set.
pub(crate) fn apply_judgment_kinds(
    store: &crate::db::sqlite::SqliteGraphStore,
    edge_kinds: &[String],
    from: &str,
    to: &str,
    provided: &[String],
) -> Result<Vec<String>> {
    for k in provided {
        let rk = k.parse::<crate::types::RelationKind>()?;
        if rk.is_mechanical() {
            anyhow::bail!(
                "--kind '{k}' is a MECHANICAL kind (derived by `loom populate kinds`, not asserted). \
                 Assert a judgment kind: calls | inheritance | shares_state | doc_reference | manual."
            );
        }
    }
    let mut new_kinds: Vec<String> = edge_kinds
        .iter()
        .filter(|k| {
            k.parse::<crate::types::RelationKind>()
                .map(|rk| rk.is_mechanical())
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    for p in provided {
        if !new_kinds.contains(p) {
            new_kinds.push(p.clone());
        }
    }
    new_kinds.sort();
    store.update_relates_to_kinds(from, to, &new_kinds)?;
    Ok(new_kinds)
}

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
        EdgeCmd::Unexplored { class, limit } => {
            run_unexplored_with_sqlite(&cwd, class, limit, printer)
        }
        EdgeCmd::Show { edge_id } => run_show_with_sqlite(&cwd, edge_id, printer),
        EdgeCmd::Stable {
            intent_a_id,
            intent_b_id,
            off,
        } => run_stable_with_sqlite(&cwd, intent_a_id, intent_b_id, off, printer),
        EdgeCmd::Fix {
            edge_id,
            description,
        } => run_fix_with_sqlite(&cwd, edge_id, description, printer),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn run_stable_with_sqlite(
    root: &std::path::Path,
    intent_a_key: String,
    intent_b_key: String,
    off: bool,
    printer: &Printer,
) -> Result<()> {
    let mut store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    store.ensure_owned("mark a relationship stable")?;
    let intent_a_id = resolve_intent_with_db(&store, &intent_a_key)?;
    let intent_b_id = resolve_intent_with_db(&store, &intent_b_key)?;
    let stable = !off;
    let changed = store.set_relates_to_stable(&intent_a_id, &intent_b_id, stable)?;
    if !changed {
        anyhow::bail!(
            "No RELATES_TO edge between '{intent_a_key}' and '{intent_b_key}' — explore/ground it \
             first (`loom edge explore <a> <b> ground …`). Only a grounded relationship can be \
             marked stable."
        );
    }
    let edge = store.get_relates_to_between(&intent_a_id, &intent_b_id)?;
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "stable": stable,
            "edge": edge,
            "next_step": if stable {
                "sync will no longer re-open this edge on endpoint code changes (`loom edge stable … --off` to re-arm)"
            } else {
                "sync reverification re-armed for this edge"
            },
        }));
        return Ok(());
    }
    if stable {
        println!("✓ Marked relationship stable — `loom sync` will not re-open it on code changes.");
    } else {
        println!("✓ Cleared stable — `loom sync` reverification re-armed for this relationship.");
    }
    Ok(())
}

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
    // A self-relationship is meaningless and used to miscount as "intent not
    // found" in the existence probe — name the real cause.
    if intent_a_id == intent_b_id {
        anyhow::bail!(
            "An intent can't relate to itself — both arguments resolved to {intent_a_id}. Pass two different intents (did you paste the same id twice?)."
        );
    }
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
            kinds,
            inspected_by,
        }) => {
            let now = chrono::Utc::now().to_rfc3339();
            let by = gate::acting_in_lane(&gate::lane::GROUND_RELATES_TO, inspected_by.as_deref())?;
            gate::require_substantive("criterion", &criterion, gate::RELATES_TO_CRITERION_PURPOSE)?;
            if !evidence.trim().is_empty() {
                gate::require_substantive(
                    "evidence",
                    &evidence,
                    gate::RELATES_TO_EVIDENCE_PURPOSE,
                )?;
            }
            gate::require_locators_resolve(root, &evidence_locator)?;
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
            let final_kinds = if kinds.is_empty() {
                edge.kinds.clone()
            } else {
                apply_judgment_kinds(&store, &edge.kinds, &edge.from_id, &edge.to_id, &kinds)?
            };
            let updated = RelatesTo {
                inspection_status: "passing".to_string(),
                criterion,
                evidence,
                confidence,
                inspected_by: by.to_string(),
                last_inspected: now,
                kinds: final_kinds,
                ..edge
            };
            let next_step = crate::output::NEXT_DISCOVERY_STEP;
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
            kinds,
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
            gate::require_locators_resolve(root, &evidence_locator)?;
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
            let final_kinds = if kinds.is_empty() {
                edge.kinds.clone()
            } else {
                apply_judgment_kinds(&store, &edge.kinds, &edge.from_id, &edge.to_id, &kinds)?
            };
            let updated = RelatesTo {
                inspection_status: "failing".to_string(),
                criterion,
                evidence,
                confidence,
                inspected_by: by.to_string(),
                last_inspected: now,
                kinds: final_kinds,
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
            gate::require_substantive("notes", &notes, gate::INDEPENDENT_NOTES_PURPOSE)?;
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
    mut locator: String,
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
        // A locator names ONE symbol in ONE file — it cannot apply to a glob that
        // matched many. Silently dropping it (and skipping verify-first) is how a
        // typo'd locator used to mass-ground an intent to every file. Refuse.
        if !locator.trim().is_empty() {
            anyhow::bail!(
                "`--locator \"{}\"` cannot be used with a glob that matched {} files ('{}') — a \
                 locator names one symbol in one file.\n\
                 Ground per file: loom edge implement {} <one-file> --locator \"{}\"\n\
                 Or drop --locator for file-level grounding across the match.",
                locator.trim(),
                targets.len(),
                codefile_key,
                intent_key,
                locator.trim(),
            );
        }
        // A file-level glob grounding must NOT silently CLOBBER a precise locator
        // the intent already has on a matched file (insert_implements upserts to an
        // empty locator). Preserve those; only newly ground files not already owned
        // at a symbol. Report both so the spread (incl. any unintended glob matches)
        // is visible, never a silent mass-mutation.
        let prior: std::collections::HashMap<String, String> = store
            .list_implements_for_intent(&intent_id)?
            .into_iter()
            .map(|e| (e.codefile_id, e.locator))
            .collect();
        let mut newly_grounded: Vec<String> = Vec::new();
        let mut preserved: Vec<String> = Vec::new();
        for cf in &targets {
            match prior.get(&cf.id) {
                Some(loc) if !loc.trim().is_empty() => {
                    preserved.push(format!("{} @ {}", cf.path, loc.trim()))
                }
                _ => {
                    store.insert_implements(&intent_id, &cf.id, "", &notes, &now)?;
                    newly_grounded.push(cf.path.clone());
                }
            }
        }
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "ok",
                "intent_id": intent_id,
                "grounded": newly_grounded,
                "preserved_precise_locators": preserved,
                "count": targets.len(),
                "next_step": next_step,
            }));
        } else {
            println!(
                "✓ File-level grounded {} of {} file(s) matching '{}'.",
                newly_grounded.len(),
                targets.len(),
                codefile_key
            );
            for p in &newly_grounded {
                println!("    + {p}");
            }
            if !preserved.is_empty() {
                println!(
                    "  ⚠ kept {} existing PRECISE locator(s) (a file-level glob does not clobber them):",
                    preserved.len()
                );
                for p in &preserved {
                    println!("    · {p}");
                }
            }
            println!("  → Next: {}", next_step);
        }
    } else {
        let cf = &targets[0];
        // Verify-first grounding: a non-empty locator must actually occur in the
        // file NOW — otherwise the grounding is born stale and only surfaces at
        // the next `loom sync`, after the worker has moved on. We check at ground
        // time using the SAME matcher sync uses (`repo::locator_present`). Only
        // enforced when the file is readable on disk; a file missing on disk is a
        // separate condition that codefile registration / sync already report.
        if !locator.trim().is_empty() {
            if let Ok(content) = std::fs::read_to_string(root.join(&cf.path)) {
                let loc = locator.trim().to_string();
                // Resolve the locator against the file's extracted top-level
                // symbols so loom's OWN display label round-trips. The label
                // normalizes/qualifies away from raw source — `func Get` for
                // `func (s *Store) Get`, `export function main` for `export
                // default async function main`, `fn Shape for Circle::area` for a
                // trait-impl method — so passing it verbatim used to fail the
                // substring gate even though coverage/next/seed SUGGESTED it.
                // Match on the locator's trailing identifier (the symbol name).
                let facts = crate::repo::extract_physical_facts(root, &cf.path, &content);
                let last = crate::repo::last_identifier(&loc);
                let named_symbol = (!last.is_empty())
                    .then(|| facts.symbol_facts.iter().find(|s| s.name == last))
                    .flatten();
                let present = crate::repo::locator_present(&content, &loc);
                match (named_symbol, present) {
                    // Names an extracted symbol AND is a raw substring → ideal,
                    // store verbatim (covers `fn run`, `def make_slug`, a bare name).
                    (Some(_), true) => {}
                    // Names an extracted symbol but is NOT a raw substring (loom's
                    // normalized label) → rewrite to the bare name, which IS a
                    // substring and is what sync's symbol-precision keys on.
                    (Some(sym), false) => {
                        if !printer.json {
                            println!(
                                "  ℹ locator \"{}\" normalized to its extracted symbol \"{}\" so it round-trips on sync",
                                loc, sym.name
                            );
                        }
                        locator = sym.name.clone();
                    }
                    // A real substring, but NOT an extracted top-level symbol: a
                    // nested method / decorated def tree-sitter does not surface.
                    // Accept, but warn INLINE that it grounds at file granularity —
                    // don't make the driver discover it later via `loom doctor`.
                    (None, true) => {
                        if !printer.json {
                            println!(
                                "  ⚠ \"{}\" is not an extracted top-level symbol (likely a nested method, or a form the extractor doesn't surface) — this IMPLEMENTS grounds at FILE granularity, so `loom sync` re-checks it on ANY change to {}.",
                                loc, cf.path
                            );
                        }
                    }
                    // Neither a substring nor a known symbol → born stale; refuse.
                    (None, false) => anyhow::bail!(
                        "Locator '{}' does not occur in {} — the grounding would be stale on arrival.\n\
                         Use the symbol AS IT APPEARS in the file (e.g. `fn run`, `def shorten`, `class Link`) \
                         or its bare name; `loom codefile show {}` lists the extracted symbols.\n\
                         `loom explain {}` shows how related intents are grounded; re-run with the real symbol:\n  \
                         loom edge implement {} {} --locator \"<symbol>\"",
                        loc,
                        cf.path,
                        cf.path,
                        intent_id,
                        intent_key,
                        cf.path,
                    ),
                }
            }
        }
        // The IMPLEMENTS edge id is DERIVED from (intent, file), so a second
        // `edge implement` on the same pair is an UPDATE, not a create:
        // insert_implements upserts and resets criterion/evidence. Re-printing
        // "✓ created" would hide that the prior locator (and any analyzer
        // verdict) was discarded — exactly the silent overwrite a multi-symbol
        // file walks into. Read the prior edge first so the output can be honest.
        let prior = store
            .list_implements_for_intent(&intent_id)?
            .into_iter()
            .find(|e| e.codefile_id == cf.id);
        let updated = prior.is_some();
        let locator_replaced = match &prior {
            Some(p) if p.locator != locator => Some(p.locator.clone()),
            _ => None,
        };
        let verdict_discarded = prior
            .as_ref()
            .map(|p| !p.criterion.trim().is_empty())
            .unwrap_or(false);

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
                "updated": updated,
                "replaced_locator": locator_replaced,
                "discarded_verdict": verdict_discarded,
                "next_step": next_step,
            }));
        } else {
            if updated {
                println!("↻ IMPLEMENTS edge updated  (id: {})", edge_id);
            } else {
                println!("✓ IMPLEMENTS edge created  (id: {})", edge_id);
            }
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
            if let Some(old) = &locator_replaced {
                let old_disp = if old.is_empty() {
                    "(file-level, no locator)".to_string()
                } else {
                    format!("'{old}'")
                };
                let new_disp = if locator.is_empty() {
                    "(file-level)".to_string()
                } else {
                    format!("'{locator}'")
                };
                println!(
                    "  ⚠ replaced locator {old_disp} → {new_disp} — one intent→file edge holds ONE \
                     locator. To own a second symbol in this file, create a separate intent and ground that."
                );
            }
            if verdict_discarded {
                println!(
                    "  ⚠ prior grounding verdict discarded (criterion/evidence reset) — re-verify this edge."
                );
            }
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
        gate::require_substantive("criterion", criterion, gate::GOVERNS_CRITERION_PURPOSE)?;
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
        println!("{}", crate::output::governs_edge_created_line(&edge_id));
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
    // A self-edge (same id in both slots — an easy UUID fat-finger) used to hit
    // the DB's `id IN (a,b)` existence probe, which collapses to one row and
    // reported "one or both intents not found" — sending the AI to recreate an
    // intent that exists. Name the real cause instead.
    if parent_id == child_id {
        anyhow::bail!(
            "An intent can't be its own parent — `parent` and `child` both resolved to {parent_id}. Pass two different intents (did you paste the same id twice?)."
        );
    }
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

/// List the intent pairs that have NO RELATES_TO edge yet — the drainable form of
/// the `unexplored_pairs` count the compass reports. Reuses the SAME candidate
/// generation `loom next --mode discovery` uses, so the count and this list agree.
/// Each pair carries a pre-filled `loom edge explore` command for batching.
fn run_unexplored_with_sqlite(
    root: &std::path::Path,
    class: Option<String>,
    limit: usize,
    printer: &Printer,
) -> Result<()> {
    use crate::db::queries::scoring::{
        unexplored_pairs_scored_from_snapshot, DiscoveryClassFilter,
    };
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let snapshot = store.query_snapshot()?;
    // Default to `all`: every unexplored pair is owed for phase=complete. The
    // narrower classes (`suspected-coupling`, `impact-map`) only prioritise.
    let class_filter = DiscoveryClassFilter::parse(class.as_deref().or(Some("all")))?;
    let mut pairs = unexplored_pairs_scored_from_snapshot(&snapshot, class_filter)?;
    pairs.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.id.cmp(&b.0.id))
    });
    let total = pairs.len();
    if limit > 0 && pairs.len() > limit {
        pairs.truncate(limit);
    }
    let explore_cmd = |e: &crate::types::RelatesTo| {
        // Signal-aware prefill: pairs with no semantic signals (impact_map)
        // get `independent` first (the expected verdict); pairs with signals
        // (suspected_coupling) get `ground` first (a real coupling to inspect).
        let has_signals = !e.discovery_signals.is_empty();
        if has_signals {
            format!(
                "loom edge explore {} {} ground --criterion \"<what couples them>\" --confidence 0.9   (or `independent --notes \"…\"` if unrelated)",
                e.from_id, e.to_id
            )
        } else {
            format!(
                "loom edge explore {} {} independent --notes \"<why they don't interact — what boundary keeps them apart>\"   (or `ground --criterion \"…\"` if a real coupling exists)",
                e.from_id, e.to_id
            )
        }
    };
    let next = "Verdict each pair. Signal-bearing pairs (suspected-coupling): read the code, `ground` if coupled, `independent` if not. Centrality-only pairs (impact-map): `independent` is expected — but name the specific boundary that keeps them apart (shared imports? no. shared vocab? no. same domain? no). Batch the verdicts: paste these commands into `loom batch`.";
    if printer.json {
        let items: Vec<_> = pairs
            .iter()
            .map(|(e, score)| {
                serde_json::json!({
                    "from_id": e.from_id, "from_name": e.from_name,
                    "to_id": e.to_id, "to_name": e.to_name,
                    "class": e.discovery_class, "why": e.notes, "score": score,
                    "explore_command": explore_cmd(e),
                })
            })
            .collect();
        printer.print_json(&serde_json::json!({
            "unexplored_pairs": items,
            "total": total,
            "shown": pairs.len(),
            "class": class_filter.as_cli_value(),
            "more": more_marker(total, pairs.len(), "loom edge unexplored --limit 0"),
            "next_step": next,
        }));
    } else if pairs.is_empty() {
        println!(
            "(no unexplored pairs for class '{}')",
            class_filter.as_cli_value()
        );
    } else {
        println!(
            "── unexplored pairs ({} class) ───────────────────────────────",
            class_filter.as_cli_value()
        );
        for (e, score) in &pairs {
            println!(
                "  {} ✕ {}   [{} · score {:.2}]",
                e.from_name, e.to_name, e.discovery_class, score
            );
            if !e.notes.is_empty() {
                println!("      why: {}", e.notes);
            }
            println!("      {}", explore_cmd(e));
        }
        if let Some(marker) = more_marker(total, pairs.len(), "loom edge unexplored --limit 0") {
            println!("  {marker}");
        }
        println!("  → {next}");
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
        .ok_or_else(|| anyhow::anyhow!(relates_edge_not_found(&edge_id)))?;
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
        .ok_or_else(|| anyhow::anyhow!(relates_edge_not_found(&edge_id)))?;
    anyhow::ensure!(
        matches!(
            edge.inspection_status.as_str(),
            "failing" | "needs_reverification"
        ),
        "Edge '{}' is '{}'; `loom edge fix` only applies to failing or needs_reverification edges.",
        edge.id,
        edge.inspection_status
    );
    // A saga-proven boundary edge carries RUNTIME evidence ("runtime: saga …") — it
    // was proven by EXECUTING the journey against the live surface. A prose
    // `edge fix` claim cannot re-establish a runtime boundary (the service may
    // still be broken); only a passing `loom saga run` can. Refuse to launder it.
    if edge.evidence.trim_start().starts_with("runtime: saga ") {
        anyhow::bail!(
            "Edge '{}' was proven by a SAGA RUN — its evidence is RUNTIME (it executed the journey \
             against the live surface). A manual `loom edge fix` description cannot re-establish a \
             runtime boundary; the service may still be broken. Fix the code/service, then re-run \
             the saga: `loom saga run <saga>` — it re-stamps this path edge passing ONLY if the \
             journey actually passes end-to-end.",
            edge.id
        );
    }
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

fn resolve_codefiles_with_db(db: &dyn GraphReadRepository, key: &str) -> Result<Vec<CodeFile>> {
    let codefiles = db.query_snapshot()?.codefiles;
    let is_glob = key.contains('*') || key.contains('?') || key.contains('[');
    if is_glob {
        let pat = glob::Pattern::new(key)
            .map_err(|e| anyhow::anyhow!(crate::output::invalid_glob_msg(key, &e)))?;
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

fn default_intent(id: &str) -> Intent {
    Intent {
        id: id.to_string(),
        name: "(unknown)".to_string(),
        description: String::new(),
        criterion: String::new(),
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
