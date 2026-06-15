//! `loom coverage` — reconcile files on disk against the graph so nothing is
//! silently missed. Every (non-gitignored) file is grounded, excluded, or a gap.

use anyhow::Result;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

use crate::db::queries::{symbol_accountability_from_parts_with_notes, QuerySnapshot};
use crate::db::{GraphReadHandle, GraphReadRepository};
use crate::output::Printer;
use crate::types::{CodeFile, Implements, Intent};

pub fn run(summary: bool, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db = GraphReadHandle::open(&cwd)?;
    run_with_db(&db, &cwd, summary, printer)
}

pub fn run_with_db(
    db: &dyn GraphReadRepository,
    root: &std::path::Path,
    summary: bool,
    printer: &Printer,
) -> Result<()> {
    let disk = crate::repo::walk_files(root)?;
    let snapshot = db.query_snapshot()?;
    let codefiles = &snapshot.codefiles;
    let registered: HashSet<String> = codefiles.iter().map(|c| c.path.clone()).collect();
    let grounded = grounded_paths_from_snapshot(&snapshot);
    let mut pattern_errors = Vec::new();
    let patterns: Vec<glob::Pattern> = db
        .list_ignores()?
        .into_iter()
        .filter_map(|i| match glob::Pattern::new(&i.pattern) {
            Ok(p) => Some(p),
            Err(e) => {
                pattern_errors.push(format!("ignore '{}': {}", i.pattern, e));
                None
            }
        })
        .collect();
    let is_ignored = |p: &str| patterns.iter().any(|pat| pat.matches(p));
    // Delegated subtrees: covered by a CHILD graph (federation), verified
    // against its committed export rather than blanket-excluded.
    let delegations = db.list_delegations()?;
    let delegation_pats: Vec<(glob::Pattern, &str)> = delegations
        .iter()
        .filter_map(|d| match glob::Pattern::new(&d.pattern) {
            Ok(p) => Some((p, d.target.as_str())),
            Err(e) => {
                pattern_errors.push(format!(
                    "delegation '{}' -> '{}': {}",
                    d.pattern, d.target, e
                ));
                None
            }
        })
        .collect();
    let is_delegated = |p: &str| delegation_pats.iter().any(|(pat, _)| pat.matches(p));
    let missing_targets: Vec<&str> = delegations
        .iter()
        .filter(|d| !root.join(&d.target).exists())
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
    let pct = if total == 0 {
        100.0
    } else {
        covered as f64 / total as f64 * 100.0
    };
    let symbol_diagnostics = symbol_diagnostics(
        codefiles,
        &grounded,
        &snapshot.intents,
        &snapshot.implements,
    )?;
    let decision_notes = db.notes_by_kind("decision")?;
    let symbol_accountability = symbol_accountability_from_parts_with_notes(
        &snapshot.codefiles,
        &snapshot.intents,
        &snapshot.implements,
        &decision_notes,
    );

    if summary {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "summary": true,
                "total_files":           total,
                "grounded":              grounded_n,
                "delegated":             delegated_n,
                "excluded":              excluded_n,
                "registered_ungrounded": ungrounded.len(),
                "unaccounted":           unaccounted.len(),
                "coverage_pct":          (pct * 10.0).round() / 10.0,
                "delegation_targets_missing": missing_targets.len(),
                "pattern_errors":        pattern_errors.len(),
                "symbol_coverage":       &symbol_diagnostics.coverage,
                "symbol_accountability": &symbol_accountability.summary,
                "actionable_symbol_gaps_shown": symbol_accountability
                    .actionable_symbol_gaps
                    .iter()
                    .take(10)
                    .collect::<Vec<_>>(),
                "actionable_symbol_gaps_total": symbol_accountability.actionable_symbol_gaps.len(),
                "adjudicated_symbol_gaps_total": symbol_accountability.adjudicated_symbol_gaps.len(),
                "note": "Summary mode omits full file, symbol, raw-gap, and adjudication archives. Use `loom coverage --json` only when per-item evidence is needed.",
            }));
        } else {
            println!("── loom coverage summary ────────────────────────────────────────────");
            println!(
                "  {covered}/{total} files covered ({pct:.1}%)   [grounded {grounded_n} + delegated {delegated_n} + excluded {excluded_n}]"
            );
            println!("  registered but ungrounded: {}", ungrounded.len());
            println!("  unaccounted: {}", unaccounted.len());
            println!("  delegation targets missing: {}", missing_targets.len());
            println!("  pattern errors: {}", pattern_errors.len());
            if symbol_diagnostics.coverage.total > 0 {
                println!(
                    "  symbols grounded: {} / {} ({:.1}%)",
                    symbol_diagnostics.coverage.grounded,
                    symbol_diagnostics.coverage.total,
                    symbol_diagnostics.coverage.coverage_pct
                );
            }
            if symbol_accountability.summary.total_symbols > 0 {
                let s = &symbol_accountability.summary;
                println!(
                    "  symbol accountability: {} open gaps · {} raw gaps · {} adjudicated",
                    s.actionable_gaps, s.raw_actionable_gaps, s.adjudicated
                );
            }
            println!("  Full detail: `loom coverage --json`.");
        }
        return Ok(());
    }

    if printer.json {
        let mut payload = serde_json::json!({
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
            "pattern_errors":        pattern_errors,
            "symbol_coverage":       symbol_diagnostics.coverage,
            "ungrounded_symbols":    symbol_diagnostics.ungrounded,
            "symbol_accountability": symbol_accountability.summary,
            "raw_actionable_symbol_gaps": symbol_accountability.raw_actionable_symbol_gaps,
            "actionable_symbol_gaps": symbol_accountability.actionable_symbol_gaps,
            "adjudicated_symbol_gaps": symbol_accountability.adjudicated_symbol_gaps,
            "symbol_teaching":       symbol_accountability.teaching,
        });
        // Parity (invariant 2): carry the human remediation into json so the
        // --json-driven agent isn't left to infer the next move.
        if !unaccounted.is_empty() {
            payload["note"] = serde_json::json!(
                "unaccounted files: map with `loom codefile add` + `loom edge implement`, or exclude with `loom ignore add <glob> --reason …`"
            );
        }
        printer.print_json(&payload);
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
    if !pattern_errors.is_empty() {
        println!("  ⚠ invalid stored glob pattern(s) — fix or remove them:");
        for e in &pattern_errors {
            println!("      - {}", e);
        }
    }
    println!(
        "  unexplained (registered, no intent): {}",
        ungrounded.len()
    );
    println!(
        "  unaccounted (not mapped, not excluded): {}",
        unaccounted.len()
    );

    if !ungrounded.is_empty() {
        println!();
        println!("Unexplained code — registered but no intent grounds it (dead-code / missing-intent candidates):");
        for f in ungrounded.iter().take(40) {
            println!("  ? {}", f);
        }
        if let Some(m) = crate::output::more_marker(
            ungrounded.len(),
            40,
            "`loom coverage --json` for the full list",
        ) {
            println!("  {m}");
        }
    }
    if !unaccounted.is_empty() {
        println!();
        println!("Unaccounted — map (`loom codefile add` + `loom edge implement`) or exclude (`loom ignore add <glob> --reason …`):");
        for f in unaccounted.iter().take(40) {
            println!("  - {}", f);
        }
        if let Some(m) = crate::output::more_marker(
            unaccounted.len(),
            40,
            "`loom coverage --json` for the full list",
        ) {
            println!("  {m}");
        }
    }
    if unaccounted.is_empty() && ungrounded.is_empty() {
        println!();
        println!(
            "  ✓ Every file is grounded in an intent or explicitly excluded — nothing missed."
        );
    }
    if symbol_diagnostics.coverage.total > 0 {
        println!();
        println!(
            "Symbol diagnostics — {} / {} symbols mentioned by non-empty locators ({:.1}%)",
            symbol_diagnostics.coverage.grounded,
            symbol_diagnostics.coverage.total,
            symbol_diagnostics.coverage.coverage_pct
        );
        if symbol_diagnostics.ungrounded.is_empty() {
            println!("  ✓ Every extracted symbol is mentioned by a grounding locator.");
        } else {
            for item in symbol_diagnostics.ungrounded.iter().take(40) {
                println!("  ? {} @ {}", item.symbol, item.path);
            }
            if let Some(m) = crate::output::more_marker(
                symbol_diagnostics.ungrounded.len(),
                40,
                "`loom coverage --json` for the full list",
            ) {
                println!("  {m}");
            }
        }
    }
    if symbol_accountability.summary.total_symbols > 0 {
        let s = &symbol_accountability.summary;
        println!();
        println!(
            "Symbol accountability — {} / {} required symbols resolved ({:.1}%)",
            s.grounded + s.accepted + s.adjudicated,
            s.required,
            s.resolved_pct
        );
        println!(
            "  grounded {} · accepted {} · adjudicated {} · open gaps {} · raw gaps {} · support {} · tests {} · debris candidates {}",
            s.grounded,
            s.accepted,
            s.adjudicated,
            s.actionable_gaps,
            s.raw_actionable_gaps,
            s.support,
            s.test_support,
            s.debris_candidates
        );
        if symbol_accountability.actionable_symbol_gaps.is_empty() {
            println!("  ✓ No open actionable symbol gaps.");
        } else {
            for gap in symbol_accountability.actionable_symbol_gaps.iter().take(20) {
                println!(
                    "  ? {} @ {}:{} — {}",
                    gap.label, gap.path, gap.line_start, gap.reason
                );
            }
            if let Some(m) = crate::output::more_marker(
                symbol_accountability.actionable_symbol_gaps.len(),
                20,
                "`loom coverage --json` for the full list",
            ) {
                println!("  {m}");
            }
        }
        if !symbol_accountability.adjudicated_symbol_gaps.is_empty() {
            println!(
                "  adjudicated by current decision notes: {}",
                symbol_accountability.adjudicated_symbol_gaps.len()
            );
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
struct SymbolCoverage {
    grounded: usize,
    total: usize,
    coverage_pct: f64,
}

#[derive(Debug, Clone, Serialize)]
struct UngroundedSymbol {
    path: String,
    symbol: String,
}

#[derive(Debug, Clone)]
struct SymbolDiagnostics {
    coverage: SymbolCoverage,
    ungrounded: Vec<UngroundedSymbol>,
}

fn grounded_paths_from_snapshot(snapshot: &QuerySnapshot) -> HashSet<String> {
    let active_ids: HashSet<&str> = snapshot
        .intents
        .iter()
        .map(|intent| intent.id.as_str())
        .collect();
    snapshot
        .implements
        .iter()
        .filter(|implements| active_ids.contains(implements.intent_id.as_str()))
        .map(|implements| implements.codefile_path.clone())
        .collect()
}

fn symbol_diagnostics(
    codefiles: &[CodeFile],
    grounded_paths: &HashSet<String>,
    intents: &[Intent],
    implements: &[Implements],
) -> Result<SymbolDiagnostics> {
    let active_ids: HashSet<&str> = intents.iter().map(|intent| intent.id.as_str()).collect();
    let mut locators_by_path: HashMap<String, Vec<String>> = HashMap::new();
    for im in implements {
        if active_ids.contains(im.intent_id.as_str()) && grounded_paths.contains(&im.codefile_path)
        {
            locators_by_path
                .entry(im.codefile_path.clone())
                .or_default()
                .push(im.locator.clone());
        }
    }

    let mut total = 0usize;
    let mut grounded = 0usize;
    let mut ungrounded = Vec::new();
    for cf in codefiles {
        if !grounded_paths.contains(&cf.path) {
            continue;
        }
        let locators = locators_by_path.get(&cf.path).cloned().unwrap_or_default();
        for symbol in &cf.symbols {
            total += 1;
            if symbol_is_grounded(symbol, &locators) {
                grounded += 1;
            } else {
                ungrounded.push(UngroundedSymbol {
                    path: cf.path.clone(),
                    symbol: symbol.clone(),
                });
            }
        }
    }
    ungrounded.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.symbol.cmp(&b.symbol)));
    let coverage_pct = if total == 0 {
        100.0
    } else {
        (grounded as f64 / total as f64 * 1000.0).round() / 10.0
    };
    Ok(SymbolDiagnostics {
        coverage: SymbolCoverage {
            grounded,
            total,
            coverage_pct,
        },
        ungrounded,
    })
}

fn symbol_is_grounded(symbol: &str, locators: &[String]) -> bool {
    let ident = symbol_identifier(symbol);
    locators.iter().any(|locator| {
        let l = locator.trim();
        if l.is_empty() {
            return false;
        }
        l == symbol || l.contains(symbol) || contains_identifier_word(l, ident)
    })
}

fn symbol_identifier(symbol: &str) -> &str {
    symbol
        .split_whitespace()
        .last()
        .unwrap_or(symbol)
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
}

fn contains_identifier_word(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let bytes = haystack.as_bytes();
    let needle_bytes = needle.as_bytes();
    for (idx, _) in haystack.match_indices(needle) {
        let before = idx
            .checked_sub(1)
            .and_then(|i| bytes.get(i))
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_');
        let after_idx = idx + needle_bytes.len();
        let after = bytes
            .get(after_idx)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_');
        if !before && !after {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_diagnostics_report_ungrounded_symbols_without_gating_files() {
        let intents = vec![
            Intent {
                id: "i".into(),
                name: "Run things".into(),
                description: "runs the main flow".into(),
                abstraction_level: "feature".into(),
                domain: String::new(),
                layer: String::new(),
                source_refs: Vec::new(),
                status: "active".into(),
                aspect: "happy".into(),
                tags: Vec::new(),
                visibility: "internal".into(),
                boundary: String::new(),
                lifecycle: "implemented".into(),
                created_at: "t".into(),
                updated_at: "t".into(),
            },
            Intent {
                id: "j".into(),
                name: "Use users".into(),
                description: "uses the user symbol".into(),
                abstraction_level: "feature".into(),
                domain: String::new(),
                layer: String::new(),
                source_refs: Vec::new(),
                status: "active".into(),
                aspect: "happy".into(),
                tags: Vec::new(),
                visibility: "internal".into(),
                boundary: String::new(),
                lifecycle: "implemented".into(),
                created_at: "t".into(),
                updated_at: "t".into(),
            },
        ];
        let codefiles = vec![CodeFile {
            id: "cf".into(),
            path: "src/run.rs".into(),
            language: "rust".into(),
            last_modified: String::new(),
            imports: Vec::new(),
            symbols: vec!["fn run".into(), "struct Worker".into(), "class User".into()],
            symbol_facts: Vec::new(),
            content_hash: String::new(),
        }];
        let implements = vec![
            Implements {
                id: crate::db::schema::edge_key(crate::db::schema::edge::IMPLEMENTS, "i", "cf"),
                intent_id: "i".into(),
                codefile_id: "cf".into(),
                intent_name: "Run things".into(),
                codefile_path: "src/run.rs".into(),
                inspection_status: "passing".into(),
                criterion: String::new(),
                confidence: 0.0,
                evidence: String::new(),
                last_inspected: String::new(),
                inspected_by: String::new(),
                locator: "fn run".into(),
                notes: String::new(),
                created_at: "t".into(),
            },
            Implements {
                id: crate::db::schema::edge_key(crate::db::schema::edge::IMPLEMENTS, "j", "cf"),
                intent_id: "j".into(),
                codefile_id: "cf".into(),
                intent_name: "Use users".into(),
                codefile_path: "src/run.rs".into(),
                inspection_status: "passing".into(),
                criterion: String::new(),
                confidence: 0.0,
                evidence: String::new(),
                last_inspected: String::new(),
                inspected_by: String::new(),
                locator: "User".into(),
                notes: String::new(),
                created_at: "t".into(),
            },
        ];
        let grounded = HashSet::from(["src/run.rs".to_string()]);
        let diag = symbol_diagnostics(&codefiles, &grounded, &intents, &implements).unwrap();

        assert_eq!(diag.coverage.total, 3);
        assert_eq!(diag.coverage.grounded, 2);
        assert_eq!(diag.ungrounded.len(), 1);
        assert_eq!(diag.ungrounded[0].symbol, "struct Worker");
        assert!(symbol_is_grounded("fn run", &["run()".to_string()]));
        assert!(!symbol_is_grounded("fn run", &["runtime".to_string()]));
    }
}
