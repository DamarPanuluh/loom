//! `loom coverage` — reconcile files on disk against the graph so nothing is
//! silently missed. Every (non-gitignored) file is grounded, excluded, or a gap.

use anyhow::Result;
use std::collections::HashSet;
use std::env;

use crate::db::queries::{grounded_paths, list_codefiles, list_delegations, list_ignores};
use crate::db::{ensure_initialized, GrafeoDb};
use crate::output::Printer;

pub fn run(printer: &Printer) -> Result<()> {
    let cwd = env::current_dir()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    let disk = crate::repo::walk_files(&cwd)?;
    let registered: HashSet<String> =
        list_codefiles(&db)?.into_iter().map(|c| c.path).collect();
    let grounded: HashSet<String> = grounded_paths(&db)?.into_iter().collect();
    let patterns: Vec<glob::Pattern> = list_ignores(&db)?
        .into_iter()
        .filter_map(|i| glob::Pattern::new(&i.pattern).ok())
        .collect();
    let is_ignored = |p: &str| patterns.iter().any(|pat| pat.matches(p));
    // Delegated subtrees: covered by a CHILD graph (federation), verified
    // against its committed export rather than blanket-excluded.
    let delegations = list_delegations(&db)?;
    let delegation_pats: Vec<(glob::Pattern, &str)> = delegations
        .iter()
        .filter_map(|d| glob::Pattern::new(&d.pattern).ok().map(|p| (p, d.target.as_str())))
        .collect();
    let is_delegated = |p: &str| delegation_pats.iter().any(|(pat, _)| pat.matches(p));
    let missing_targets: Vec<&str> = delegations
        .iter()
        .filter(|d| !cwd.join(&d.target).exists())
        .map(|d| d.target.as_str())
        .collect();

    let (mut grounded_n, mut excluded_n, mut delegated_n) = (0usize, 0usize, 0usize);
    let mut ungrounded: Vec<String> = Vec::new();
    let mut unaccounted: Vec<String> = Vec::new();
    for f in &disk {
        if grounded.contains(f) {
            grounded_n += 1;
        } else if is_delegated(f) {
            delegated_n += 1;
        } else if is_ignored(f) {
            excluded_n += 1;
        } else if registered.contains(f) {
            ungrounded.push(f.clone());
        } else {
            unaccounted.push(f.clone());
        }
    }
    let total = disk.len();
    let covered = grounded_n + excluded_n + delegated_n;
    let pct = if total == 0 { 100.0 } else { covered as f64 / total as f64 * 100.0 };

    if printer.json {
        printer.print_json(&serde_json::json!({
            "total_files":           total,
            "grounded":              grounded_n,
            "delegated":             delegated_n,
            "excluded":              excluded_n,
            "registered_ungrounded": ungrounded.len(),
            "unaccounted":           unaccounted.len(),
            "coverage_pct":          (pct * 10.0).round() / 10.0,
            "ungrounded_files":      ungrounded,
            "unaccounted_files":     unaccounted,
            "delegation_targets_missing": missing_targets,
        }));
        return Ok(());
    }

    println!("── loom coverage ────────────────────────────────────────────────────");
    println!(
        "  {covered}/{total} files covered ({pct:.1}%)   [grounded {grounded_n} + delegated {delegated_n} + excluded {excluded_n}]"
    );
    if !missing_targets.is_empty() {
        println!("  ⚠ delegation target(s) MISSING — the child graph must `loom export`:");
        for t in &missing_targets {
            println!("      - {}", t);
        }
    }
    println!("  unexplained (registered, no intent): {}", ungrounded.len());
    println!("  unaccounted (not mapped, not excluded): {}", unaccounted.len());

    if !ungrounded.is_empty() {
        println!();
        println!("Unexplained code — registered but no intent grounds it (dead-code / missing-intent candidates):");
        for f in ungrounded.iter().take(40) {
            println!("  ? {}", f);
        }
        if ungrounded.len() > 40 {
            println!("  … and {} more", ungrounded.len() - 40);
        }
    }
    if !unaccounted.is_empty() {
        println!();
        println!("Unaccounted — map (`loom codefile add` + `loom edge implement`) or exclude (`loom ignore add <glob> --reason …`):");
        for f in unaccounted.iter().take(40) {
            println!("  - {}", f);
        }
        if unaccounted.len() > 40 {
            println!("  … and {} more", unaccounted.len() - 40);
        }
    }
    if unaccounted.is_empty() && ungrounded.is_empty() {
        println!();
        println!("  ✓ Every file is grounded in an intent or explicitly excluded — nothing missed.");
    }
    Ok(())
}
