//! `loom smells` — make problems obvious, then hand over the methodical
//! remedy. Pure graph computation (see `db::queries::smells`); read-only, so
//! any role may run it. The findings route INTO the normal loop: every smell's
//! remedy is an existing loom command sequence.

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
            "note": "Findings are suspicions computed from graph structure — refute or confirm each via its remedy; an `independent` verdict is as valuable as a fix.",
        }));
        return Ok(());
    }

    println!("── loom smells (derived from graph structure — suspicions, not verdicts) ──");
    println!();
    if smells.is_empty() {
        println!("  ✓ No structural smells: no twins, no overlapping ownership, no scatter,");
        println!("    no tangles, and every rule has been considered against every coded intent.");
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
    println!("  Refute or confirm each via its remedy — `independent` is as valuable as a fix.");
    Ok(())
}
