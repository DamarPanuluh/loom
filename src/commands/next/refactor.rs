use super::scoring::dispatch_line_for_lane;
use super::*;

// ---------------------------------------------------------------------------
// Refactor mode: the excellence-debt lane, loom-native. Serves the real-code
// findings — size (`oversized_file`, `large_behavioral_symbol`), metadata debt,
// proof-locality drift, and open `code_clone` groups — that do not necessarily
// block Production-ready but DO block the stricter Excellent certificate. Each
// item carries the loom-native
// three-way remedy (fix now / defer as tracked debt via `loom hypothesis add` /
// rule deliberate via `loom note add --smell`). Accepted/deferred real debt keeps
// Excellent yellow; only fixed, false-positive, or deliberate-design rulings clear
// the best-codebase claim.
//
// STATIC by construction (snapshot-only): the git-derived advisories
// (`cochange_coupling`, `shotgun_surgery`) stay in `loom smells` so this lane
// keeps `loom next` git-free, fast, and deterministic.
// ---------------------------------------------------------------------------

const REFACTOR_EMPTY_MESSAGE: &str =
    "No excellence-debt findings — no oversized files/symbols, metadata debt, proof-locality drift, or open code clones to refactor.";

/// Combine and rank the STATIC excellence-debt sources into one queue: local
/// debt/advisories plus proof-locality and open code-clone advisories, sorted by score
/// (blast radius) descending with stable kind/summary tiebreaks. Pure over its
/// inputs so the ranking is unit-testable without a store — and so the
/// git-derived advisories simply have no path in here.
fn rank_refactor_advisories(
    mut local_debt: Vec<Smell>,
    proof_open: Vec<Smell>,
    clone_open: Vec<Smell>,
) -> Vec<Smell> {
    let mut out = Vec::new();
    out.append(&mut local_debt);
    out.extend(proof_open);
    out.extend(clone_open);
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.summary.cmp(&b.summary))
    });
    out
}

/// The static excellence-debt queue for the refactor lane: local debt/advisories
/// from the (git-free) smell report plus proof-locality and OPEN code-clone
/// advisories (deliberate and hypothesis-tracked clones are already disposed by
/// `code_clone_dispositions`).
fn refactor_candidates(
    db: &dyn GraphReadRepository,
    snapshot: &QuerySnapshot,
) -> Result<Vec<Smell>> {
    let report = db.smell_report(snapshot)?;
    let mut local_debt = report.advisory;
    local_debt.extend(report.debt);

    let clone_patterns: Vec<glob::Pattern> = db
        .list_ignores()?
        .iter()
        .filter_map(|i| glob::Pattern::new(&i.pattern).ok())
        .collect();
    let decision_notes = db.notes_by_kind("decision")?;
    let hypotheses = db.list_hypotheses(None)?;
    let (proof_open, _) = crate::commands::smells::split_advisories_for_adjudication(
        snapshot,
        crate::db::queries::proof_locality_suggestions(snapshot),
        &decision_notes,
    );
    let clone_open = crate::commands::smells::code_clone_dispositions(
        snapshot,
        crate::db::queries::clone_suggestions(snapshot, &clone_patterns),
        &decision_notes,
        &hypotheses,
    )
    .open_advisories;

    Ok(rank_refactor_advisories(local_debt, proof_open, clone_open))
}

pub(super) fn run_refactor(
    db: &dyn GraphReadRepository,
    take_note: Option<&str>,
    printer: &Printer,
) -> Result<()> {
    let snapshot = db.query_snapshot()?;
    let candidates = refactor_candidates(db, &snapshot)?;
    let gs = db.graph_state(&snapshot)?;

    if candidates.is_empty() {
        if printer.json {
            printer.print_json(&inject_take_note(
                serde_json::json!({
                    "status": "empty", "mode": "refactor",
                    "message": REFACTOR_EMPTY_MESSAGE,
                    "next_step": gs.next_action,
                    "graph_state": pulse_json(&gs),
                }),
                take_note,
            ));
        } else {
            println!("✓ {REFACTOR_EMPTY_MESSAGE}");
            println!();
            println!("  {}", fmt_pulse(&gs));
            println!("  → Next: {}", gs.next_action);
        }
        return Ok(());
    }

    let top = &candidates[0];
    let queue_total = candidates.len();

    if printer.json {
        printer.print_json(&inject_take_note(
            serde_json::json!({
                "mode": "refactor",
                "work_kind": "excellence_debt",
                "debt_kind": &top.kind,
                "advisory_kind": &top.kind,
                "priority_score": top.score,
                "summary": &top.summary,
                "evidence": &top.evidence,
                "remedy": &top.remedy,
                "teaching": &top.teaching,
                "queue_total": queue_total,
                "suggested_action": &top.remedy,
                "owner_role": "builder",
                "effort": "high",
                "dispatch": dispatch_line_for_lane("builder", "refactor"),
                "next_step": &top.remedy,
                "note": "EXCELLENCE DEBT — these findings gate the Excellent certificate, not Production-ready. Resolve each by fixing now, deferring as tracked work (`loom hypothesis add` → `loom hypothesis adopt --spawned`), or ruling it false-positive/deliberate-design (`loom note add --smell …`). Accepted/deferred real debt remains debt.",
                "graph_state": pulse_json(&gs),
            }),
            take_note,
        ));
        return Ok(());
    }

    println!(
        "── Next Refactor Item  [{}  priority={:.1}  · EXCELLENCE DEBT] ──",
        top.kind, top.score
    );
    println!();
    println!("  {}", top.summary);
    println!("  evidence: {}", top.evidence);
    println!();
    println!("── Suggested Action ────────────────────────────────────────────────");
    println!("{}", top.remedy);
    println!();
    println!("  teaches: {}", top.teaching.principle);
    if !top.teaching.inspect.is_empty() {
        println!("  inspect: {}", top.teaching.inspect.join(" · "));
    }
    if !top.teaching.avoid.is_empty() {
        println!("  avoid:   {}", top.teaching.avoid.join(" · "));
    }
    println!("  done:    {}", top.teaching.done_when);
    if let Some(m) = more_marker(queue_total, 1, "loom next --mode refactor --take 20") {
        println!();
        println!("  {m}");
    }
    println!();
    println!(
        "  Dispatch — {}  [effort: high]",
        dispatch_line_for_lane("builder", "refactor")
    );
    println!("  {}", fmt_pulse(&gs));
    Ok(())
}

// ---------------------------------------------------------------------------
// Refactor mode, BULK: the same static advisory queue served as a ranked chunk
// so a backlog of size/clone advisories can be triaged in one sitting. There is
// no `loom batch` op for "fix code", so this is a ranked read (each item keeps
// its remedy), not a paste-ready template.
// ---------------------------------------------------------------------------

pub(super) fn run_take_refactor(
    db: &dyn GraphReadRepository,
    take: usize,
    printer: &Printer,
) -> Result<()> {
    const TAKE_CAP: usize = 50;

    let snapshot = db.query_snapshot()?;
    let candidates = refactor_candidates(db, &snapshot)?;
    let gs = db.graph_state(&snapshot)?;
    let queue_total = candidates.len();

    if candidates.is_empty() {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "empty", "mode": "refactor", "taken": 0, "queue_total": 0,
                "message": REFACTOR_EMPTY_MESSAGE,
                "next_step": gs.next_action,
                "graph_state": pulse_json(&gs),
            }));
        } else {
            println!("✓ {REFACTOR_EMPTY_MESSAGE}");
            println!("  {}", fmt_pulse(&gs));
        }
        return Ok(());
    }

    let n = take.min(TAKE_CAP).min(candidates.len());
    let guidance = "EXCELLENCE DEBT — these gate the Excellent certificate, not Production-ready. Per finding, read ITS code, then pick one: \
        fix now (split/dedupe), DEFER as tracked work (`loom hypothesis add` the smell as the claim \
        and the collapsed/split outcome as the prediction → `loom hypothesis adopt --spawned`), or \
        rule it false-positive/deliberate-design (`loom note add --smell \"<id>\" --kind decision --text \"<finding-specific reason>\"`). Accepted/deferred real debt remains visible debt.";

    if printer.json {
        let items: Vec<serde_json::Value> = candidates
            .iter()
            .take(n)
            .map(|s| {
                serde_json::json!({
                    "kind": &s.kind,
                    "score": s.score,
                    "summary": &s.summary,
                    "evidence": &s.evidence,
                    "remedy": &s.remedy,
                })
            })
            .collect();
        printer.print_json(&serde_json::json!({
            "status": "ok", "mode": "refactor", "taken": n, "queue_total": queue_total,
            "items": items,
            "guidance": guidance,
            "dispatch": {"role": "builder", "effort": "high"},
            "graph_state": pulse_json(&gs),
        }));
        return Ok(());
    }

    println!("── Refactor: {n} of {queue_total} excellence-debt finding(s) — ranked ──");
    println!();
    for s in candidates.iter().take(n) {
        println!("  [{}  score {:.1}]  {}", s.kind, s.score, s.summary);
        println!("    remedy: {}", s.remedy);
        println!();
    }
    if let Some(m) = more_marker(queue_total, n, "loom next --mode refactor --take 50") {
        println!("  {m}");
        println!();
    }
    println!("  {guidance}");
    println!("  Dispatch — builder  [effort: high]");
    println!("  {}", fmt_pulse(&gs));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::rank_refactor_advisories;
    use crate::db::queries::{Smell, SmellTeaching};

    fn smell(kind: &str, summary: &str, score: f64) -> Smell {
        Smell {
            kind: kind.into(),
            score,
            summary: summary.into(),
            evidence: "evidence".into(),
            remedy: "remedy".into(),
            teaching: SmellTeaching {
                principle: "p".into(),
                inspect: Vec::new(),
                avoid: Vec::new(),
                done_when: "d".into(),
            },
        }
    }

    #[test]
    fn ranks_size_and_clone_advisories_by_score_desc() {
        // Two size advisories + one clone, interleaved scores: the queue must
        // come back highest-blast-radius first regardless of source bucket.
        let size = vec![
            smell("oversized_file", "big file", 40.0),
            smell("large_behavioral_symbol", "big fn", 10.0),
        ];
        let clone_open = vec![smell("code_clone", "5-copy clone", 25.0)];
        let ranked = rank_refactor_advisories(size, Vec::new(), clone_open);
        let kinds: Vec<&str> = ranked.iter().map(|s| s.kind.as_str()).collect();
        assert_eq!(
            kinds,
            ["oversized_file", "code_clone", "large_behavioral_symbol"],
            "ranked by score desc across both advisory sources"
        );
    }

    #[test]
    fn only_supplied_kinds_appear_no_git_advisories() {
        // The lane is fed exactly the two static buckets — git-derived kinds
        // (cochange_coupling / shotgun_surgery) have no path into the queue.
        let ranked = rank_refactor_advisories(
            vec![smell("oversized_file", "f", 1.0)],
            Vec::new(),
            vec![smell("code_clone", "c", 2.0)],
        );
        assert!(ranked
            .iter()
            .all(|s| matches!(s.kind.as_str(), "oversized_file" | "code_clone")));
        assert!(!ranked
            .iter()
            .any(|s| s.kind == "cochange_coupling" || s.kind == "shotgun_surgery"));
    }

    #[test]
    fn empty_sources_yield_empty_queue() {
        assert!(rank_refactor_advisories(Vec::new(), Vec::new(), Vec::new()).is_empty());
    }
}
