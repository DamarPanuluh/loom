use anyhow::Result;
use std::env;

use crate::db::{ensure_initialized, GrafeoDb};
use crate::db::queries::{get_intent, unresolved_edges_for_intent};
use crate::output::{fmt_edge_row, fmt_intent, Printer};

pub fn run(intent_id: &str, printer: &Printer) -> Result<()> {
    let cwd = env::current_dir()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;
    let intent_id = &crate::db::queries::resolve_intent(&db, intent_id)?;

    let intent = get_intent(&db, intent_id)?
        .ok_or_else(|| anyhow::anyhow!(
            "Intent '{}' not found.\nRun `loom intent list` to see available intents.",
            intent_id
        ))?;

    let edges = unresolved_edges_for_intent(&db, intent_id)?;

    if printer.json {
        printer.print_json(&serde_json::json!({
            "intent": intent,
            "unresolved_edges": edges,
        }));
    } else {
        println!("── Intent ──────────────────────────────────────────────────────────");
        println!("{}", fmt_intent(&intent));
        println!();
        println!("── Unresolved Edges ({}) ─────────────────────────────────────────────", edges.len());
        if edges.is_empty() {
            println!("  ✓ No unresolved edges touching this intent.");
        } else {
            for e in &edges {
                println!("{}", fmt_edge_row(e));
            }
        }
    }
    Ok(())
}
