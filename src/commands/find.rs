//! `loom find` — ask the map: BM25 keyword search over intent names and
//! descriptions, each hit returned with its tree position, groundings, and
//! freshness so the answer can be acted on without a second lookup.

use anyhow::Result;

use crate::db::queries::find_intents;
use crate::db::{ensure_initialized, GrafeoDb};
use crate::output::Printer;

pub fn run(query: &str, limit: usize, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    let (hits, match_total) = find_intents(&db, query, limit)?;

    if printer.json {
        printer.print_json(&serde_json::json!({
            "query": query,
            // total = pre-truncation matches; truncated distinguishes
            // "5 of 5" from "5 of 40" (list-envelope convention).
            "total": match_total,
            "truncated": match_total > hits.len(),
            "hits": hits.iter().map(|h| serde_json::json!({
                "id": h.intent.id,
                "name": h.intent.name,
                "description": h.intent.description,
                "level": h.intent.abstraction_level,
                "domain": h.intent.domain,
                "layer": h.intent.layer,
                "lifecycle": h.intent.lifecycle,
                "status": h.intent.status,
                "score": h.score,
                "parent_chain": h.parent_chain,
                "groundings": h.groundings.iter().map(|(path, locator)| serde_json::json!({
                    "path": path, "locator": locator,
                })).collect::<Vec<_>>(),
                "stale_edges": h.stale_edges,
            })).collect::<Vec<_>>(),
            "note": if hits.is_empty() {
                "No match. Either this isn't mapped yet (`loom coverage` shows unaccounted files) or the map uses different words — reformulate and retry."
            } else { "" },
        }));
        return Ok(());
    }

    println!("── loom find \"{query}\" ──────────────────────────────────────");
    if hits.is_empty() {
        println!();
        println!("  No intents match.");
        println!("  Either this isn't mapped yet (`loom coverage` shows unaccounted files)");
        println!("  or the map uses different words — try synonyms.");
        return Ok(());
    }
    for h in &hits {
        println!();
        println!(
            "  {:>5.2}  [{}] {}  ({})",
            h.score, h.intent.abstraction_level, h.intent.name, h.intent.id
        );
        if !h.parent_chain.is_empty() {
            println!("         under: {}", h.parent_chain.join(" › "));
        }
        if !(h.intent.domain.is_empty() || h.intent.domain == "unknown")
            || !h.intent.layer.is_empty()
        {
            let domain = if h.intent.domain.is_empty() || h.intent.domain == "unknown" {
                "-".to_string()
            } else {
                h.intent.domain.clone()
            };
            let layer = if h.intent.layer.is_empty() {
                "-".to_string()
            } else {
                h.intent.layer.clone()
            };
            println!("         domain: {domain}   layer: {layer}");
        }
        println!("         {}", h.intent.description);
        if h.groundings.is_empty() {
            println!("         code: (not grounded — `loom intent show` for source_refs)");
        } else {
            let rendered: Vec<String> = h
                .groundings
                .iter()
                .map(|(path, locator)| {
                    if locator.is_empty() {
                        path.clone()
                    } else {
                        format!("{path} ({locator})")
                    }
                })
                .collect();
            println!("         code: {}", rendered.join(" · "));
        }
        if h.stale_edges > 0 {
            println!(
                "         ⚠ {} stale edge(s) — code changed since these claims were verified",
                h.stale_edges
            );
        }
    }
    if let Some(m) = crate::output::more_marker(
        match_total,
        hits.len(),
        &format!("`loom find \"{query}\" --limit 0` for all matches"),
    ) {
        println!();
        println!("  {m}");
    }
    Ok(())
}
