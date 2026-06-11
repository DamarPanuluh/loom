//! `loom smells` — make problems obvious, then hand over the methodical
//! remedy. Pure graph computation (see `db::queries::smells`); read-only, so
//! any role may run it. The findings route INTO the normal loop: every smell's
//! remedy is an existing loom command sequence — and OPEN findings gate green
//! (`graph_state` routes phase=audit until zero remain).

use anyhow::Result;

use crate::db::queries::compute_smells;
use crate::db::{ensure_initialized, GrafeoDb};
use crate::output::Printer;

pub fn run(limit: usize, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    let mut smells = compute_smells(&db)?;
    let total = smells.len();
    smells.truncate(limit);

    if printer.json {
        printer.print_json(&serde_json::json!({
            "total": total,
            "shown": smells.len(),
            "smells": smells,
            "note": "Findings are suspicions computed from graph structure — resolve or refute each via its remedy (an `independent` verdict / decision note is as valuable as a fix). OPEN findings gate green: phase=complete requires zero.",
        }));
        return Ok(());
    }

    println!("── loom smells (derived from graph structure — suspicions, not verdicts) ──");
    println!();
    if smells.is_empty() {
        println!("  ✓ No open findings: no twins, no overlapping ownership, no scatter, no");
        println!("    tangles, every rule considered against every coded intent — and every");
        println!("    adjudicated suspicion is on record. The audit gate is green.");
        return Ok(());
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
    println!("  Resolve or refute each via its remedy — `independent`/a decision note is as");
    println!("  valuable as a fix. Open findings gate green: phase=complete requires zero.");
    Ok(())
}
