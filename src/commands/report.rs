use anyhow::Result;

use crate::db::queries::{
    completeness_gaps, count_all_edges_by_inspection_status, count_codefiles, count_intents,
    count_validations, intents_without_validations, list_all_failing_governs, list_validations,
    recent_passing, top_intents_by_centrality, validation_pass_rate, vertical_completeness,
};
use crate::db::{ensure_initialized, GrafeoDb};
use crate::output::{fmt_status, Printer};
use crate::types::{FullReport, StatusReport};

pub fn run(printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;
    run_with_db(&db, &cwd, printer)
}

pub fn run_with_db(db: &GrafeoDb, _root: &std::path::Path, printer: &Printer) -> Result<()> {
    // ---- Node counts ----
    let total_intents = count_intents(db)?;
    let total_codefiles = count_codefiles(db)?;
    let total_validations = count_validations(db)?;

    // ---- Edge inspection_status counts (all types combined) ----
    let by_status = count_all_edges_by_inspection_status(db)?;

    let uninspected = *by_status.get("uninspected").unwrap_or(&0);
    let passing = *by_status.get("passing").unwrap_or(&0);
    let failing = *by_status.get("failing").unwrap_or(&0);
    let independent = *by_status.get("independent").unwrap_or(&0);
    let needs_reverification = *by_status.get("needs_reverification").unwrap_or(&0);
    let total_edges = by_status.values().sum::<i64>();

    // ---- Validation quality ----
    let pass_rate = validation_pass_rate(db)?;
    let (blocked_validations, validation_pass_rate_runnable) =
        crate::db::queries::blocked_count_and_runnable_rate(&list_validations(db)?);
    let intents_no_val = intents_without_validations(db)?;

    let status = StatusReport {
        total_intents,
        total_codefiles,
        total_validations,
        total_edges,
        uninspected_edges: uninspected,
        passing_edges: passing,
        failing_edges: failing,
        independent_edges: independent,
        needs_reverification,
        intents_without_validations: intents_no_val.len() as i64,
        validation_pass_rate: pass_rate,
        blocked_validations,
        validation_pass_rate_runnable,
        open_issues: failing,
    };

    let top_intents = top_intents_by_centrality(db, 5)?;
    let failing_governs = list_all_failing_governs(db)?;
    let recent = recent_passing(db, 10)?;

    let gaps = completeness_gaps(db)?;
    let vc = vertical_completeness(db)?;
    // Blocked proofs leave the validator queue (deliberate — they can't run),
    // so the report is where they stay visible until someone unblocks them.
    let blocked: Vec<_> = list_validations(db)?
        .into_iter()
        .filter(|v| v.last_result == "blocked")
        .collect();

    if printer.json {
        let report = FullReport {
            status,
            top_intents_by_centrality: top_intents,
            intents_without_validations: intents_no_val,
            failing_governs,
            recent_passing: recent,
            edge_counts_by_status: by_status,
        };
        let mut v = serde_json::to_value(&report)?;
        if let Some(obj) = v.as_object_mut() {
            obj.insert(
                "completeness_gaps".to_string(),
                serde_json::to_value(&gaps)?,
            );
            obj.insert(
                "vertical_completeness".to_string(),
                serde_json::to_value(&vc)?,
            );
            obj.insert(
                "blocked_validations".to_string(),
                serde_json::to_value(&blocked)?,
            );
        }
        printer.print_json(&v);
        return Ok(());
    }

    // ---- Human-readable report ----
    println!("══ loom report ═════════════════════════════════════════════════════");
    println!();
    println!("{}", fmt_status(&status));
    println!();

    println!("── Top Intents by RELATES_TO Centrality ─────────────────────────────");
    println!("  (degree counts horizontal RELATES_TO links only — a root with many");
    println!("   HIERARCHY children can still show degree 0; that's expected.)");
    if top_intents.is_empty() {
        println!("  (none)");
    } else {
        for ic in &top_intents {
            println!(
                "  degree={deg:<4}  {name}  ({id})",
                deg = ic.degree,
                name = ic.intent.name,
                id = ic.intent.id,
            );
        }
    }
    println!();

    println!("── Edge Counts by Inspection Status ─────────────────────────────────");
    let mut sorted_statuses: Vec<_> = by_status.iter().collect();
    sorted_statuses.sort_by_key(|(status, _)| *status);
    for (s, count) in &sorted_statuses {
        println!("  {s:<30}  {count}");
    }
    println!();

    println!(
        "── Intents with No Validations ({}) ─────────────────────────────────",
        intents_no_val.len()
    );
    if intents_no_val.is_empty() {
        println!("  ✓ All intents have at least one validation.");
    } else {
        for i in &intents_no_val {
            println!("  [RISKY]  {}  ({})", i.name, i.id);
        }
        println!();
        println!(
            "  → Add validations with `loom validation add` and link via `loom edge validates`."
        );
    }
    println!();

    println!(
        "── Failing GOVERNS Edges ({}) ──────────────────────────────────────",
        failing_governs.len()
    );
    if failing_governs.is_empty() {
        println!("  ✓ No rule violations found.");
    } else {
        for g in &failing_governs {
            println!(
                "  [failing]  rule={rname}  intent={iname}  evidence={}",
                truncate_chars(&g.evidence, 80),
                rname = g.rule_name,
                iname = g.intent_name,
            );
        }
    }
    println!();

    if !blocked.is_empty() {
        println!(
            "── Blocked Validations ({}) ─────────────────────────────────────────",
            blocked.len()
        );
        for v in &blocked {
            println!("  ⊘ {}  ({})", v.name, v.id);
        }
        println!("  → Waiting on something external (the why is on each VALIDATES edge).");
        println!(
            "    Unblock with `loom validation mark <id> --result passed|failed --evidence …`."
        );
        println!();
    }

    println!(
        "── Recent Passing Edges ({}) ────────────────────────────────────────",
        recent.len()
    );
    if recent.is_empty() {
        println!("  (none)");
    } else {
        for e in &recent {
            println!(
                "  {}  {} → {}  criterion='{}'",
                e.id,
                e.from_name,
                e.to_name,
                truncate_chars(&e.criterion, 60),
            );
        }
    }
    println!();

    println!("── Vertical Completeness (the binding spine) ────────────────────────");
    println!(
        "  {}  tree: {} root(s) · {} leaf intent(s)  [{}]",
        if vc.complete { "✓" } else { "✗" },
        vc.roots,
        vc.leaves,
        if vc.multi_parent.is_empty() && !vc.cycle {
            "well-formed"
        } else {
            "MALFORMED — see `loom doctor`"
        },
    );
    if !vc.unrealized_leaves.is_empty() {
        println!("  ✗ unrealized leaf intents (implemented but no code grounds them):");
        for n in vc.unrealized_leaves.iter().take(40) {
            println!("      - {}", n);
        }
        if let Some(m) = crate::output::more_marker(
            vc.unrealized_leaves.len(),
            40,
            "`loom report --json` for the full list",
        ) {
            println!("      {m}");
        }
        println!("    → `loom edge implement <intent> <codefile>` or decompose with `loom edge hierarchy`.");
    }
    if !vc.unreached_codefiles.is_empty() {
        println!("  ✗ CodeFiles reached by no intent (code with no recorded purpose):");
        for p in vc.unreached_codefiles.iter().take(40) {
            println!("      - {}", p);
        }
        if let Some(m) = crate::output::more_marker(
            vc.unreached_codefiles.len(),
            40,
            "`loom report --json` for the full list",
        ) {
            println!("      {m}");
        }
        println!("    → ground them (`loom edge implement`) or drop the CodeFile.");
    }
    if !vc.non_system_roots.is_empty() {
        println!(
            "  ⚠ root intents not at `system` level (advisory — does not block completeness):"
        );
        for n in vc.non_system_roots.iter().take(40) {
            println!("      - {}", n);
        }
        if let Some(m) = crate::output::more_marker(
            vc.non_system_roots.len(),
            40,
            "`loom report --json` for the full list",
        ) {
            println!("      {m}");
        }
    }
    if vc.complete {
        println!("  ✓ Every implemented leaf is realized and every CodeFile is reached.");
    }
    println!();

    println!(
        "── Completeness Gaps ({}) ───────────────────────────────────────────",
        gaps.len()
    );
    if gaps.is_empty() {
        println!("  ✓ No gaps — grounded, validated, and error/fallback paths covered.");
    } else {
        for g in &gaps {
            println!("  • {}", g);
        }
    }

    Ok(())
}
fn truncate_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}
