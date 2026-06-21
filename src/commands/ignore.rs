use anyhow::Result;
use uuid::Uuid;

use crate::cli::IgnoreCmd;
use crate::db::{ensure_initialized, GraphReadHandle, GraphReadRepository};
use crate::output::Printer;
use crate::types::Ignore;

pub fn run(cmd: IgnoreCmd, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    match cmd {
        IgnoreCmd::List => {
            let db = GraphReadHandle::open(&cwd)?;
            run_list_with_db(&db, printer)
        }
        IgnoreCmd::Add {
            pattern,
            reason,
            author,
        } => {
            ensure_initialized(&cwd)?;
            run_add_with_sqlite(&cwd, pattern, reason, author, printer)
        }
    }
}

fn run_add_with_sqlite(
    root: &std::path::Path,
    pattern: String,
    reason: String,
    author: Option<String>,
    printer: &Printer,
) -> Result<()> {
    // An empty/blank pattern persists a junk row that pollutes `ignore list` and
    // could read as match-everything downstream. Refuse it.
    if pattern.trim().is_empty() {
        anyhow::bail!(
            "An ignore pattern can't be empty — pass a glob, e.g. `loom ignore add 'target/**' --reason \"build output\"`."
        );
    }
    let ig = Ignore {
        id: Uuid::new_v4().to_string(),
        pattern,
        reason,
        author: crate::agent::acting(author.as_deref()),
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    store.insert_ignore(&ig)?;
    let next_step =
        "`loom coverage` — files matching this pattern now count as excluded, not unaccounted.";
    if printer.json {
        let mut payload = serde_json::to_value(&ig).unwrap_or(serde_json::Value::Null);
        if let Some(obj) = payload.as_object_mut() {
            obj.insert(
                "next_step".to_string(),
                serde_json::Value::String(next_step.to_string()),
            );
        }
        printer.print_json(&payload);
    } else {
        println!("✓ Ignore pattern added: {}", ig.pattern);
        println!("  reason: {}", ig.reason);
        println!("  → Next: {next_step}");
    }
    Ok(())
}

fn run_list_with_db(db: &dyn GraphReadRepository, printer: &Printer) -> Result<()> {
    let igs = db.list_ignores()?;
    if printer.json {
        printer.print_json(&serde_json::json!({
            "ignores": igs,
            "total": igs.len(),
            "truncated": false,
        }));
    } else if igs.is_empty() {
        println!("(no ignore patterns — every non-gitignored file must be mapped or excluded)");
    } else {
        for i in &igs {
            println!("  {:<30}  — {}  ({})", i.pattern, i.reason, i.author);
        }
    }
    Ok(())
}
