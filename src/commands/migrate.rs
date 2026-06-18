//! `loom migrate` — verify the live SQLite graph schema version.
//!
//! There is no in-place upgrade step: SQLite is the storage boundary, its schema
//! is created on open, and JSON imports are normalized into the active schema. A
//! graph stamped by an OLDER loom is not rewritten here — it is rebuilt by
//! re-exporting from that loom, then `loom init . && loom import` in this one.
//! This command only reports whether the live version matches this binary.

use anyhow::Result;

use crate::db::schema::SCHEMA_VERSION;
use crate::db::{ensure_initialized, sqlite_db_path};
use crate::output::Printer;

pub fn run(printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    ensure_initialized(&cwd)?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&sqlite_db_path(&cwd))?;
    let version = store
        .graph_meta()?
        .map(|meta| meta.version)
        .unwrap_or_else(|| SCHEMA_VERSION.to_string());
    let current = version == SCHEMA_VERSION;
    let rebuild = "re-export from the loom that wrote this graph, then `loom init . && loom import loom.graph.json` here";

    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "backend": "sqlite",
            "migrated": false,
            "version": version,
            "expected": SCHEMA_VERSION.to_string(),
            "current": current,
            "next_step": if current { serde_json::Value::Null } else { serde_json::json!(rebuild) },
            "message": if current {
                "schema version matches this binary; no migration needed (schema is created on open).".to_string()
            } else {
                format!("graph is v{version}, this loom expects v{SCHEMA_VERSION} — no in-place upgrade exists; {rebuild}.")
            },
        }));
    } else if current {
        println!("✓ Graph schema version v{version} matches this loom — no migration needed.");
    } else {
        println!("✗ Graph schema is v{version}, this loom expects v{SCHEMA_VERSION}.");
        println!("    → no in-place upgrade: {rebuild}.");
    }

    Ok(())
}
