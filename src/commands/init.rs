use anyhow::Result;
use std::fs;
use std::path::Path;

use crate::db::{db_path, loom_dir, GrafeoDb, LoomDb};
use crate::db::schema::{CHECK_INITIALIZED, insert_meta, SCHEMA_VERSION};
use crate::output::Printer;

pub fn run(path_str: &str, name: Option<&str>, observed: bool, printer: &Printer) -> Result<()> {
    let target = Path::new(path_str).canonicalize()
        .unwrap_or_else(|_| Path::new(path_str).to_path_buf());

    let loom = loom_dir(&target);
    let db_file = db_path(&target);

    // Create .loom/ directory if it doesn't exist
    if !loom.exists() {
        fs::create_dir_all(&loom)?;
    }

    // Open (or create) the database
    let db = GrafeoDb::open(&db_file)?;

    // The graph's default human name is the directory it maps.
    let default_name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unnamed")
        .to_string();
    let custody = if observed { "observed" } else { "owned" };

    // Idempotency check: is there already a LoomMeta node?
    let check = db.execute(CHECK_INITIALIZED)?;
    if !check.rows().is_empty() {
        // Re-running init is safe — and it's also the identity touch-point:
        // backfill a missing graph_id (pre-identity graph), and apply
        // explicitly-passed --name/--observed (init is the only meta writer).
        let meta = crate::db::queries::get_meta(&db)?;
        let (cur_id, cur_name, cur_custody) = meta
            .map(|m| (m.graph_id, m.graph_name, m.custody))
            .unwrap_or_default();
        let new_id = if cur_id.is_empty() { uuid::Uuid::new_v4().to_string() } else { cur_id.clone() };
        let new_name = match name {
            Some(n) => n.to_string(),
            None if cur_name.is_empty() => default_name,
            None => cur_name.clone(),
        };
        let new_custody = if observed {
            "observed".to_string()
        } else if cur_custody.is_empty() {
            "owned".to_string()
        } else {
            cur_custody.clone()
        };
        let changed = new_id != cur_id || new_name != cur_name || new_custody != cur_custody;
        if changed {
            crate::db::queries::set_identity(&db, &new_id, &new_name, &new_custody)?;
        }
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "ok",
                "message": format!("Already initialised at {}", loom.display()),
                "graph_id": new_id, "graph_name": new_name, "custody": new_custody,
                "identity_updated": changed,
            }));
        } else {
            println!("✓ Already initialised at {}  (run again is safe)", loom.display());
            println!("  graph: '{}' ({})  custody: {}{}",
                new_name, new_id, new_custody,
                if changed { "  [identity updated]" } else { "" });
        }
        return Ok(());
    }

    // Insert the meta node to mark this DB as initialised
    let now = chrono::Utc::now().to_rfc3339();
    let graph_id = uuid::Uuid::new_v4().to_string();
    let graph_name = name.map(str::to_string).unwrap_or(default_name);
    db.execute(&insert_meta(SCHEMA_VERSION, &now, &graph_id, &graph_name, custody))?;

    if printer.json {
        printer.print_json(&serde_json::json!({
            "status":  "ok",
            "message": format!("Initialised loom graph at {}", loom.display()),
            "db":      db_file.display().to_string(),
            "graph_id": graph_id,
            "graph_name": graph_name,
            "custody": custody,
            "next_steps": [
                "Read the driving protocol: `loom guide`.",
                "Seed 1–3 system intents: `loom intent add --name \"…\" --level system --description \"…\"`.",
                "Then drive discovery with `loom next`.",
            ],
        }));
    } else {
        println!("✓ Initialised loom graph at {}", loom.display());
        println!("  DB:    {}", db_file.display());
        println!("  graph: '{}' ({})  custody: {}", graph_name, graph_id, custody);
        if observed {
            println!("  Observed graph: you're mapping code you don't own — build/fix lanes are");
            println!("  disabled; record findings (issue verdicts, notes), not fixes.");
        }
        println!();
        println!("  → Next: `loom guide` to learn the loop, then seed intents (`loom intent add … --level system`).");
    }
    Ok(())
}
