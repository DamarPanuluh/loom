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

use crate::db::queries::compute_smells;
use crate::db::{ensure_initialized, GrafeoDb};
use crate::output::Printer;

pub fn run(limit: usize, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    let report = compute_smells(&db)?;
    let total = report.open.len();
    let (coded, tagged) = (report.coded_intents, report.tagged_coded_intents);
    let registry = crate::db::queries::list_vocab_terms(&db)?.len();
    let mut smells = report.open;
    smells.truncate(limit);
    let adjudicated = report.adjudicated;

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
            "note": "Findings are suspicions computed from graph structure — resolve or refute each via its remedy (an `independent` verdict / decision note is as valuable as a fix). OPEN findings gate green: phase=complete requires zero. `adjudicated` lists suppressed findings WITH their rulings — review them; each names what re-opens it.",
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
        println!();
    }
    if total > smells.len() {
        println!("  ({} more — `loom smells --limit {}`)", total - smells.len(), total);
    }
    if !adjudicated.is_empty() {
        println!();
        println!("── adjudicated ({}) — suppressed by recorded rulings, not by absence ──────", adjudicated.len());
        println!();
        for a in &adjudicated {
            println!("  [{}]  {}", a.kind, a.summary);
            println!("    ruling ({}, {}): {}", a.ruled_by, &a.ruled_at[..a.ruled_at.len().min(19)], a.ruling);
            println!("    re-opens when: {}", a.reopens_when);
            println!();
        }
        println!("  A ruling you disagree with is overruled through the work, not the ledger:");
        println!("  propose the change (`loom hypothesis add … --target <intent>`) — adoption");
        println!("  restructures the graph and the ruling's subject with it.");
    }
    // The instrument's own blind spot, disclosed next to its readings:
    // duplicated_responsibility collides on registered tags only, so a quiet
    // report with untagged coded intents is NOT evidence of no duplication.
    let blind = coded - tagged;
    if coded >= 2 && blind > 0 {
        println!();
        if registry == 0 {
            println!("  ⚠ duplicated_responsibility is unarmed: no vocabulary registered, and");
            println!("    {blind} of {coded} coded intent(s) are untagged — same-responsibility pairs in");
            println!("    unrelated code are invisible to every detector here. Seed terms");
            println!("    (`loom vocab add`), then tag (`loom intent tag add <intent> <term>`).");
        } else {
            println!("  ⚠ blind spot: {blind} of {coded} coded intent(s) carry no registered tag —");
            println!("    duplicated_responsibility cannot see pairs involving them (tags are");
            println!("    positive evidence only). `loom vocab list` shows the registry; tag with");
            println!("    `loom intent tag add <intent> <term>`.");
        }
    }
    if !smells.is_empty() {
        println!();
        println!("  Resolve or refute each via its remedy — `independent`/a decision note is as");
        println!("  valuable as a fix. Open findings gate green: phase=complete requires zero.");
    }
    Ok(())
}
