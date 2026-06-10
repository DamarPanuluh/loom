//! `loom delegate` — federation: hand a subtree to a child graph.
//!
//! In a monorepo, the root graph shouldn't blanket-`ignore` service subtrees —
//! they ARE covered, just by other graphs. A delegation records that boundary
//! against a verifiable artifact (the child's committed export), so root-level
//! `loom coverage` composes across the federation instead of going blind.

use anyhow::Result;
use uuid::Uuid;

use crate::cli::DelegateCmd;
use crate::db::queries::{insert_delegation, list_delegations};
use crate::db::{ensure_initialized, GrafeoDb};
use crate::output::Printer;
use crate::types::Delegation;

pub fn run(cmd: DelegateCmd, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    match cmd {
        DelegateCmd::Add { pattern, target, author } => {
            let by = crate::gate::acting_in_lane(
                "delegate a subtree",
                &[crate::db::schema::role::BUILDER],
                author.as_deref(),
            )?;
            glob::Pattern::new(&pattern)
                .map_err(|e| anyhow::anyhow!("Invalid glob '{}': {}", pattern, e))?;
            if list_delegations(&db)?.iter().any(|d| d.pattern == pattern) {
                anyhow::bail!(
                    "Pattern '{}' is already delegated. Run `loom delegate list`.",
                    pattern
                );
            }
            let target_exists = cwd.join(&target).exists();
            let d = Delegation {
                id:         Uuid::new_v4().to_string(),
                pattern:    pattern.clone(),
                target:     target.clone(),
                author:     by,
                created_at: chrono::Utc::now().to_rfc3339(),
            };
            insert_delegation(&db, &d)?;
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status": "ok", "delegation": d, "target_exists": target_exists,
                }));
            } else {
                println!("✓ Delegated '{}' → {}", pattern, target);
                if target_exists {
                    println!("  Child export found — `loom coverage` now buckets matching files as delegated.");
                } else {
                    println!("  ⚠ Child export NOT found at {target} — the child graph must `loom export`");
                    println!("    (and commit it) for this boundary to be verifiable.");
                }
            }
        }

        DelegateCmd::List => {
            let ds = list_delegations(&db)?;
            if printer.json {
                let items: Vec<_> = ds.iter().map(|d| serde_json::json!({
                    "id": d.id, "pattern": d.pattern, "target": d.target,
                    "author": d.author, "created_at": d.created_at,
                    "target_exists": cwd.join(&d.target).exists(),
                })).collect();
                printer.print_json(&items);
            } else if ds.is_empty() {
                println!("(no delegations — this graph covers its whole tree itself)");
            } else {
                for d in &ds {
                    let mark = if cwd.join(&d.target).exists() { "✓" } else { "✗ MISSING" };
                    println!("  {pattern:<35} → {target}  [{mark}]  ({author})",
                        pattern = d.pattern, target = d.target, author = d.author);
                }
            }
        }
    }
    Ok(())
}
