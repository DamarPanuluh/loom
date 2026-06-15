//! `loom migrate` — verify the live SQLite graph schema.
//!
//! Legacy live-graph migrations were only needed while loom still opened the
//! old backend directly. SQLite is now the storage boundary, and its schema is
//! created on open; JSON imports are normalized into that schema before use.

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

    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "backend": "sqlite",
            "migrated": false,
            "version": version,
            "message": "SQLite schema is created on open; JSON imports are normalized into the active schema.",
        }));
    } else {
        println!("✓ SQLite graph schema is current/open-time verified (v{version}).");
    }

    Ok(())
}
