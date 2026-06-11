use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use crate::db::{ensure_initialized, GrafeoDb, LoomDb};
use crate::db::queries::{
    invalidate_validations_for_intents_with_indexes, list_all_governs, list_all_implements,
    list_all_targets, list_all_validates, list_codefiles, list_relates_to, list_validations,
    record_sync_flip, set_last_synced, update_codefile_hash, update_codefile_hash_and_mtime,
    update_codefile_imports, update_codefile_mtime,
};
use crate::db::schema::esc;
use crate::output::Printer;
use crate::types::{SyncChange, SyncReport};

pub fn run(path: &str, printer: &Printer) -> Result<()> {
    let base = if path == "." {
        crate::db::resolve_root()?
    } else {
        Path::new(path).canonicalize().unwrap_or_else(|_| Path::new(path).to_path_buf())
    };

    let db_file = ensure_initialized(&base)?;
    let db = GrafeoDb::open(&db_file)?;

    let codefiles = list_codefiles(&db)?;
    let files_checked = codefiles.len();
    let now = chrono::Utc::now().to_rfc3339();

    let mut files_changed = 0usize;
    let mut targets_flagged = 0usize;
    let mut relates_to_flagged = 0usize;
    let mut governs_flagged = 0usize;
    let mut validations_invalidated = 0usize;
    let mut changes: Vec<SyncChange> = Vec::new();
    let mut missing_files: Vec<String> = Vec::new();
    let mut text_contents: HashMap<String, String> = HashMap::new();
    let mut non_utf8_files: HashSet<String> = HashSet::new();
    let mut active_intents: Option<HashSet<String>> = None;

    let all_implements = list_all_implements(&db)?;
    let mut intents_by_codefile: HashMap<&str, Vec<String>> = HashMap::new();
    for im in &all_implements {
        intents_by_codefile
            .entry(im.codefile_id.as_str())
            .or_default()
            .push(im.intent_id.clone());
    }
    let all_validates = list_all_validates(&db)?;
    let all_validations = list_validations(&db)?;
    let mut validates_by_intent: HashMap<&str, Vec<&crate::types::ValidatesEdge>> = HashMap::new();
    for e in &all_validates {
        validates_by_intent.entry(e.intent_id.as_str()).or_default().push(e);
    }
    let validation_by_id: HashMap<&str, &crate::types::Validation> =
        all_validations.iter().map(|v| (v.id.as_str(), v)).collect();
    let mut invalidated_validation_ids: HashSet<String> = HashSet::new();

    let all_relates = list_relates_to(&db, None)?;
    let mut relates_by_intent: HashMap<&str, Vec<&crate::types::RelatesTo>> = HashMap::new();
    for edge in &all_relates {
        relates_by_intent.entry(edge.from_id.as_str()).or_default().push(edge);
        relates_by_intent.entry(edge.to_id.as_str()).or_default().push(edge);
    }
    let all_governs = list_all_governs(&db)?;
    let mut governs_by_intent: HashMap<&str, Vec<&crate::types::Governs>> = HashMap::new();
    for edge in &all_governs {
        governs_by_intent.entry(edge.intent_id.as_str()).or_default().push(edge);
    }
    let all_targets = list_all_targets(&db)?;
    let mut targets_by_intent: HashMap<&str, Vec<&crate::types::TargetsEdge>> = HashMap::new();
    for edge in &all_targets {
        targets_by_intent.entry(edge.intent_id.as_str()).or_default().push(edge);
    }
    let mut related_edges_flagged: HashSet<String> = HashSet::new();
    let mut governs_edges_flagged_ids: HashSet<String> = HashSet::new();
    let mut targets_edges_flagged_ids: HashSet<String> = HashSet::new();

    for cf in &codefiles {
        // Resolve path relative to the loom project root if not absolute
        let file_path = Path::new(&cf.path);
        let abs_path = if file_path.is_absolute() {
            file_path.to_path_buf()
        } else {
            base.join(file_path)
        };

        let meta = match fs::metadata(&abs_path) {
            Ok(m) => m,
            Err(_) => {
                // File is registered in the graph but gone from disk
                // (deleted/renamed) — a phantom that distorts coverage and
                // vertical completeness. Surface it; never skip silently.
                missing_files.push(cf.path.clone());
                continue;
            }
        };

        let mtime = meta.modified().map_err(|e| {
            anyhow::anyhow!(
                "Cannot read mtime for {}: {} — restore the file (or `loom codefile remove <path>` if it is intentionally gone), then re-run `loom sync`.",
                abs_path.display(), e
            )
        })?;

        let mtime_str = {
            let dt: chrono::DateTime<chrono::Utc> = mtime.into();
            dt.to_rfc3339()
        };

        // "Changed" means the BYTES changed: the content fingerprint decides.
        // mtime alone false-flags after a checkout/rebase (timestamps churn,
        // content doesn't) — that would reset half the graph for nothing. The
        // mtime comparison remains only as the fallback when no fingerprint is
        // stored yet (pre-upgrade graph).
        let bytes = fs::read(&abs_path).map_err(|e| {
            anyhow::anyhow!(
                "Cannot read bytes for {}: {} — restore the file (or `loom codefile remove <path>` if it is intentionally gone), then re-run `loom sync`.",
                abs_path.display(), e
            )
        })?;
        let new_hash = crate::repo::content_hash(&bytes);
        match String::from_utf8(bytes) {
            Ok(content) => {
                text_contents.insert(cf.path.clone(), content);
            }
            Err(_) => {
                non_utf8_files.insert(cf.path.clone());
            }
        }
        let changed = if !cf.content_hash.is_empty() {
            new_hash != cf.content_hash
        } else {
            if cf.last_modified.is_empty() {
                true // never synced
            } else {
                match chrono::DateTime::parse_from_rfc3339(&cf.last_modified) {
                    Ok(stored) => {
                        let stored_utc = stored.with_timezone(&chrono::Utc);
                        let disk_utc: chrono::DateTime<chrono::Utc> = mtime.into();
                        disk_utc > stored_utc
                    }
                    Err(_) => true, // malformed timestamp → treat as changed
                }
            }
        };

        // Keep the stored fingerprint + mtime current even when nothing
        // propagates (first hash on an upgraded graph, checkout-only churn) —
        // quiet upkeep, not a change.
        let hash_updated = new_hash != cf.content_hash;
        if hash_updated && !changed {
            update_codefile_hash(&db, &cf.id, &new_hash)?;
        }
        if !changed {
            if mtime_str != cf.last_modified {
                update_codefile_mtime(&db, &cf.id, &mtime_str)?;
            }
            continue;
        }

        files_changed += 1;

        // 1. Update CodeFile content fingerprint + last_modified
        update_codefile_hash_and_mtime(&db, &cf.id, &new_hash, &mtime_str)?;
        changes.push(SyncChange {
            path:        cf.path.clone(),
            codefile_id: cf.id.clone(),
            new_mtime:   mtime_str.clone(),
        });

        // 2. One-hop propagation: use the IMPLEMENTS edges (read-only, as an
        //    index) to find intents grounded in this file, and flag THEIR
        //    RELATES_TO neighbours needs_reverification. The IMPLEMENTS edges
        //    themselves are structural assertions and are not mutated — flagging
        //    them would leave an unresolvable state (the loop only re-inspects
        //    RELATES_TO). Each flip records a transition note naming the
        //    triggering file, so a stale edge explains itself in `loom edge
        //    show` / `loom next`.
        let cause = format!("{} changed", cf.path);
        // Retired intents take no ripple: their claims are history, not live
        // design — flipping them would resurrect work nobody owns.
        if active_intents.is_none() {
            active_intents = Some(
                crate::db::queries::list_active_intents(&db)?
                    .into_iter()
                    .map(|i| i.id)
                    .collect(),
            );
        }
        let active = active_intents.as_ref().expect("active intents loaded above");
        let intent_ids = intents_by_codefile.get(cf.id.as_str()).cloned().unwrap_or_default();
        for iid in intent_ids.iter().filter(|i| active.contains(*i)) {
            relates_to_flagged += flag_relates_to_for_intent_with_indexes(
                &db,
                iid,
                &cause,
                &now,
                &relates_by_intent,
                &mut related_edges_flagged,
            )?;
            // A passing quality verdict is a claim about the old code — flip
            // it to needs_reverification so green is re-earned.
            targets_flagged += flag_targets_for_intent_with_indexes(
                &db,
                iid,
                &cause,
                &now,
                &targets_by_intent,
                &mut targets_edges_flagged_ids,
            )?;
            governs_flagged += flag_governs_for_intent_with_indexes(
                &db,
                iid,
                &cause,
                &now,
                &governs_by_intent,
                &mut governs_edges_flagged_ids,
            )?;
        }

        // 3. Invalidate Validation.last_result for those intents
        validations_invalidated += invalidate_validations_for_intents_with_indexes(
            &db,
            &intent_ids,
            &validates_by_intent,
            &validation_by_id,
            &mut invalidated_validation_ids,
        )?;
    }

    // 4. Grounding-truth pass over every file present on disk:
    //    a) re-extract static imports (the physical-plane evidence that
    //       smells/discovery reconcile against the semantic graph), and
    //    b) verify every IMPLEMENTS locator still occurs in its file —
    //       a renamed symbol must not leave a grounding silently pointing
    //       at nothing.
    let mut locators_stale: Vec<String> = Vec::new();
    for cf in &codefiles {
        if let Some(content) = text_contents.get(&cf.path) {
            let imports = crate::repo::extract_imports(&base, &cf.path, content);
            let imports_json = serde_json::to_string(&imports)?;
            if imports_json != cf.imports {
                update_codefile_imports(&db, &cf.id, &imports_json)?;
            }
        } else if non_utf8_files.contains(&cf.path) {
            // Present but unreadable as text (binary/non-UTF8). Never skip
            // silently: any non-empty locator on such a file cannot be
            // verified — surface it instead of letting it rot.
            for im in &all_implements {
                if im.codefile_path == cf.path && !im.locator.trim().is_empty() {
                    locators_stale.push(format!(
                        "{} @ '{}' (intent '{}') — file is not readable as text; locator unverifiable",
                        im.codefile_path, im.locator, im.intent_name
                    ));
                }
            }
        }
    }
    for im in &all_implements {
        let Some(content) = text_contents.get(&im.codefile_path) else {
            continue; // missing or unreadable — reported above
        };
        if !crate::repo::locator_present(content, &im.locator) {
            locators_stale.push(format!(
                "{} @ '{}' (intent '{}')",
                im.codefile_path, im.locator, im.intent_name
            ));
            if im.inspection_status == "passing" {
                db.execute(&format!(
                    "MATCH (i:Intent {{id: '{iid}'}})-[e:IMPLEMENTS]->(cf:CodeFile {{id: '{cfid}'}}) \
                     SET e.inspection_status = 'needs_reverification'",
                    iid = esc(&im.intent_id),
                    cfid = esc(&im.codefile_id),
                ))?;
            }
        }
    }

    // Stamp the graph as reconciled against disk (freshness signal).
    set_last_synced(&db, &chrono::Utc::now().to_rfc3339())?;

    let report = SyncReport {
        files_checked,
        files_changed,
        relates_to_edges_flagged: relates_to_flagged,
        targets_edges_flagged: targets_flagged,
        governs_edges_flagged: governs_flagged,
        validations_invalidated,
        missing_files,
        locators_stale,
        changes,
    };

    // Bounded rendering: sync runs after every code churn, and a big rebase
    // can list hundreds of files — flooding the context window evicts the
    // driving agent's plan. Counts stay exact; lists are capped.
    const REPORT_CAP: usize = 20;
    let next_step = if report.files_changed == 0 && report.missing_files.is_empty() {
        "`loom status` (or `loom next --all` for closeout)".to_string()
    } else {
        format!(
            "`loom next --mode fix{}` to re-inspect flagged edges{}",
            // A big flagged queue is exactly what the bulk read exists for:
            // grouped by staling file + one `loom batch` per group.
            if report.relates_to_edges_flagged > 10 { " --take 20" } else { "" },
            if report.governs_edges_flagged > 0 {
                ", and `loom next --mode quality` to re-earn flagged quality green."
            } else {
                "."
            }
        )
    };

    if printer.json {
        let mut v = serde_json::to_value(&report)?;
        let obj = v.as_object_mut().expect("SyncReport serializes to an object");
        for (key, total_key) in [
            ("changes", "changes_total"),
            ("missing_files", "missing_files_total"),
            ("locators_stale", "locators_stale_total"),
        ] {
            let total = obj.get(key).and_then(|a| a.as_array()).map_or(0, |a| a.len());
            if let Some(arr) = obj.get_mut(key).and_then(|a| a.as_array_mut()) {
                arr.truncate(REPORT_CAP);
            }
            obj.insert(total_key.to_string(), total.into());
        }
        printer.print_json(&crate::output::with_anchor(v, &db, &next_step)?);
    } else {
        println!("── loom sync ────────────────────────────────────────────────────────");
        println!("  Files checked:                 {}", report.files_checked);
        println!("  Files changed since last sync: {}", report.files_changed);
        println!("  RELATES_TO edges flagged:      {}", report.relates_to_edges_flagged);
        println!("  GOVERNS verdicts flagged:      {}", report.governs_edges_flagged);
        println!("  TARGETS edges flagged:         {}", report.targets_edges_flagged);
        println!("  Validations invalidated:       {}", report.validations_invalidated);
        if !report.changes.is_empty() {
            println!();
            println!("  Changed files ({}):", report.changes.len());
            for c in report.changes.iter().take(REPORT_CAP) {
                println!("    {}  (mtime → {})", c.path, c.new_mtime);
            }
            if let Some(m) = crate::output::more_marker(report.changes.len(), REPORT_CAP, "`loom next --mode fix`") {
                println!("    {m}");
            }
        }
        if !report.missing_files.is_empty() {
            println!();
            println!("  ⚠ Registered files MISSING on disk ({} — deleted/renamed?):", report.missing_files.len());
            for p in report.missing_files.iter().take(REPORT_CAP) {
                println!("    {}", p);
            }
            if let Some(m) = crate::output::more_marker(report.missing_files.len(), REPORT_CAP, "`loom report`") {
                println!("    {m}");
            }
            println!("    → `loom codefile remove <path>` to drop a phantom, or restore the file.");
        }
        if !report.locators_stale.is_empty() {
            println!();
            println!("  ⚠ STALE locators ({} — symbol renamed/moved? grounding flipped to needs_reverification):", report.locators_stale.len());
            for l in report.locators_stale.iter().take(REPORT_CAP) {
                println!("    {}", l);
            }
            if let Some(m) = crate::output::more_marker(report.locators_stale.len(), REPORT_CAP, "`loom next --mode fix`") {
                println!("    {m}");
            }
            println!("    → re-ground: `loom edge implement <intent> <path> --locator \"<current symbol>\"`.");
        }
        println!();
        if report.files_changed == 0 && report.missing_files.is_empty() {
            println!("  ✓ All files up to date — no edges need reverification.");
        } else if report.relates_to_edges_flagged + report.governs_edges_flagged > 0 {
            println!("  Each flagged edge carries a transition note naming the changed file (`loom edge show <id>`).");
        }
        crate::output::print_anchor(&db, &next_step)?;
    }

    Ok(())
}


// ---------------------------------------------------------------------------
// Helper: flag RELATES_TO edges for one intent → needs_reverification
// Returns count of edges updated.
// ---------------------------------------------------------------------------

fn flag_relates_to_for_intent_with_indexes(
    db: &GrafeoDb,
    intent_id: &str,
    cause: &str,
    now: &str,
    relates_by_intent: &HashMap<&str, Vec<&crate::types::RelatesTo>>,
    already_flagged: &mut HashSet<String>,
) -> Result<usize> {
    let mut count = 0usize;
    let Some(edges) = relates_by_intent.get(intent_id) else {
        return Ok(0);
    };
    for edge in edges {
        if (edge.inspection_status == "passing" || edge.inspection_status == "independent")
            && already_flagged.insert(edge.id.clone())
        {
            db.execute(&format!(
                "MATCH (a:Intent {{id: '{from}'}})-[r:RELATES_TO]->(b:Intent {{id: '{to}'}}) \
                 SET r.inspection_status = 'needs_reverification'",
                from = esc(&edge.from_id),
                to   = esc(&edge.to_id),
            ))?;
            record_sync_flip(
                db, "edge", &edge.id, &edge.inspection_status,
                "needs_reverification", cause, now,
            )?;
            count += 1;
        }
    }
    Ok(count)
}

fn flag_governs_for_intent_with_indexes(
    db: &GrafeoDb,
    intent_id: &str,
    cause: &str,
    now: &str,
    governs_by_intent: &HashMap<&str, Vec<&crate::types::Governs>>,
    already_flagged: &mut HashSet<String>,
) -> Result<usize> {
    let mut count = 0usize;
    let Some(edges) = governs_by_intent.get(intent_id) else {
        return Ok(0);
    };
    for edge in edges {
        if edge.inspection_status == "passing" && already_flagged.insert(edge.id.clone()) {
            db.execute(&format!(
                "MATCH (r:QualityRule {{id: '{rid}'}})-[e:GOVERNS]->(i:Intent {{id: '{iid}'}}) \
                 SET e.inspection_status = 'needs_reverification'",
                rid = esc(&edge.rule_id),
                iid = esc(&edge.intent_id),
            ))?;
            if !cause.is_empty() {
                record_sync_flip(
                    db, "edge", &edge.id, "passing", "needs_reverification", cause, now,
                )?;
            }
            count += 1;
        }
    }
    Ok(count)
}

fn flag_targets_for_intent_with_indexes(
    db: &GrafeoDb,
    intent_id: &str,
    cause: &str,
    now: &str,
    targets_by_intent: &HashMap<&str, Vec<&crate::types::TargetsEdge>>,
    already_flagged: &mut HashSet<String>,
) -> Result<usize> {
    let mut count = 0usize;
    let Some(edges) = targets_by_intent.get(intent_id) else {
        return Ok(0);
    };
    for edge in edges {
        if edge.inspection_status == "passing" && already_flagged.insert(edge.id.clone()) {
            db.execute(&format!(
                "MATCH (h:Hypothesis {{id: '{hid}'}})-[e:TARGETS]->(i:Intent {{id: '{iid}'}}) \
                 SET e.inspection_status = 'needs_reverification', e.notes = '{notes}'",
                hid = esc(&edge.hypothesis_id),
                iid = esc(&edge.intent_id),
                notes = esc(&format!("stale: {cause}")),
            ))?;
            if !cause.is_empty() {
                record_sync_flip(
                    db, "edge", &edge.id, "passing", "needs_reverification", cause, now,
                )?;
            }
            count += 1;
        }
    }
    Ok(count)
}
