//! `loom debt` — ranked statistical advisory clusters.
//!
//! Statistical signals (co-change, shotgun-surgery, code-clone, proof-locality)
//! are NEVER required debt and NEVER gate any maturity rung — they are a ranked
//! FEED, not an obligation queue. This command compresses the raw signal volume
//! into actionable clusters so an operator finds the few pressure points worth
//! acting on rather than drowning in raw instance counts.
//!
//! Confirming a cluster creates an asserted edge or refactor task; dismissing
//! one records a decision note. Both actions remove it from future output.

use anyhow::Result;

use crate::db::queries::{
    clone_suggestions, cochange_suggestions, proof_locality_suggestions,
    shotgun_surgery_suggestions, Smell,
};
use crate::db::{GraphReadHandle, GraphReadRepository};
use crate::output::Printer;

/// Maximum git log entries to read for co-change / shotgun computation.
const GIT_LOG_LIMIT: usize = 800;

pub fn run(kind: Option<&str>, limit: usize, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let store = GraphReadHandle::open(&cwd)?;
    run_with_db(&store, &cwd, kind, limit, printer)
}

pub fn run_with_db(
    db: &dyn GraphReadRepository,
    root: &std::path::Path,
    kind: Option<&str>,
    limit: usize,
    printer: &Printer,
) -> Result<()> {
    let snapshot = db.query_snapshot()?;
    let ignores = db.list_ignores()?;
    let decision_notes = db.notes_by_kind("decision")?;

    // Co-change and shotgun require a git pass; clone and proof-locality are
    // graph-only.
    let paths: std::collections::HashSet<String> =
        snapshot.codefiles.iter().map(|c| c.path.clone()).collect();
    let cc = crate::repo::git_cochange(root, &paths, GIT_LOG_LIMIT);

    let clone_patterns: Vec<glob::Pattern> = ignores
        .iter()
        .filter_map(|i| glob::Pattern::new(&i.pattern).ok())
        .collect();

    // Split each feed into open (un-adjudicated) and adjudicated counts.
    let split = |raw: Vec<Smell>| -> (Vec<Smell>, usize) {
        let (open, adj) = crate::commands::smells::split_advisories_for_adjudication(
            &snapshot,
            raw,
            &decision_notes,
        );
        (open, adj.len())
    };

    let (cochange, cochange_adj) =
        split(cochange_suggestions(&snapshot, &cc.pairs, &cc.individual));
    let (shotgun, shotgun_adj) = split(shotgun_surgery_suggestions(
        &snapshot,
        &cc.pairs,
        &cc.individual,
    ));
    let (clones, clones_adj) = split(clone_suggestions(&snapshot, &clone_patterns));
    let (proof_loc, proof_loc_adj) = split(proof_locality_suggestions(&snapshot));

    // Collect, filter by kind, sort by score descending.
    let mut all: Vec<(&str, &Smell)> = Vec::new();
    if kind.map_or(true, |k| k == "co-change" || k == "cochange") {
        for s in &cochange {
            all.push(("co-change", s));
        }
    }
    if kind.map_or(true, |k| k == "shotgun") {
        for s in &shotgun {
            all.push(("shotgun", s));
        }
    }
    if kind.map_or(true, |k| k == "clone") {
        for s in &clones {
            all.push(("clone", s));
        }
    }
    if kind.map_or(true, |k| k == "proof-locality") {
        for s in &proof_loc {
            all.push(("proof-locality", s));
        }
    }
    all.sort_by(|a, b| {
        b.1.score
            .partial_cmp(&a.1.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let shown: Vec<_> = all.iter().take(limit).collect();

    let raw_total = (cochange.len() + cochange_adj)
        + (shotgun.len() + shotgun_adj)
        + (clones.len() + clones_adj)
        + (proof_loc.len() + proof_loc_adj);
    let open_total = cochange.len() + shotgun.len() + clones.len() + proof_loc.len();
    let adj_total = cochange_adj + shotgun_adj + clones_adj + proof_loc_adj;

    if printer.json {
        let clusters: Vec<serde_json::Value> = shown
            .iter()
            .map(|(k, s)| {
                serde_json::json!({
                    "kind": k,
                    "score": s.score,
                    "summary": s.summary,
                    "evidence": s.evidence,
                    "remedy": s.remedy,
                    "intent_ids": s.intent_ids(),
                })
            })
            .collect();
        printer.print_json(&serde_json::json!({
            "note": "Statistical signals — never required debt, never gate any maturity rung.",
            "total_raw_signals": raw_total,
            "open_clusters": open_total,
            "adjudicated": adj_total,
            "shown": shown.len(),
            "kinds": {
                "co_change": { "open": cochange.len(), "adjudicated": cochange_adj },
                "shotgun": { "open": shotgun.len(), "adjudicated": shotgun_adj },
                "clone": { "open": clones.len(), "adjudicated": clones_adj },
                "proof_locality": { "open": proof_loc.len(), "adjudicated": proof_loc_adj },
            },
            "clusters": clusters,
        }));
        return Ok(());
    }

    // Human output.
    println!("Statistical advisory feed — never required debt, never gates any maturity rung.");
    println!("{raw_total} raw signals · {open_total} open clusters · {adj_total} adjudicated");
    println!(
        "  co-change {} · shotgun {} · clone {} · proof-locality {}",
        cochange.len(),
        shotgun.len(),
        clones.len(),
        proof_loc.len()
    );
    println!("Confirm: `loom edge explore <a> <b> ground …` or `loom hypothesis add …`");
    println!("Dismiss: `loom note add --intent <id> --kind decision --text \"<why incidental>\"`");
    println!();

    if shown.is_empty() {
        println!("No open statistical clusters.");
        return Ok(());
    }

    println!("Top {} clusters by score:", shown.len());
    for (i, (k, s)) in shown.iter().enumerate() {
        println!("{}. [{k}] score={:.1}  {}", i + 1, s.score, s.summary);
        println!("   evidence: {}", s.evidence);
        println!("   remedy:   {}", s.remedy);
        println!();
    }

    if all.len() > limit {
        println!(
            "… {} more — use --limit N to show more, or --kind <kind> to filter.",
            all.len() - limit
        );
    }

    Ok(())
}
