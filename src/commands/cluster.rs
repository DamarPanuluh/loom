use anyhow::Result;

use crate::db::queries::{resolve_intent_from_snapshot, unresolved_edges_for_intent_from_snapshot};
use crate::db::{GraphReadHandle, GraphReadRepository};
use crate::output::{fmt_edge_row, fmt_intent, Printer};

pub fn run(intent_id: &str, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let store = GraphReadHandle::open(&cwd)?;
    run_with_db(&store, &cwd, intent_id, printer)
}

pub fn run_with_db(
    db: &dyn GraphReadRepository,
    _root: &std::path::Path,
    intent_id: &str,
    printer: &Printer,
) -> Result<()> {
    let snapshot = db.query_snapshot()?;
    let intent_id = &resolve_intent_from_snapshot(&snapshot, intent_id)?;

    let intent = db
        .get_intent(intent_id)?
        .ok_or_else(|| anyhow::anyhow!(crate::output::intent_not_found_list(intent_id)))?;

    let edges = unresolved_edges_for_intent_from_snapshot(&snapshot, intent_id);

    if printer.json {
        printer.print_json(&serde_json::json!({
            "intent": intent,
            "unresolved_edges": edges,
        }));
    } else {
        println!("── Intent ──────────────────────────────────────────────────────────");
        println!("{}", fmt_intent(&intent));
        println!();
        println!(
            "── Unresolved Edges ({}) ─────────────────────────────────────────────",
            edges.len()
        );
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
