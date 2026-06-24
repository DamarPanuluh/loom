//! `loom coverage` — reconcile files on disk against the graph so nothing is
//! silently missed. Every (non-gitignored) file is grounded, excluded, or a gap.

use anyhow::Result;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

use crate::db::queries::{
    contains_identifier_word, symbol_accountability_from_parts_with_notes, symbol_identifier,
    QuerySnapshot,
};
use crate::db::{GraphReadHandle, GraphReadRepository};
use crate::output::Printer;
use crate::types::{CodeFile, Implements, Intent};

pub fn run(summary: bool, adjudicated: bool, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db = GraphReadHandle::open(&cwd)?;
    run_with_db(&db, &cwd, summary, adjudicated, printer)
}

pub fn run_with_db(
    db: &dyn GraphReadRepository,
    root: &std::path::Path,
    summary: bool,
    adjudicated: bool,
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

    if adjudicated {
        return render_adjudicated_drilldown(&symbol_accountability, printer);
    }

    // Extraction fidelity: how many files' symbol facts are heuristic-grade
    // (lower trust than tree-sitter) or ungraded (legacy / never re-synced under
    // a grade-aware loom). Lets a reader weight the symbol diagnostics below.
    let low_fidelity = codefiles
        .iter()
        .filter(|c| c.extractor_grade == "low")
        .count();
    let ungraded = codefiles
        .iter()
        .filter(|c| c.extractor_grade.is_empty())
        .count();

    if summary {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "summary": true,
                "total_files":           total,
                "low_fidelity_extraction": low_fidelity,
                "ungraded_extraction":   ungraded,
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
            if low_fidelity > 0 || ungraded > 0 {
                println!(
                    "  extraction fidelity: {low_fidelity} heuristic-grade · {ungraded} ungraded (facts to weight accordingly; `loom sync` grades them)"
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
        // A freshly-registered repo has required==0 (nothing is grounded, so no
        // symbol is yet REQUIRED to carry a precise locator) → resolved_pct is a
        // vacuous 100%. Don't let that read as "all accounted for" when there is
        // an unowned public surface waiting to be claimed.
        if s.required == 0 && s.unowned_public > 0 {
            println!(
                "  ⓘ the 100% is vacuous — no intent grounds this code yet; {} public symbol(s) are unowned. Seed/ground them (`loom seed --suggest`) or exclude (`loom ignore add <glob> --reason …`).",
                s.unowned_public
            );
        }
        if symbol_accountability.actionable_symbol_gaps.is_empty() {
            // Qualify the ✓ by co-located negatives so it doesn't ride over
            // adjudication-bought green (symbols resolved by a decision note,
            // not by a grounding locator) or raw gaps the headline ignores.
            // "No OPEN actionable gaps" is true, but green earned by
            // adjudication rather than grounding is exactly the shape the
            // false-green cluster hunts — disclose it next to the ✓.
            let mut qualifiers: Vec<String> = Vec::new();
            if s.adjudicated > 0 {
                qualifiers.push(format!(
                    "{} adjudicated (bought green, not grounded)",
                    s.adjudicated
                ));
            }
            if s.raw_actionable_gaps > 0 {
                qualifiers.push(format!(
                    "{} raw gap(s) not yet actionable",
                    s.raw_actionable_gaps
                ));
            }
            if s.unowned_public > 0 {
                qualifiers.push(format!(
                    "{} public symbol(s) unowned — no intent grounds them yet",
                    s.unowned_public
                ));
            }
            if qualifiers.is_empty() {
                println!("  ✓ No open actionable symbol gaps.");
            } else {
                println!(
                    "  ✓ No open actionable symbol gaps (but {} — `loom coverage --json` for detail).",
                    qualifiers.join("; ")
                );
            }
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

/// `loom coverage --adjudicated`: turn the adjudicated-symbol COUNT into an
/// auditable per-symbol trail. Each bought symbol shows the ruling (decision
/// note) that bought its green, who ruled, when (staleness = ruling age), and
/// the condition that re-opens it — so a stale/copied/low-trust ruling can be
/// surfaced and challenged individually instead of laundering in behind a
/// single number.
///
/// Honest gap: decision notes carry no confidence field, so the drill-down
/// surfaces staleness + author (the challenge handles), not a confidence
/// grade. If graded adjudication is wanted, that's a schema change, not a view.
fn render_adjudicated_drilldown(
    symbol_accountability: &crate::db::queries::SymbolAccountabilityReport,
    printer: &Printer,
) -> Result<()> {
    let gaps = &symbol_accountability.adjudicated_symbol_gaps;
    let now = chrono::Utc::now();
    if printer.json {
        let payload = serde_json::json!({
            "scope": "adjudicated",
            "adjudicated_total": gaps.len(),
            "adjudicated_symbol_gaps": gaps,
            "note": "Decision notes carry no confidence field; staleness = ruled_at age. \
                     Challenge a ruling by re-grounding (`loom edge implement`) or by adding a \
                     newer decision note (`loom note add --kind decision`).",
            "next_step": "challenge a stale ruling: re-ground the symbol or record a newer decision note",
        });
        printer.print_json(&payload);
        return Ok(());
    }

    println!("── loom coverage · adjudication drill-down ────────────────────────");
    if gaps.is_empty() {
        println!("  ✓ No symbols resolved by adjudication — every required symbol is");
        println!("    grounded or accepted, not bought green by a decision note.");
        println!("  (Adjudication is green earned by a ruling, not a locator. When it");
        println!("   appears, this view audits each bought symbol individually.)");
        return Ok(());
    }
    println!(
        "  {} symbol(s) resolved by a decision note (bought green, not grounded).",
        gaps.len()
    );
    println!("  Each is auditable: the ruling that bought it, who ruled, when");
    println!("  (staleness = ruling age), and what would re-open it. Decision notes");
    println!("  carry no confidence field — staleness + author are the challenge handles.");
    println!();
    for (i, gap) in gaps.iter().take(40).enumerate() {
        let owners = if gap.owner_intents.is_empty() {
            "(no owner intent)".to_string()
        } else {
            gap.owner_intents.join(", ")
        };
        println!(
            "  [{}] {}:{}  {} {}",
            i + 1,
            gap.path,
            gap.line_start,
            gap.kind,
            gap.label
        );
        println!("      owner: {owners}");
        // The ruling is freeform note text — cap it so one verbose note can't
        // drown the rest of the audit trail.
        let ruling = truncate_ruling(&gap.ruling, 160);
        println!("      ruling: {ruling}");
        println!(
            "      ruled by: {}   at: {}{}",
            gap.ruled_by,
            gap.ruled_at,
            fmt_ruling_age(&gap.ruled_at, now)
        );
        println!("      re-opens when: {}", gap.reopens_when);
    }
    if let Some(m) = crate::output::more_marker(
        gaps.len(),
        40,
        "`loom coverage --adjudicated --json` for the full list",
    ) {
        println!("  {m}");
    }
    println!();
    println!("  → Challenge a stale ruling: re-ground the symbol");
    println!("    (`loom edge implement <intent> <path>`) or record a newer");
    println!("    decision note (`loom note add --kind decision --target …`).");
    Ok(())
}

/// Cap a freeform ruling so one verbose note can't drown the audit trail.
fn truncate_ruling(text: &str, max: usize) -> String {
    let stripped = text.trim();
    if stripped.chars().count() <= max {
        return stripped.to_string();
    }
    let mut out: String = stripped.chars().take(max).collect();
    out.push('…');
    out
}

/// Human staleness for a ruling: how long ago `ruled_at` landed, or an honest
/// marker when it's empty/unparseable (an untimestamped ruling is the hardest
/// to challenge — surface that, don't paper over it).
fn fmt_ruling_age(ruled_at: &str, now: chrono::DateTime<chrono::Utc>) -> String {
    if ruled_at.trim().is_empty() {
        return "  (untimestamped — no ruling age to challenge)".to_string();
    }
    let Ok(ts) = chrono::DateTime::parse_from_rfc3339(ruled_at) else {
        return "  (unparseable timestamp)".to_string();
    };
    let elapsed = now.signed_duration_since(ts.with_timezone(&chrono::Utc));
    let days = elapsed.num_days();
    if days < 0 {
        return String::new();
    }
    if days == 0 {
        let hours = elapsed.num_hours();
        return format!("  (age: {hours}h)");
    }
    if days < 30 {
        return format!("  (age: {days}d)");
    }
    if days < 365 {
        return format!("  (age: {}mo)", days / 30);
    }
    format!("  (age: {}y)", days / 365)
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
    // Current groundings only — a file grounded solely by a stale
    // (needs_reverification) or broken (failing) locator is not honestly
    // grounded; it surfaces as registered-ungrounded so the map≠territory gap
    // is visible instead of laundered into "grounded".
    snapshot
        .implements
        .iter()
        .filter(|implements| {
            active_ids.contains(implements.intent_id.as_str())
                && implements.inspection_status != "needs_reverification"
                && implements.inspection_status != "failing"
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_ruling_caps_verbose_notes_and_leaves_short_ones_intact() {
        // A short ruling is returned verbatim (trimmed), not padded or mangled.
        assert_eq!(truncate_ruling("  it's fine ", 160), "it's fine");
        // A verbose ruling is capped to the limit + an ellipsis, counted by
        // chars (not bytes) so multibyte notes don't split a codepoint.
        let long = "x".repeat(200);
        let capped = truncate_ruling(&long, 10);
        assert_eq!(capped.chars().count(), 11, "10 chars + ellipsis");
        assert!(capped.ends_with('…'));
    }

    #[test]
    fn fmt_ruling_age_marks_untimestamped_and_parses_rfc3339() {
        let now = chrono::Utc::now();
        // An untimestamped ruling is the hardest to challenge — surface it,
        // don't pretend it's fresh.
        assert!(fmt_ruling_age("", now).contains("untimestamped"));
        // A garbage timestamp is reported as unparseable, not silently fresh.
        assert!(fmt_ruling_age("not a date", now).contains("unparseable"));
        // A 10-day-old ruling reads its age in days.
        let ten_days_ago = (now - chrono::Duration::days(10)).to_rfc3339();
        assert!(fmt_ruling_age(&ten_days_ago, now).contains("age: 10d"));
        // A future timestamp yields no age marker (not negative, not fresh).
        let future = (now + chrono::Duration::days(5)).to_rfc3339();
        assert_eq!(fmt_ruling_age(&future, now), "");
    }

    #[test]
    fn symbol_diagnostics_report_ungrounded_symbols_without_gating_files() {
        let intents = vec![
            Intent {
                id: "i".into(),
                name: "Run things".into(),
                description: "runs the main flow".into(),
                criterion: String::new(),
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
                criterion: String::new(),
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
            extractor_grade: String::new(),
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
