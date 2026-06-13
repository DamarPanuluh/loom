//! `loom hotspots` — structural importance from graph centrality.
//!
//! This is STRUCTURAL hotness (what the graph says is central / high-blast-radius),
//! NOT runtime profiling. High-centrality intents are where understanding and
//! cleanup pay off most; tangled files carry the most concerns.

use anyhow::Result;

use crate::db::queries::{tangled_files, top_intents_by_centrality};
use crate::db::{ensure_initialized, GrafeoDb};
use crate::output::Printer;

pub fn run(limit: usize, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    let central = top_intents_by_centrality(&db, limit)?;
    let tangled = tangled_files(&db, limit)?;

    if printer.json {
        printer.print_json(&serde_json::json!({
            "kind": "structural",
            "note": "Centrality-based importance, not runtime profiling.",
            "central_intents": central.iter().map(|c| serde_json::json!({
                "id": c.intent.id, "name": c.intent.name,
                "level": c.intent.abstraction_level, "degree": c.degree,
            })).collect::<Vec<_>>(),
            "tangled_files": tangled.iter().map(|(p, c)| serde_json::json!({
                "path": p, "intents": c,
            })).collect::<Vec<_>>(),
        }));
        return Ok(());
    }

    println!("── loom hotspots (structural — graph centrality, not runtime) ────────");
    println!();
    println!("Most central intents (highest blast radius — start here):");
    if central.iter().all(|c| c.degree == 0) {
        println!("  (no RELATES_TO edges yet — centrality is 0 across the board)");
    } else {
        for c in &central {
            println!(
                "  degree {:>3}  [{}]  {}  ({})",
                c.degree, c.intent.abstraction_level, c.intent.name, c.intent.id
            );
        }
    }
    println!();
    println!("Most tangled files (most intents implemented in one file):");
    if tangled.is_empty() {
        println!("  (no IMPLEMENTS edges yet)");
    } else {
        for (path, n) in &tangled {
            println!("  {:>3} intents  {}", n, path);
        }
    }
    Ok(())
}
