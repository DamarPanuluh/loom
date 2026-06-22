//! `loom complete` — the MATURITY-LADDER view with comprehensiveness detail.
//!
//! Renders loom's single ordinal "done" (the rung-vector + focus) and, beneath
//! it, the five comprehensiveness dimensions that feed the rungs
//! (Seeded/Proven/Hardened). The crux it surfaces everywhere: RECORD ≠ DISCHARGE.
//! `--teach` serves the canonical rubric the LLM instantiates per repo. The old
//! standalone `fully_proven` QUALITY block is gone — folded into the ladder's
//! Production-ready rung.

use anyhow::Result;

use crate::db::queries::build_ladder;
use crate::db::queries::comprehensiveness as comp;
use crate::db::queries::stats::CoverageAxis;
use crate::db::{GraphReadHandle, GraphReadRepository};
use crate::output::Printer;

pub fn run(teach: bool, printer: &Printer) -> Result<()> {
    if teach {
        if printer.json {
            printer.print_json(&serde_json::json!({ "rubric": comp::rubric_teaching() }));
        } else {
            println!("{}", comp::rubric_teaching());
        }
        return Ok(());
    }

    let root = crate::db::resolve_root()?;
    let store = GraphReadHandle::open(&root)?;
    let snapshot = store.query_snapshot()?;
    let gs = store.graph_state(&snapshot)?;
    let decision_notes = store.notes_by_kind("decision")?;

    // Open smells are meaningful only at the audit gate (same rule as status).
    let open_smells = if matches!(gs.phase.as_str(), "audit" | "complete") {
        store.smell_report(&snapshot)?.open
    } else {
        Vec::new()
    };
    let inbox = store.list_inbox_items(None, None)?;
    let inbox_untriaged = inbox.iter().filter(|i| i.status == "new").count();
    let export_stale = store.committed_export_stale(&root)? == Some(true);

    let b = build_ladder(
        &root,
        &snapshot,
        &gs,
        &decision_notes,
        &inbox,
        &open_smells,
        inbox_untriaged,
        export_stale,
    );
    let invariant = gs.coverage.measured_pairs.clone();

    // Cognitive dimensions reading "—" are UNDESIGNED, not satisfied: a graph
    // with zero user_visible leaves / zero happy aspects means that modeling
    // hasn't been done — a gap to reflect on, not a ✓.
    let undesigned: Vec<&str> = [
        (b.journey.enumerated == 0).then_some("journey (no user_visible leaf classified)"),
        (b.behavioral.enumerated == 0).then_some("behavioral (no happy leaf classified)"),
    ]
    .into_iter()
    .flatten()
    .collect();

    if printer.json {
        printer.print_json(&serde_json::json!({
            "maturity": serde_json::to_value(&b.ladder)?,
            "comprehensiveness": {
                "entrypoint": axis_json(&b.entrypoint),
                "boundary": axis_json(&b.boundary),
                "invariant": axis_json(&invariant),
                "journey": ledger_json(&b.journey),
                "behavioral": ledger_json(&b.behavioral),
            },
            "modeled_depth": {
                "symbols_directly_owned": b.grounded_symbols,
                "symbols_total": b.total_symbols,
                "percent": b.modeled_pct,
            },
            "undesigned_dimensions": undesigned,
            "doc_only_realizations": b.doc_only,
            "source_corpus": b.source_corpus,
            "inbox_untriaged": inbox_untriaged,
            "boundary_owed_files": b.boundary_owed,
            "self_check": "loom measures your SEED, not the full vision. If you are not certain you modeled every responsibility, you have not — the ladder cannot see what you never seeded.",
        }));
        return Ok(());
    }

    let name = root.file_name().and_then(|n| n.to_str()).unwrap_or("graph");
    println!("── loom complete: {name} ─────────────────────────────────────────────");

    // The ladder — loom's single ordinal "done" (a rung-vector + focus).
    println!("  MATURITY LADDER — the single ordinal \"done\" (focus = the lowest unmet rung)");
    println!("    {}", b.ladder.vector_line());
    println!("    → {}", b.ladder.focus_summary());
    println!();

    // The comprehensiveness dimensions that feed the rungs (RECORD ≠ DISCHARGE).
    println!("  COMPREHENSIVENESS DETAIL — does the graph capture everything the code should do?");
    println!(
        "    entrypoint   {}   public symbols owned (feeds Seeded/Production-ready)",
        axis_str(&b.entrypoint)
    );
    println!(
        "    boundary     {}   files with external deps have a boundary intent",
        axis_str(&b.boundary)
    );
    println!(
        "    invariant    {}   coded intents measured under a GOVERNS rule (feeds Hardened)",
        axis_str(&invariant)
    );
    println!(
        "    journey      {}   user_visible leaves with a passing boundary proof (feeds Proven)",
        ledger_str(&b.journey)
    );
    println!(
        "    behavioral   {}   happy leaves with a sad/edge sibling (feeds Hardened)",
        ledger_str(&b.behavioral)
    );
    let cognitive_names = |leaves: &[comp::OwedLeaf]| -> Vec<String> {
        leaves.iter().map(|o| o.name.clone()).collect()
    };
    for names in [
        cognitive_names(&b.journey.owed),
        cognitive_names(&b.behavioral.owed),
        b.boundary_owed.clone(),
    ] {
        if !names.is_empty() {
            let shown: Vec<&str> = names.iter().take(5).map(|s| s.as_str()).collect();
            let more = names.len().saturating_sub(5);
            println!(
                "      owed: {}{}",
                shown.join(", "),
                if more > 0 {
                    format!(", +{more}")
                } else {
                    String::new()
                }
            );
        }
    }
    if !b.behavioral.owed.is_empty() {
        println!(
            "    → author each missing failure path (it then enters `loom next --mode build`):"
        );
        for o in b.behavioral.owed.iter().take(5) {
            println!(
                "      loom intent add --name \"{} — sad path\" --aspect sad --lifecycle planned --parent {} --description \"<what should happen when {} fails>\"",
                o.name, o.parent_id, o.name
            );
        }
        if b.behavioral.owed.len() > 5 {
            println!(
                "      (+{} more — `loom complete --json` lists every owed leaf with its parent id)",
                b.behavioral.owed.len() - 5
            );
        }
    }
    println!();

    // The meta-cognitive trigger: loom certifies your SEED, not the full vision.
    println!("  ⟲ SELF-CHECK — loom certifies your SEED, not the system you never modeled.");
    if inbox_untriaged > 0 {
        println!(
            "    INBOX: {inbox_untriaged} un-triaged item(s) — pieces of the repo enumerated but NOT yet"
        );
        println!(
            "      decomposed into intents. Not complete until processed — `loom inbox triage`."
        );
    }
    if !b.doc_only.is_empty() {
        let shown: Vec<&str> = b.doc_only.iter().take(4).map(|s| s.as_str()).collect();
        println!(
            "    DOC-GROUNDED: {} intent(s) marked 'implemented' are grounded ONLY to docs ({}{}).",
            b.doc_only.len(),
            shown.join(", "),
            if b.doc_only.len() > 4 { ", …" } else { "" }
        );
        println!("      A doc is a CONTRACT, not a built system. CONFIRM each: is the document itself the");
        println!("      deliverable? If it SPECIFIES code that should exist, you certified a spec as done —");
        println!("      BUILD the real code + reground, or set `--lifecycle planned`.");
    }
    if b.source_corpus.has_signal() {
        if !b.source_corpus.warning.is_empty() {
            println!("    SOURCE CORPUS: {}", b.source_corpus.warning);
        }
        if b.source_corpus.ids_total > 0 {
            println!(
                "      structured IDs: {} total · {} modeled · {} resolved · {} unresolved.",
                b.source_corpus.ids_total,
                b.source_corpus.modeled,
                b.source_corpus.resolved,
                b.source_corpus.unresolved
            );
        }
    }
    if !undesigned.is_empty() {
        println!(
            "    Not yet designed (reads '—'): {}.",
            undesigned.join("; ")
        );
    }
    println!(
        "    modeled depth: {}% ({}/{} symbols directly owned) — responsibilities, not every symbol.",
        b.modeled_pct, b.grounded_symbols, b.total_symbols
    );
    println!(
        "    Confirm you seeded EVERY responsibility — loom cannot flag what was never modeled."
    );

    Ok(())
}

fn axis_json(a: &CoverageAxis) -> serde_json::Value {
    serde_json::json!({ "covered": a.covered, "total": a.total })
}

fn ledger_json(l: &comp::Ledger) -> serde_json::Value {
    serde_json::json!({ "enumerated": l.enumerated, "discharged": l.discharged, "owed": l.owed })
}

fn axis_str(a: &CoverageAxis) -> String {
    if a.total == 0 {
        "    —    ".to_string()
    } else if a.covered >= a.total {
        format!("{:>3}/{:<3} ✓", a.covered, a.total)
    } else {
        format!("{:>3}/{:<3}  ", a.covered, a.total)
    }
}

fn ledger_str(l: &comp::Ledger) -> String {
    if l.enumerated == 0 {
        "    —    ".to_string()
    } else if l.discharged >= l.enumerated {
        format!("{:>3}/{:<3} ✓", l.discharged, l.enumerated)
    } else {
        format!("{:>3}/{:<3}  ", l.discharged, l.enumerated)
    }
}
