use anyhow::Result;
use std::fs;
use std::path::Path;

use crate::db::{db_path, loom_dir, GrafeoDb, LoomDb};
use crate::db::schema::{CHECK_INITIALIZED, insert_meta, SCHEMA_VERSION};
use crate::output::Printer;

pub fn run(path_str: &str, printer: &Printer) -> Result<()> {
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

    // Idempotency check: is there already a LoomMeta node?
    let check = db.execute(CHECK_INITIALIZED)?;
    if !check.rows().is_empty() {
        printer.success(&format!(
            "Already initialised at {}  (run again is safe — nothing changed)",
            loom.display()
        ));
        return Ok(());
    }

    // Insert the meta node to mark this DB as initialised
    let now = chrono::Utc::now().to_rfc3339();
    db.execute(&insert_meta(SCHEMA_VERSION, &now))?;

    if printer.json {
        printer.print_json(&serde_json::json!({
            "status":  "ok",
            "message": format!("Initialised loom graph at {}", loom.display()),
            "db":      db_file.display().to_string(),
            "next_steps": [
                "Read the driving protocol: `loom guide`.",
                "Seed 1–3 system intents: `loom intent add --name \"…\" --level system --description \"…\"`.",
                "Then drive discovery with `loom next`.",
            ],
        }));
    } else {
        println!("✓ Initialised loom graph at {}", loom.display());
        println!("  DB: {}", db_file.display());
        println!();
        println!("  → Next: `loom guide` to learn the loop, then seed intents (`loom intent add … --level system`).");
    }
    Ok(())
}
