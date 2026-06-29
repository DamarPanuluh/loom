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
        IgnoreCmd::Remove { pattern } => {
            ensure_initialized(&cwd)?;
            run_remove_with_sqlite(&cwd, &pattern, printer)
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
    let mut store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    store.insert_ignore(&ig)?;

    // Reconcile the registry with the exclusion: a CodeFile that is BOTH
    // registered and ignored is the contradiction `loom coverage` (which excludes
    // it) and `loom status` (which counts it as "reached by no intent") used to
    // disagree on. De-register the UNGROUNDED matches now so every surface honors
    // the exclusion the same way. Grounded matches are left intact (dropping them
    // would silently discard groundings) and reported, so the operator can
    // `loom codefile remove` deliberately if that's the intent.
    let mut deregistered: Vec<String> = Vec::new();
    let mut grounded_left: Vec<String> = Vec::new();
    if let Ok(pat) = glob::Pattern::new(&ig.pattern) {
        let snapshot = store.query_snapshot()?;
        let grounded: std::collections::HashSet<&str> = snapshot
            .implements
            .iter()
            .map(|im| im.codefile_path.as_str())
            .collect();
        let matching: Vec<(String, bool)> = snapshot
            .codefiles
            .iter()
            .filter(|cf| pat.matches(&cf.path))
            .map(|cf| (cf.path.clone(), grounded.contains(cf.path.as_str())))
            .collect();
        for (path, is_grounded) in matching {
            if is_grounded {
                grounded_left.push(path);
            } else if store.delete_codefile(&path)?.is_some() {
                deregistered.push(path);
            }
        }
        deregistered.sort();
        grounded_left.sort();
    }

    let next_step =
        "`loom coverage` — files matching this pattern now count as excluded, not unaccounted.";
    if printer.json {
        let mut payload = serde_json::to_value(&ig).unwrap_or(serde_json::Value::Null);
        if let Some(obj) = payload.as_object_mut() {
            obj.insert(
                "deregistered_codefiles".to_string(),
                serde_json::json!(deregistered),
            );
            obj.insert(
                "grounded_matches_kept".to_string(),
                serde_json::json!(grounded_left),
            );
            obj.insert(
                "next_step".to_string(),
                serde_json::Value::String(next_step.to_string()),
            );
        }
        printer.print_json(&payload);
    } else {
        println!("✓ Ignore pattern added: {}", ig.pattern);
        println!("  reason: {}", ig.reason);
        if !deregistered.is_empty() {
            println!(
                "  de-registered {} now-excluded CodeFile(s) (were ungrounded): {}",
                deregistered.len(),
                deregistered.join(", ")
            );
        }
        if !grounded_left.is_empty() {
            println!(
                "  ⚠ {} matching CodeFile(s) are GROUNDED and kept: {} — `loom codefile remove <path>` to drop one (and its groundings) deliberately.",
                grounded_left.len(),
                grounded_left.join(", ")
            );
        }
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

fn run_remove_with_sqlite(root: &std::path::Path, pattern: &str, printer: &Printer) -> Result<()> {
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let removed = store.delete_ignore(pattern)?;
    if !removed {
        anyhow::bail!(
            "no ignore rule with pattern '{}' — run `loom ignore list` to see exact patterns.",
            pattern
        );
    }
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "removed": pattern,
            "next_step": "`loom coverage` — files matching this pattern are no longer excluded.",
        }));
    } else {
        println!("✓ Removed ignore rule for '{pattern}'.");
        println!("  → `loom coverage` to see any files that now show as unaccounted.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Ignore;

    #[test]
    fn ignore_remove_deletes_existing_and_errors_on_missing() {
        let dir = std::env::temp_dir().join(format!(
            "loom-ignore-remove-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        // `sqlite_db_path(root)` resolves to `root/.loom/graph.sqlite`.
        let loom_dir = dir.join(".loom");
        std::fs::create_dir_all(&loom_dir).unwrap();
        let db_path = loom_dir.join("graph.sqlite");
        {
            let store = crate::db::sqlite::SqliteGraphStore::open(&db_path).unwrap();
            store
                .insert_ignore(&Ignore {
                    id: uuid::Uuid::new_v4().to_string(),
                    pattern: "fixtures/**".into(),
                    reason: "test fixtures".into(),
                    author: "llm".into(),
                    created_at: "t".into(),
                })
                .unwrap();
        }

        let printer = Printer::new(false);
        run_remove_with_sqlite(&dir, "fixtures/**", &printer).unwrap();

        let db = crate::db::sqlite::SqliteGraphStore::open(&db_path).unwrap();
        assert!(
            db.list_ignores().unwrap().is_empty(),
            "rule must be gone after remove"
        );
        drop(db);

        // A missing pattern should error cleanly.
        let err = run_remove_with_sqlite(&dir, "fixtures/**", &printer).unwrap_err();
        assert!(
            err.to_string().contains("no ignore rule"),
            "missing pattern should give a named error: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
