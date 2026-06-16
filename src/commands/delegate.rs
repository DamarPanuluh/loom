//! `loom delegate` — federation: hand a subtree to a child graph.
//!
//! In a monorepo, the root graph shouldn't blanket-`ignore` service subtrees —
//! they ARE covered, just by other graphs. A delegation records that boundary
//! against a verifiable artifact (the child's committed export), so root-level
//! `loom coverage` composes across the federation instead of going blind.

use anyhow::Result;
use uuid::Uuid;

use crate::cli::DelegateCmd;
use crate::db::{ensure_initialized, GraphReadHandle, GraphReadRepository};
use crate::output::Printer;
use crate::types::Delegation;

pub fn run(cmd: DelegateCmd, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    match cmd {
        DelegateCmd::List => {
            let db = GraphReadHandle::open(&cwd)?;
            run_list_with_db(&db, &cwd, printer)
        }
        DelegateCmd::Add {
            pattern,
            target,
            author,
        } => {
            ensure_initialized(&cwd)?;
            run_add_with_sqlite(&cwd, pattern, target, author, printer)
        }
        DelegateCmd::Remove { pattern } => {
            ensure_initialized(&cwd)?;
            run_remove_with_sqlite(&cwd, pattern, printer)
        }
    }
}

fn run_add_with_sqlite(
    root: &std::path::Path,
    pattern: String,
    target: String,
    author: Option<String>,
    printer: &Printer,
) -> Result<()> {
    let by = crate::gate::acting_in_lane(&crate::gate::lane::ADD_DELEGATION, author.as_deref())?;
    glob::Pattern::new(&pattern)
        .map_err(|e| anyhow::anyhow!("Invalid glob '{}': {}", pattern, e))?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    if store
        .list_delegations()?
        .iter()
        .any(|delegation| delegation.pattern == pattern)
    {
        anyhow::bail!(
            "Pattern '{}' is already delegated. Run `loom delegate list`.",
            pattern
        );
    }
    let target_exists = root.join(&target).exists();
    let delegation = Delegation {
        id: Uuid::new_v4().to_string(),
        pattern: pattern.clone(),
        target: target.clone(),
        author: by,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    store.insert_delegation(&delegation)?;
    print_add_result(&delegation, target_exists, printer);
    Ok(())
}

fn run_remove_with_sqlite(
    root: &std::path::Path,
    pattern: String,
    printer: &Printer,
) -> Result<()> {
    crate::gate::acting_in_lane(&crate::gate::lane::REMOVE_DELEGATION, None)?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    let Some(delegation) = store.delete_delegation(&pattern)? else {
        anyhow::bail!(
            "Pattern '{}' is not delegated. Run `loom delegate list`.",
            pattern
        );
    };
    print_remove_result(&delegation, printer);
    Ok(())
}

fn print_add_result(delegation: &Delegation, target_exists: bool, printer: &Printer) {
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok", "delegation": delegation, "target_exists": target_exists,
        }));
    } else {
        println!(
            "✓ Delegated '{}' → {}",
            delegation.pattern, delegation.target
        );
        if target_exists {
            println!(
                "  Child export found — `loom coverage` now buckets matching files as delegated."
            );
        } else {
            println!(
                "  ⚠ Child export NOT found at {} — the child graph must `loom export`",
                delegation.target
            );
            println!("    (and commit it) for this boundary to be verifiable.");
        }
    }
}

fn print_remove_result(delegation: &Delegation, printer: &Printer) {
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "removed": delegation,
            "next_step": "loom coverage --json",
        }));
    } else {
        println!(
            "✓ Removed delegation '{}' → {}",
            delegation.pattern, delegation.target
        );
        println!("  → Next: loom coverage --json");
    }
}

fn run_list_with_db(
    db: &dyn GraphReadRepository,
    root: &std::path::Path,
    printer: &Printer,
) -> Result<()> {
    let ds = db.list_delegations()?;
    if printer.json {
        let items: Vec<_> = ds
            .iter()
            .map(|d| {
                serde_json::json!({
                    "id": d.id, "pattern": d.pattern, "target": d.target,
                    "author": d.author, "created_at": d.created_at,
                    "target_exists": root.join(&d.target).exists(),
                })
            })
            .collect();
        printer.print_json(&serde_json::json!({
            "delegations": items,
            "total": items.len(),
            "truncated": false,
        }));
    } else if ds.is_empty() {
        println!("(no delegations — this graph covers its whole tree itself)");
    } else {
        for d in &ds {
            let mark = if root.join(&d.target).exists() {
                "✓"
            } else {
                "✗ MISSING"
            };
            println!(
                "  {pattern:<35} → {target}  [{mark}]  ({author})",
                pattern = d.pattern,
                target = d.target,
                author = d.author
            );
        }
    }
    Ok(())
}
