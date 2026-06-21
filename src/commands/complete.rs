//! `loom complete` — the COMPREHENSIVENESS projection: does the intent graph
//! capture everything the code should do? A pure read-only view (re-derives
//! nothing about "done") over the five canonical rubric dimensions + the existing
//! `fully_proven` badge. `--teach` serves the canonical rubric the LLM
//! instantiates per repo. The crux it surfaces everywhere: RECORD ≠ DISCHARGE.

use anyhow::Result;

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

    // The five dimensions.
    let symbol_report =
        crate::db::queries::symbol_accountability::symbol_accountability_from_parts_with_notes(
            &snapshot.codefiles,
            &snapshot.intents,
            &snapshot.implements,
            &decision_notes,
        );
    let entrypoint = comp::entrypoint_coverage(&symbol_report);
    let (boundary, boundary_owed) = comp::boundary_scan_from_disk(&root, &snapshot);
    let invariant = gs.coverage.measured_pairs.clone();
    let journey = comp::journey_ledger_from_snapshot(&snapshot);
    let behavioral = comp::behavioral_ledger_from_snapshot(&snapshot);

    // The quality badge (same one `loom status` shows).
    let open_smells = if matches!(gs.phase.as_str(), "audit" | "complete") {
        store.smell_report(&snapshot)?.open
    } else {
        Vec::new()
    };
    let inbox = store.list_inbox_items(None, None)?;
    let inbox_untriaged = inbox.iter().filter(|i| i.status == "new").count();
    let (mut fp_ok, mut fp_reasons) = crate::db::queries::stats::fully_proven_from_state(
        &gs,
        &snapshot,
        &open_smells,
        &entrypoint,
        inbox_untriaged,
    );
    if store.committed_export_stale(&root)? == Some(true) {
        fp_ok = false;
        fp_reasons.push("committed loom.graph.json is STALE — `loom export`".to_string());
    }

    // THE HONEST MIRROR (the meta-cognitive trigger). loom is DUMB — it measures
    // your seed, not the full vision; only YOU can tell if your model captures the
    // whole system or a sketch. So it shows the RAW depth loudly: how much of the
    // actual code surface a human/LLM intent directly owns. A low % next to a green
    // badge is the tell that the SEED was thin — reflect before declaring done.
    let total_sym = symbol_report.summary.total_symbols;
    let grounded_sym = symbol_report.summary.grounded;
    let modeled_pct = (grounded_sym * 100).checked_div(total_sym).unwrap_or(0);
    // Cognitive dimensions that read "—" are UNDESIGNED, not satisfied: a code
    // graph with zero user_visible leaves / zero happy aspects means the agent
    // hasn't done the journey/behavioral modeling — a gap to reflect on, not a ✓.
    let undesigned: Vec<&str> = [
        (journey.enumerated == 0).then_some("journey (no user_visible leaf classified)"),
        (behavioral.enumerated == 0).then_some("behavioral (no happy leaf classified)"),
    ]
    .into_iter()
    .flatten()
    .collect();
    // Docs-as-realization: intents marked implemented but grounded ONLY to docs.
    // A spec is a contract, not a built system — push the LLM to BUILD the code.
    let doc_realizations = comp::doc_only_realizations(&snapshot);

    if printer.json {
        printer.print_json(&serde_json::json!({
            "comprehensiveness": {
                "entrypoint": axis_json(&entrypoint),
                "boundary": axis_json(&boundary),
                "invariant": axis_json(&invariant),
                "journey": ledger_json(&journey),
                "behavioral": ledger_json(&behavioral),
            },
            "modeled_depth": {
                "symbols_directly_owned": grounded_sym,
                "symbols_total": total_sym,
                "percent": modeled_pct,
            },
            "undesigned_dimensions": undesigned,
            "doc_only_realizations": doc_realizations,
            "inbox_untriaged": inbox_untriaged,
            "boundary_owed_files": boundary_owed,
            "fully_proven": fp_ok,
            "fully_proven_reasons": fp_reasons,
            "self_check": "loom measures your SEED, not the full vision. If you are not certain you modeled every responsibility, you have not — the badge cannot see what you never seeded.",
            "next_step": next_step(&entrypoint, &boundary, &invariant, &journey, &behavioral, fp_ok),
        }));
    } else {
        let name = root.file_name().and_then(|n| n.to_str()).unwrap_or("graph");
        println!("── loom complete: {name} ─────────────────────────────────────────────");
        println!("  COMPREHENSIVENESS — does the graph capture everything the code should do?");
        println!(
            "    entrypoint   {}   public symbols owned (mechanical, gates fully_proven)",
            axis_str(&entrypoint)
        );
        println!(
            "    boundary     {}   files with external deps have a boundary intent",
            axis_str(&boundary)
        );
        println!(
            "    invariant    {}   coded intents measured under a GOVERNS rule",
            axis_str(&invariant)
        );
        println!(
            "    journey      {}   user_visible leaves with a passing saga (RECORD≠DISCHARGE)",
            ledger_str(&journey)
        );
        println!(
            "    behavioral   {}   happy leaves with a sad/edge sibling (RECORD≠DISCHARGE)",
            ledger_str(&behavioral)
        );
        for owed in [&journey.owed, &behavioral.owed, &boundary_owed] {
            if !owed.is_empty() {
                let shown: Vec<&str> = owed.iter().take(5).map(|s| s.as_str()).collect();
                let more = owed.len().saturating_sub(5);
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
        println!();
        println!("  QUALITY — is what's in the graph proven?");
        if fp_ok {
            println!("    ✓ PRODUCTION READY (fully_proven) — proven AND comprehensive.");
        } else {
            println!("    fully_proven: NOT YET");
            for r in &fp_reasons {
                println!("      · {r}");
            }
        }
        println!();
        // THE META-COGNITIVE TRIGGER. loom is DUMB — it certifies your SEED, not
        // the full vision it can't see. It does NOT pretend to judge completeness
        // (a coarse file-level grounding is legitimate; a symbol-precision % would
        // false-alarm on a well-modeled graph). It surfaces the gaps it CAN see and
        // makes YOU reflect on the ones it can't.
        println!("  ⟲ SELF-CHECK — loom certifies your SEED, not the system you never modeled.");
        if inbox_untriaged > 0 {
            println!(
                "    INBOX: {inbox_untriaged} un-triaged item(s) — pieces of the repo enumerated but NOT yet"
            );
            println!(
                "      decomposed into intents. Not complete until processed — `loom inbox triage`."
            );
        }
        if !doc_realizations.is_empty() {
            let shown: Vec<&str> = doc_realizations
                .iter()
                .take(4)
                .map(|s| s.as_str())
                .collect();
            println!(
                "    DOC-GROUNDED: {} intent(s) marked 'implemented' are grounded ONLY to docs ({}{}).",
                doc_realizations.len(),
                shown.join(", "),
                if doc_realizations.len() > 4 { ", …" } else { "" }
            );
            println!(
                "      A doc is a CONTRACT, not a built system. CONFIRM each: is the document itself the"
            );
            println!(
                "      deliverable? If it SPECIFIES code that should exist, you certified a spec as done —"
            );
            println!("      BUILD the real code + reground, or set `--lifecycle planned`.");
        }
        if !undesigned.is_empty() {
            println!(
                "    Not yet designed (reads '—'): {}.",
                undesigned.join("; ")
            );
        }
        println!(
            "    Confirm you seeded EVERY responsibility — loom cannot flag what was never modeled."
        );
        println!();
        println!(
            "  → {}",
            next_step(
                &entrypoint,
                &boundary,
                &invariant,
                &journey,
                &behavioral,
                fp_ok
            )
        );
    }
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

fn next_step(
    entrypoint: &CoverageAxis,
    boundary: &CoverageAxis,
    invariant: &CoverageAxis,
    journey: &comp::Ledger,
    behavioral: &comp::Ledger,
    fp_ok: bool,
) -> String {
    if entrypoint.covered < entrypoint.total {
        return "Map the unowned public symbols — `loom coverage` then `loom edge implement`."
            .to_string();
    }
    if boundary.covered < boundary.total {
        return "Declare a boundary on the intents owning external-dependency files — `loom intent update <id> --boundary outbound`.".to_string();
    }
    if !behavioral.owed.is_empty() {
        return "Design the failure paths — for each happy leaf add a sad/edge sibling (`loom intent add --aspect sad …`), then ground + prove it.".to_string();
    }
    if !journey.owed.is_empty() {
        return "Prove the user journeys — `loom saga add <spec>` then `loom saga run` for each user_visible leaf.".to_string();
    }
    if invariant.covered < invariant.total {
        return "Measure the remaining intents — `loom next --mode quality`.".to_string();
    }
    if !fp_ok {
        return "Comprehensive — now finish the QUALITY half: `loom status` for the fully_proven gaps.".to_string();
    }
    "Comprehensive AND proven. `loom guide` for the rubric; `loom complete --teach` to teach it."
        .to_string()
}
