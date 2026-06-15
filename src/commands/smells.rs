//! `loom smells` — make problems obvious, then hand over the methodical
//! remedy. Pure graph computation (see `db::queries::smells`); read-only, so
//! any role may run it. The findings route INTO the normal loop: every smell's
//! remedy is an existing loom command sequence — and OPEN findings gate green
//! (`graph_state` routes phase=audit until zero remain).
//!
//! Adjudicated findings are NOT hidden: a suppressed suspicion prints with
//! its ruling (who, when, why, and what re-opens it). "No findings" and
//! "five findings, all ruled deliberate" must never look alike — the second
//! is an audit surface a human may want to overrule.

use anyhow::Result;

use crate::db::queries::{
    cochange_suggestions, proof_locality_suggestions, AdjudicatedSmell, QuerySnapshot, Smell,
    SmellReport,
};
use crate::db::{GraphReadHandle, GraphReadRepository};
use crate::output::Printer;

pub fn run(limit: usize, summary: bool, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let store = GraphReadHandle::open(&cwd)?;
    run_with_db(&store, &cwd, limit, summary, printer)
}

pub fn run_with_db(
    db: &dyn GraphReadRepository,
    root: &std::path::Path,
    limit: usize,
    summary: bool,
    printer: &Printer,
) -> Result<()> {
    let snapshot = db.query_snapshot()?;
    let report = db.smell_report(&snapshot)?;
    let registry = db.vocab_term_count()?;
    render(root, &snapshot, report, registry, limit, summary, printer)
}
fn kind_counts(smells: &[Smell]) -> std::collections::BTreeMap<String, usize> {
    let mut counts = std::collections::BTreeMap::new();
    for smell in smells {
        *counts.entry(smell.kind.clone()).or_insert(0) += 1;
    }
    counts
}

fn adjudicated_kind_counts(
    smells: &[AdjudicatedSmell],
) -> std::collections::BTreeMap<String, usize> {
    let mut counts = std::collections::BTreeMap::new();
    for smell in smells {
        *counts.entry(smell.kind.clone()).or_insert(0) += 1;
    }
    counts
}

fn render(
    root: &std::path::Path,
    snapshot: &QuerySnapshot,
    report: SmellReport,
    registry: usize,
    limit: usize,
    summary: bool,
    printer: &Printer,
) -> Result<()> {
    // Advisory cochange_coupling suggestions: git-derived, command-only (the
    // audit gate's `compute_smells_from` stays git-free and fast), never gate
    // green. Bounded to recent history; degrades silently with no git.
    let paths: std::collections::HashSet<String> =
        snapshot.codefiles.iter().map(|c| c.path.clone()).collect();
    let cc = crate::repo::git_cochange(root, &paths, 800);
    let suggestions = cochange_suggestions(snapshot, &cc.pairs, &cc.individual);
    let suggestions_total = suggestions.len();
    let suggestions_shown: Vec<_> = suggestions.into_iter().take(limit.max(1)).collect();

    // Advisory proof-locality: STATIC (no git, no coverage run), never gates
    // green. Flags leaves the `proven` axis counts whose only `test` proof
    // resolves to other files than their grounded code.
    let proof_adv = proof_locality_suggestions(snapshot);
    let proof_total = proof_adv.len();
    let proof_shown: Vec<_> = proof_adv.into_iter().take(limit.max(1)).collect();

    let total = report.open.len();
    let (coded, tagged) = (report.coded_intents, report.tagged_coded_intents);
    let (coded_layers, declared_layers) = (report.coded_layers, report.declared_layers);
    let mut smells = report.open;
    let open_by_kind = kind_counts(&smells);
    smells.truncate(limit);
    let adjudicated = report.adjudicated;
    let adjudicated_by_kind = adjudicated_kind_counts(&adjudicated);

    if summary {
        let blind = coded.saturating_sub(tagged);
        if printer.json {
            printer.print_json(&serde_json::json!({
                "summary": true,
                "total": total,
                "shown": smells.len(),
                "open_by_kind": open_by_kind,
                "top": smells.iter().map(|s| serde_json::json!({
                    "kind": s.kind,
                    "summary": s.summary,
                    "remedy": s.remedy,
                })).collect::<Vec<_>>(),
                "adjudicated_total": adjudicated.len(),
                "adjudicated_by_kind": adjudicated_by_kind,
                "coded_intents": coded,
                "tagged_coded_intents": tagged,
                "untagged_coded_intents": blind,
                "vocab_terms": registry,
                "coded_layers": coded_layers,
                "declared_layers": declared_layers,
                "cochange_suggestions_total": suggestions_total,
                "proof_locality_suggestions_total": proof_total,
                "note": "Summary mode omits per-finding evidence, teaching, adjudication bodies, and advisory bodies. Use `loom smells --json` only when per-item evidence is needed.",
            }));
        } else {
            println!("── loom smells summary ──────────────────────────────────────────────");
            println!("  open findings: {total}");
            for (kind, count) in &open_by_kind {
                println!("    {kind}: {count}");
            }
            println!("  adjudicated findings: {}", adjudicated.len());
            println!("  co-change advisories: {suggestions_total}");
            println!("  proof-locality advisories: {proof_total}");
            println!("  tagged coded intents: {tagged}/{coded}");
            if blind > 0 {
                println!("  duplicate detector blind spot: {blind} untagged coded intent(s)");
            }
            if declared_layers == 0 && coded_layers >= 2 {
                println!(
                    "  layering detector unarmed: {coded_layers} coded layer(s), no declared order"
                );
            }
            for s in &smells {
                println!("  - [{}] {}", s.kind, s.summary);
                println!("    remedy: {}", s.remedy);
            }
            println!("  Full detail: `loom smells --json`.");
        }
        return Ok(());
    }
    if printer.json {
        printer.print_json(&serde_json::json!({
            "total": total,
            "shown": smells.len(),
            "smells": smells,
            "adjudicated_total": adjudicated.len(),
            "adjudicated": adjudicated,
            "coded_intents": coded,
            "tagged_coded_intents": tagged,
            "vocab_terms": registry,
            "coded_layers": coded_layers,
            "declared_layers": declared_layers,
            "cochange_suggestions": suggestions_shown,
            "cochange_suggestions_total": suggestions_total,
            "proof_locality_suggestions": proof_shown,
            "proof_locality_suggestions_total": proof_total,
            "note": "Findings are suspicions computed from graph structure — resolve or refute each via its remedy (an `independent` verdict / decision note is as valuable as a fix). OPEN findings gate green: phase=complete requires zero. `adjudicated` lists suppressed findings WITH their rulings — review them; each names what re-opens it. `cochange_suggestions` (git evolutionary coupling) and `proof_locality_suggestions` (a proven leaf whose only `test` proof lives in other files) are ADVISORY — they never gate green; explore or ignore.",
        }));
        return Ok(());
    }

    println!("── loom smells (derived from graph structure — suspicions, not verdicts) ──");
    println!();
    if smells.is_empty() {
        if adjudicated.is_empty() {
            println!("  ✓ No open findings: no twins, no overlapping ownership, no scatter, no");
            println!("    tangles, every rule considered against every coded intent. The audit");
            println!("    gate is green.");
        } else {
            println!("  ✓ No OPEN findings — the audit gate is green.");
        }
    }
    for s in &smells {
        println!("  [{}]  (score {:.1})", s.kind, s.score);
        println!("    {}", s.summary);
        println!("    evidence: {}", s.evidence);
        println!("    remedy:   {}", s.remedy);
        println!("    teaches:  {}", s.teaching.principle);
        println!("    inspect:  {}", s.teaching.inspect.join(" · "));
        println!("    avoid:    {}", s.teaching.avoid.join(" · "));
        println!("    done:     {}", s.teaching.done_when);
        println!();
    }
    if total > smells.len() {
        println!(
            "  ({} more — `loom smells --limit {}`)",
            total - smells.len(),
            total
        );
    }
    if !suggestions_shown.is_empty() {
        println!();
        println!(
            "── co-change suggestions ({}) — ADVISORY (git evolutionary coupling; never gate green) ──",
            suggestions_total
        );
        println!();
        for s in &suggestions_shown {
            println!("  [{}]  (score {:.1})", s.kind, s.score);
            println!("    {}", s.summary);
            println!("    evidence: {}", s.evidence);
            println!("    remedy:   {}", s.remedy);
            println!();
        }
        if suggestions_total > suggestions_shown.len() {
            println!(
                "  ({} more — `loom smells --limit {}`)",
                suggestions_total - suggestions_shown.len(),
                suggestions_total
            );
        }
    }
    if !proof_shown.is_empty() {
        println!();
        println!(
            "── proof-locality advisories ({}) — ADVISORY (proven leaf, test lives elsewhere; never gate green) ──",
            proof_total
        );
        println!();
        for s in &proof_shown {
            println!("  [{}]  (score {:.1})", s.kind, s.score);
            println!("    {}", s.summary);
            println!("    evidence: {}", s.evidence);
            println!("    remedy:   {}", s.remedy);
            println!();
        }
        if proof_total > proof_shown.len() {
            println!(
                "  ({} more — `loom smells --limit {}`)",
                proof_total - proof_shown.len(),
                proof_total
            );
        }
    }
    if !adjudicated.is_empty() {
        println!();
        println!(
            "── adjudicated ({}) — suppressed by recorded rulings, not by absence ──────",
            adjudicated.len()
        );
        println!();
        for a in &adjudicated {
            println!("  [{}]  {}", a.kind, a.summary);
            println!(
                "    ruling ({}, {}): {}",
                a.ruled_by,
                &a.ruled_at[..a.ruled_at.len().min(19)],
                a.ruling
            );
            println!("    re-opens when: {}", a.reopens_when);
            println!("    teaches: {}", a.teaching.principle);
            println!("    done:    {}", a.teaching.done_when);
            println!();
        }
        println!("  A ruling you disagree with is overruled through the work, not the ledger:");
        println!("  propose the change (`loom hypothesis add … --target <intent>`) — adoption");
        println!("  restructures the graph and the ruling's subject with it.");
    }
    // The instrument's own coverage, disclosed next to its readings:
    // duplicated_responsibility has a weak lexical fallback for untagged coded
    // pairs, but registered tags are still the high-signal detector.
    let blind = coded - tagged;
    if coded >= 2 && blind > 0 {
        println!();
        if registry == 0 {
            println!("  ⚠ duplicated_responsibility is unarmed: no vocabulary registered, and");
            println!(
                "    {blind} of {coded} coded intent(s) are untagged — only the weaker lexical"
            );
            println!("    fallback can catch same-responsibility pairs in unrelated code. Seed");
            println!(
                "    terms (`loom vocab add`), then tag (`loom intent tag add <intent> <term>`)."
            );
        } else {
            println!(
                "  ⚠ under-armed: {blind} of {coded} coded intent(s) carry no registered tag —"
            );
            println!("    duplicated_responsibility falls back to lexical similarity for those");
            println!("    pairs, but tag collisions are stronger. `loom vocab list` shows the");
            println!("    registry; tag with `loom intent tag add <intent> <term>`.");
        }
    }
    // Same doctrine for the layering instrument: layers in use but no
    // declared order means imports pointing up the architecture are invisible
    // — say so where the readings are.
    if declared_layers == 0 && coded_layers >= 2 {
        println!();
        println!(
            "  ⚠ layering_violation is unarmed: {coded_layers} layers in use across coded intents"
        );
        println!("    but no layer order declared — imports pointing up the architecture are");
        println!("    invisible. Declare it: `loom layer order <top> … <bottom>` (top layer");
        println!("    first; `loom layer list` shows usage; undeclared layers stay exempt).");
    }
    if !smells.is_empty() {
        println!();
        println!("  Resolve or refute each via its remedy — `independent`/a decision note is as");
        println!("  valuable as a fix. Open findings gate green: phase=complete requires zero.");
    }
    Ok(())
}
