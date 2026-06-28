//! `loom paths` — composition-proof coverage: the JOURNEY corner of the
//! intent/code/saga triangle. Which active intents are proven by a COMPOSITION
//! proof (one that exercises an assembly) versus only by a leaf/unit proof,
//! versus unproven.
//!
//! ADDITIVE + READ-ONLY: it informs, it never gates green. The composition tier
//! is recognised from the GRAPH's own topology — a declared journey
//! (`validation_type == saga`), a proof that spans >= 2 intents, or a proof of a
//! non-leaf/assembly intent — NEVER from test-runner command strings, so it reads
//! the same on any repo or language. `loom paths` discloses the per-signal
//! breakdown so the inference is auditable. The `leaf-only` list is the surface to
//! JUDGE: is a real journey missing here, or is this a genuine leaf (a terminal
//! computation a unit test fully proves)?

use anyhow::Result;

use crate::db::queries::composition_coverage_from_snapshot;
use crate::db::{GraphReadHandle, GraphReadRepository};
use crate::output::Printer;

pub fn run(limit: usize, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let store = GraphReadHandle::open(&cwd)?;
    run_with_db(&store, limit, printer)
}

pub fn run_with_db(db: &dyn GraphReadRepository, limit: usize, printer: &Printer) -> Result<()> {
    let snapshot = db.query_snapshot()?;
    let cov = composition_coverage_from_snapshot(&snapshot);

    if printer.json {
        printer.print_json(&serde_json::json!({
            "kind": "composition_coverage",
            "note": "Path-proof tier recognised from graph topology (declared saga / spans >=2 intents / proves a non-leaf assembly), never from command strings — same on any repo. Read-only; never gates.",
            "total": cov.total,
            "path_proven": cov.path_proven,
            "leaf_only": cov.leaf_only,
            "unproven": cov.unproven,
            "composition_proofs_by_signal": {
                "declared_journey": cov.proofs_declared_journey,
                "multi_intent_span": cov.proofs_multi_intent,
            },
            "leaf_only_intents": cov.leaf_only_intents.iter()
                .map(|(id, n)| serde_json::json!({"id": id, "name": n})).collect::<Vec<_>>(),
            "unproven_intents": cov.unproven_intents.iter()
                .map(|(id, n)| serde_json::json!({"id": id, "name": n})).collect::<Vec<_>>(),
        }));
        return Ok(());
    }

    let pct = |n: i64| {
        if cov.total > 0 {
            100 * n / cov.total
        } else {
            0
        }
    };
    println!("── loom paths (composition-proof coverage — the journey corner) ──────");
    println!();
    println!("Of {} active intents:", cov.total);
    println!(
        "  path-proven (a journey / assembly-spanning proof)     : {:>4}  ({}%)",
        cov.path_proven,
        pct(cov.path_proven)
    );
    println!(
        "  leaf-only   (pieces proven, no journey covers them)   : {:>4}  ({}%)",
        cov.leaf_only,
        pct(cov.leaf_only)
    );
    println!(
        "  unproven    (no passing proof at all)                 : {:>4}  ({}%)",
        cov.unproven,
        pct(cov.unproven)
    );
    println!();

    if !cov.unproven_intents.is_empty() {
        println!("UNPROVEN (no passing proof — start here):");
        for (id, n) in cov.unproven_intents.iter().take(limit) {
            println!("  · {n}  ({id})");
        }
        if cov.unproven_intents.len() > limit {
            println!("  … and {} more", cov.unproven_intents.len() - limit);
        }
        println!();
    }

    println!(
        "LEAF-ONLY ({} — judge: is a real journey missing here, or a genuine leaf?):",
        cov.leaf_only
    );
    for (id, n) in cov.leaf_only_intents.iter().take(limit) {
        println!("  · {n}  ({id})");
    }
    if cov.leaf_only_intents.len() > limit {
        println!("  … and {} more", cov.leaf_only_intents.len() - limit);
    }
    println!();
    println!(
        "Composition proofs recognised by signal (graph topology, NOT command strings):\n\
        \x20 declared journey (validation_type=saga) : {}\n\
        \x20 spans >=2 intents                       : {}",
        cov.proofs_declared_journey, cov.proofs_multi_intent
    );
    println!(
        "ADDITIVE & read-only — never gates green. The proof tier reads the same on any repo or\n\
         language: a proof that spans >=2 intents or is a declared saga is a journey; a proof\n\
         attributed to ONE intent (even a parent) is a leaf, since validating a parent alone\n\
         does not prove its children compose. So a leaf-only intent here is one whose only\n\
         proof is attributed to it alone — judge whether a real journey is missing, or it is\n\
         a genuine terminal leaf."
    );
    Ok(())
}
