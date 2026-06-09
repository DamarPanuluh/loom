use anyhow::Result;
use std::env;
use std::fs;
use std::path::Path;

use crate::db::{ensure_initialized, GrafeoDb, LoomDb};
use crate::db::queries::{
    edges_for_intent, intent_ids_implementing_codefile,
    invalidate_validations_for_codefile, list_codefiles, set_last_synced, update_codefile_mtime,
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
    let mut validations_invalidated = 0usize;
    let mut changes: Vec<SyncChange> = Vec::new();

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
                // File does not exist on disk; skip silently
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
        }

        // 3. Invalidate Validation.last_result for those intents
        let n_val = invalidate_validations_for_codefile(&db, &cf.id)?;
        validations_invalidated += n_val;
    }

    // Stamp the graph as reconciled against disk (freshness signal).
    set_last_synced(&db, &chrono::Utc::now().to_rfc3339())?;

    let report = SyncReport {
        files_checked,
        files_changed,
        relates_to_edges_flagged: relates_to_flagged,
        validations_invalidated,
        changes,
    };

    if printer.json {
        printer.print_json(&report);
    } else {
        println!("── loom sync ────────────────────────────────────────────────────────");
        println!("  Files checked:                 {}", report.files_checked);
        println!("  Files changed since last sync: {}", report.files_changed);
        println!("  RELATES_TO edges flagged:      {}", report.relates_to_edges_flagged);
        println!("  Validations invalidated:       {}", report.validations_invalidated);
        if !report.changes.is_empty() {
            println!();
            println!("  Changed files:");
            for c in &report.changes {
                println!("    {}  (mtime → {})", c.path, c.new_mtime);
            }
        }
        println!();
        if report.files_changed == 0 {
            println!("  ✓ All files up to date — no edges need reverification.");
        } else {
            println!("  Run `loom next --mode fix` to begin re-inspecting flagged edges.");
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
