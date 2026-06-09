use anyhow::Result;
use std::env;
use std::fs;
use std::path::Path;

use crate::db::{ensure_initialized, GrafeoDb, LoomDb};
use crate::db::queries::{
    edges_for_intent, flag_governs_for_intent, intent_ids_implementing_codefile,
    invalidate_validations_for_codefile, list_all_implements, list_codefiles,
    set_last_synced, update_codefile_imports, update_codefile_mtime,
};
use crate::db::schema::esc;
use crate::output::Printer;
use crate::types::{SyncChange, SyncReport};

pub fn run(path: &str, printer: &Printer) -> Result<()> {
    let base = if path == "." {
        env::current_dir()?
    } else {
        Path::new(path).canonicalize().unwrap_or_else(|_| Path::new(path).to_path_buf())
    };

    let db_file = ensure_initialized(&base)?;
    let db = GrafeoDb::open(&db_file)?;

    let codefiles = list_codefiles(&db)?;
    let files_checked = codefiles.len();

    let mut files_changed = 0usize;
    let mut relates_to_flagged = 0usize;
    let mut governs_flagged = 0usize;
    let mut validations_invalidated = 0usize;
    let mut changes: Vec<SyncChange> = Vec::new();
    let mut missing_files: Vec<String> = Vec::new();

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
            anyhow::anyhow!("Cannot read mtime for {}: {}", abs_path.display(), e)
        })?;

        let mtime_str = {
            let dt: chrono::DateTime<chrono::Utc> = mtime.into();
            dt.to_rfc3339()
        };

        // Determine whether the file changed since the stored last_modified
        let changed = if cf.last_modified.is_empty() {
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
        };

        if !changed {
            continue;
        }

        files_changed += 1;

        // 1. Update CodeFile.last_modified
        update_codefile_mtime(&db, &cf.id, &mtime_str)?;
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
        //    RELATES_TO).
        let intent_ids = intent_ids_implementing_codefile(&db, &cf.id)?;
        for iid in &intent_ids {
            let nrv = flag_relates_to_for_intent(&db, iid)?;
            relates_to_flagged += nrv;
            // A passing quality verdict is a claim about the old code — flip
            // it to needs_reverification so green is re-earned
            // (`loom next --mode quality`).
            governs_flagged += flag_governs_for_intent(&db, iid)?;
        }

        // 3. Invalidate Validation.last_result for those intents
        let n_val = invalidate_validations_for_codefile(&db, &cf.id)?;
        validations_invalidated += n_val;
    }

    // 4. Grounding-truth pass over every file present on disk:
    //    a) re-extract static imports (the physical-plane evidence that
    //       smells/discovery reconcile against the semantic graph), and
    //    b) verify every IMPLEMENTS locator still occurs in its file —
    //       a renamed symbol must not leave a grounding silently pointing
    //       at nothing.
    let mut locators_stale: Vec<String> = Vec::new();
    let mut contents: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
    for cf in &codefiles {
        let abs = if Path::new(&cf.path).is_absolute() {
            Path::new(&cf.path).to_path_buf()
        } else {
            base.join(&cf.path)
        };
        match fs::read_to_string(&abs) {
            Ok(content) => {
                let imports = crate::repo::extract_imports(&base, &cf.path, &content);
                let imports_json = serde_json::to_string(&imports)?;
                if imports_json != cf.imports {
                    update_codefile_imports(&db, &cf.id, &imports_json)?;
                }
                contents.insert(cf.path.as_str(), content);
            }
            Err(_) if abs.exists() => {
                // Present but unreadable as text (binary/non-UTF8). Never skip
                // silently: any non-empty locator on such a file cannot be
                // verified — surface it instead of letting it rot.
                for im in list_all_implements(&db)? {
                    if im.codefile_path == cf.path && !im.locator.trim().is_empty() {
                        locators_stale.push(format!(
                            "{} @ '{}' (intent '{}') — file is not readable as text; locator unverifiable",
                            im.codefile_path, im.locator, im.intent_name
                        ));
                    }
                }
            }
            Err(_) => {} // missing on disk — already reported above
        }
    }
    for im in list_all_implements(&db)? {
        let Some(content) = contents.get(im.codefile_path.as_str()) else {
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
        governs_edges_flagged: governs_flagged,
        validations_invalidated,
        missing_files,
        locators_stale,
        changes,
    };

    if printer.json {
        printer.print_json(&report);
    } else {
        println!("── loom sync ────────────────────────────────────────────────────────");
        println!("  Files checked:                 {}", report.files_checked);
        println!("  Files changed since last sync: {}", report.files_changed);
        println!("  RELATES_TO edges flagged:      {}", report.relates_to_edges_flagged);
        println!("  GOVERNS verdicts flagged:      {}", report.governs_edges_flagged);
        println!("  Validations invalidated:       {}", report.validations_invalidated);
        if !report.changes.is_empty() {
            println!();
            println!("  Changed files:");
            for c in &report.changes {
                println!("    {}  (mtime → {})", c.path, c.new_mtime);
            }
        }
        if !report.missing_files.is_empty() {
            println!();
            println!("  ⚠ Registered files MISSING on disk (deleted/renamed?):");
            for p in &report.missing_files {
                println!("    {}", p);
            }
            println!("    → `loom codefile remove <path>` to drop a phantom, or restore the file.");
        }
        if !report.locators_stale.is_empty() {
            println!();
            println!("  ⚠ STALE locators (symbol renamed/moved? grounding flipped to needs_reverification):");
            for l in &report.locators_stale {
                println!("    {}", l);
            }
            println!("    → re-ground: `loom edge implement <intent> <path> --locator \"<current symbol>\"`.");
        }
        println!();
        if report.files_changed == 0 && report.missing_files.is_empty() {
            println!("  ✓ All files up to date — no edges need reverification.");
        } else if report.files_changed > 0 {
            println!("  Run `loom next --mode fix` to re-inspect flagged edges{}",
                if report.governs_edges_flagged > 0 {
                    ", and `loom next --mode quality` to re-earn flagged quality green."
                } else { "." });
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helper: flag RELATES_TO edges for one intent → needs_reverification
// Returns count of edges updated.
// ---------------------------------------------------------------------------

fn flag_relates_to_for_intent(db: &GrafeoDb, intent_id: &str) -> Result<usize> {
    // Read every RELATES_TO edge touching this intent (node-keyed traversal is
    // reliable), filter to passing/independent in Rust, then flip each one to
    // needs_reverification keyed by its endpoints. Filtering or updating a
    // relationship by its own property in the query is unreliable in grafeo
    // 0.5.x, so we never do that.
    let mut count = 0usize;
    for edge in edges_for_intent(db, intent_id)? {
        if edge.inspection_status == "passing" || edge.inspection_status == "independent" {
            db.execute(&format!(
                "MATCH (a:Intent {{id: '{from}'}})-[r:RELATES_TO]->(b:Intent {{id: '{to}'}}) \
                 SET r.inspection_status = 'needs_reverification'",
                from = esc(&edge.from_id),
                to   = esc(&edge.to_id),
            ))?;
            count += 1;
        }
    }
    Ok(count)
}
