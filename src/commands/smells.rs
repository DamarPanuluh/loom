//! `loom smells` — make problems obvious, then hand over the methodical
//! remedy. Pure graph computation (see `db::queries::smells`); read-only, so
//! any role may run it. The findings route INTO the normal loop: every smell's
//! remedy is an existing loom command sequence — and OPEN findings gate green
//! (`graph_state` routes phase=audit until zero remain).
//!
//! Adjudicated findings are NOT hidden: a suppressed suspicion prints with
//! its ruling (who, when, why, and what re-opens it). "No findings" and
//! "five findings, all ruled deliberate" must never look alike — the second
//! is an audit surface a human may want to overrule.

use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;

use crate::db::queries::{
    clone_suggestions, cochange_suggestions, proof_locality_suggestions,
    shotgun_surgery_suggestions, AdjudicatedSmell, QuerySnapshot, Smell, SmellReport,
};
use crate::db::{GraphReadHandle, GraphReadRepository};
use crate::output::Printer;

pub fn run(limit: usize, summary: bool, stale: bool, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let store = GraphReadHandle::open(&cwd)?;
    run_with_db(&store, &cwd, limit, summary, stale, printer)
}

pub fn run_with_db(
    db: &dyn GraphReadRepository,
    root: &std::path::Path,
    limit: usize,
    summary: bool,
    stale: bool,
    printer: &Printer,
) -> Result<()> {
    let snapshot = db.query_snapshot()?;
    if stale {
        return render_stale_severity(&snapshot, root, limit, printer);
    }
    let report = db.smell_report(&snapshot)?;
    let registry = db.vocab_term_count()?;
    let ignores = db.list_ignores()?;
    let decision_notes = db.notes_by_kind("decision")?;
    let hypotheses = db.list_hypotheses(None)?;
    render(
        root,
        &snapshot,
        report,
        registry,
        &ignores,
        &decision_notes,
        &hypotheses,
        limit,
        summary,
        printer,
    )
}
/// `loom smells --stale`: turn the undifferentiated "N stale" wall of red into
/// a triaged queue (card 6171c646). Staleness is binary today (sync flips
/// `needs_reverification` when a grounding file's content_hash changes), so
/// 343 stale reads the same for a one-line tweak as a full rewrite. This view
/// splits + ranks by what loom can HONESTLY know live:
///
///   broken   — a grounding file is missing/unreadable, or an IMPLEMENTS
///              locator is no longer present in the file. The grounding target
///              is GONE; this needs re-grounding (`loom edge implement`), not
///              just re-inspection. Highest priority.
///   drift    — the file changed but the grounding target survived. Needs
///              re-inspection (`loom next --mode fix`). Ranked within the tier
///              by current blast radius (symbol count of the grounding file) —
///              biggest re-inspection cost first.
///   no_grounding — the edge's endpoint intent(s) have no code grounding (a
///              concept-level edge). Nothing to re-inspect; lowest priority.
///
/// Honest gap: this ranks by re-inspection COST, not retrospective drift
/// MAGNITUDE. Sync overwrites the stored symbol set to current at flag time,
/// so "how much drifted" is not recoverable live — that needs a future schema
/// field stamped at flag time. The view says so rather than pretending.
fn render_stale_severity(
    snapshot: &QuerySnapshot,
    root: &std::path::Path,
    limit: usize,
    printer: &Printer,
) -> Result<()> {
    // endpoint intent_id -> the codefiles grounding it (path + optional locator)
    let files_by_intent: HashMap<&str, Vec<(&str, &str)>> = snapshot
        .implements
        .iter()
        .map(|im| {
            (
                im.intent_id.as_str(),
                (im.codefile_path.as_str(), im.locator.as_str()),
            )
        })
        .fold(HashMap::new(), |mut acc, (iid, pair)| {
            acc.entry(iid).or_default().push(pair);
            acc
        });

    let mut rows: Vec<StaleEdge> = Vec::new();
    // IMPLEMENTS: the edge itself is the grounding (direct file + locator).
    for im in snapshot
        .implements
        .iter()
        .filter(|im| im.inspection_status == "needs_reverification")
    {
        rows.push(score_stale_edge(
            "implements",
            &format!("{} ({})", im.intent_name, im.intent_id),
            vec![(im.codefile_path.as_str(), im.locator.as_str())],
            root,
        ));
    }
    // RELATES_TO: endpoints are both intents; grounding files = union.
    for rt in snapshot
        .relates
        .iter()
        .filter(|rt| rt.inspection_status == "needs_reverification")
    {
        let mut files: Vec<(&str, &str)> = Vec::new();
        for eid in [rt.from_id.as_str(), rt.to_id.as_str()] {
            if let Some(fs) = files_by_intent.get(eid) {
                files.extend(fs.iter().copied());
            }
        }
        rows.push(score_stale_edge(
            "relates_to",
            &format!("{} ↔ {}", rt.from_name, rt.to_name),
            files,
            root,
        ));
    }
    // GOVERNS: the intent is the code-grounded endpoint (the rule isn't code).
    for g in snapshot
        .governs
        .iter()
        .filter(|g| g.inspection_status == "needs_reverification")
    {
        let files = files_by_intent
            .get(g.intent_id.as_str())
            .cloned()
            .unwrap_or_default();
        rows.push(score_stale_edge(
            "governs",
            &format!("{} ⊢ {}", g.rule_name, g.intent_name),
            files,
            root,
        ));
    }

    // Sort: broken first (by broken-count desc), then drift (by weight desc),
    // then no_grounding, then a stable name tiebreaker.
    rows.sort_by(|a, b| {
        a.tier_rank()
            .cmp(&b.tier_rank())
            .then_with(|| b.weight.cmp(&a.weight))
            .then_with(|| a.endpoints.cmp(&b.endpoints))
    });

    let broken = rows.iter().filter(|r| r.tier == "broken").count();
    let drift = rows.iter().filter(|r| r.tier == "drift").count();
    let no_grounding = rows.iter().filter(|r| r.tier == "no_grounding").count();
    let total = rows.len();

    if printer.json {
        let shown: Vec<&StaleEdge> = rows.iter().take(limit.max(1)).collect();
        printer.print_json(&serde_json::json!({
            "scope": "stale",
            "stale_total": total,
            "broken": broken,
            "drift": drift,
            "no_grounding": no_grounding,
            "truncated": shown.len() < total,
            "edges": shown,
            "note": "Severity ranks by re-inspection cost (current blast radius), not \
                     retrospective drift magnitude — sync overwrites the prior symbol set \
                     at flag time, so drift magnitude needs a future schema field. broken \
                     = re-ground; drift = re-inspect; no_grounding = concept edge, nothing \
                     to re-inspect.",
            "next_step": "broken: re-ground (`loom edge implement`); drift: re-inspect (`loom next --mode fix`)",
        }));
        return Ok(());
    }

    println!("── loom smells · stale severity ────────────────────────────────────");
    if total == 0 {
        println!("  ✓ No stale edges — nothing flagged needs_reverification.");
        return Ok(());
    }
    println!(
        "  {total} stale edge(s) — {broken} broken (re-ground) · {drift} drift (re-inspect) · {no_grounding} no grounding"
    );
    println!("  broken first (grounding target gone), then drift by blast radius.");
    println!("  Ranks by re-inspection cost, NOT drift magnitude (sync overwrites the");
    println!("  prior symbol set at flag time — drift magnitude needs a future field).");
    println!();
    for row in rows.iter().take(limit.max(1)) {
        let mark = match row.tier.as_str() {
            "broken" => "✗",
            "drift" => "~",
            _ => "·",
        };
        println!(
            "  {mark} [{tier}] {kind}  {endpoints}",
            tier = row.tier,
            kind = row.kind,
            endpoints = row.endpoints
        );
        println!(
            "      files: {}",
            if row.files.is_empty() {
                "(none)".into()
            } else {
                row.files.join(", ")
            }
        );
        println!("      {}", row.note);
    }
    if let Some(m) = crate::output::more_marker(
        total,
        limit.max(1),
        "`loom smells --stale --json` for the full list",
    ) {
        println!("  {m}");
    }
    println!();
    println!("  → broken: re-ground (`loom edge implement <intent> <path> --locator …`);");
    println!("    drift: re-inspect (`loom next --mode fix`).");
    Ok(())
}

/// Score one stale edge into a tier + weight from the live state of its
/// grounding files. `files` is (path, locator) pairs; for non-IMPLEMENTS edges
/// the locator is "" (the edge is file-level, not symbol-level).
fn score_stale_edge(
    kind: &'static str,
    endpoints: &str,
    files: Vec<(&str, &str)>,
    root: &std::path::Path,
) -> StaleEdge {
    if files.is_empty() {
        return StaleEdge {
            kind,
            endpoints: endpoints.to_string(),
            tier: "no_grounding".to_string(),
            weight: 0,
            files: Vec::new(),
            note: "no code grounding (concept edge) — nothing to re-inspect".to_string(),
        };
    }
    let mut broken_count = 0usize;
    let mut max_blast = 0usize;
    let mut broken_reasons: Vec<&str> = Vec::new();
    // Dedup by path (both endpoints may ground the same file) so a single
    // missing file isn't counted twice toward broken_count and the display
    // stays clean. First locator wins; later duplicates are dropped.
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let deduped: Vec<(&str, &str)> = files
        .iter()
        .filter(|(p, _)| seen.insert(p))
        .copied()
        .collect();
    for (path, locator) in &deduped {
        match std::fs::read_to_string(root.join(path)) {
            Ok(content) => {
                if !locator.is_empty() && !crate::repo::locator_present(&content, locator) {
                    broken_count += 1;
                    if !broken_reasons.contains(&"locator gone") {
                        broken_reasons.push("locator gone");
                    }
                    continue;
                }
                let blast = crate::repo::extract_physical_facts(root, path, &content)
                    .symbols
                    .len();
                if blast > max_blast {
                    max_blast = blast;
                }
            }
            Err(_) => {
                broken_count += 1;
                if !broken_reasons.contains(&"file missing/unreadable") {
                    broken_reasons.push("file missing/unreadable");
                }
            }
        }
    }
    let (tier, weight, note) = if broken_count > 0 {
        (
            "broken".to_string(),
            broken_count,
            format!(
                "grounding target gone ({}): re-ground",
                broken_reasons.join(", ")
            ),
        )
    } else {
        (
            "drift".to_string(),
            max_blast,
            format!("target survived; blast radius {max_blast} symbol(s): re-inspect"),
        )
    };
    StaleEdge {
        kind,
        endpoints: endpoints.to_string(),
        tier,
        weight,
        files: deduped.iter().map(|(p, _)| p.to_string()).collect(),
        note,
    }
}

#[derive(Debug, Clone, Serialize)]
struct StaleEdge {
    kind: &'static str,
    endpoints: String,
    tier: String,
    weight: usize,
    files: Vec<String>,
    note: String,
}

impl StaleEdge {
    fn tier_rank(&self) -> u8 {
        match self.tier.as_str() {
            "broken" => 0,
            "drift" => 1,
            _ => 2,
        }
    }
}

fn kind_counts(smells: &[Smell]) -> std::collections::BTreeMap<String, usize> {
    let mut counts = std::collections::BTreeMap::new();
    for smell in smells {
        *counts.entry(smell.kind.clone()).or_insert(0) += 1;
    }
    counts
}

fn adjudicated_kind_counts(
    smells: &[AdjudicatedSmell],
) -> std::collections::BTreeMap<String, usize> {
    let mut counts = std::collections::BTreeMap::new();
    for smell in smells {
        *counts.entry(smell.kind.clone()).or_insert(0) += 1;
    }
    counts
}

#[allow(clippy::too_many_arguments)]
fn render(
    root: &std::path::Path,
    snapshot: &QuerySnapshot,
    report: SmellReport,
    registry: usize,
    ignores: &[crate::types::Ignore],
    decision_notes: &[crate::types::Note],
    hypotheses: &[crate::types::Hypothesis],
    limit: usize,
    summary: bool,
    printer: &Printer,
) -> Result<()> {
    // Advisory cochange_coupling suggestions: git-derived, command-only (the
    // audit gate's `compute_smells_from` stays git-free and fast), never gate
    // green. Bounded to recent history; degrades silently with no git.
    let paths: std::collections::HashSet<String> =
        snapshot.codefiles.iter().map(|c| c.path.clone()).collect();
    let cc = crate::repo::git_cochange(root, &paths, 800);
    let (suggestions, mut advisory_adjudicated) = split_advisories_for_adjudication(
        snapshot,
        cochange_suggestions(snapshot, &cc.pairs, &cc.individual),
        decision_notes,
    );
    let suggestions_total = suggestions.len();
    let suggestions_shown: Vec<_> = suggestions.into_iter().take(limit.max(1)).collect();
    let (shotgun_adv, adjudicated_shotgun) = split_advisories_for_adjudication(
        snapshot,
        shotgun_surgery_suggestions(snapshot, &cc.pairs, &cc.individual),
        decision_notes,
    );
    advisory_adjudicated.extend(adjudicated_shotgun);
    let shotgun_total = shotgun_adv.len();
    let shotgun_shown: Vec<_> = shotgun_adv.into_iter().take(limit.max(1)).collect();

    // Advisory proof-locality: STATIC (no git, no coverage run), never gates
    // green. Flags leaves the `proven` axis counts whose only `test` proof
    // resolves to other files than their grounded code.
    let (proof_adv, adjudicated_proof) = split_advisories_for_adjudication(
        snapshot,
        proof_locality_suggestions(snapshot),
        decision_notes,
    );
    advisory_adjudicated.extend(adjudicated_proof);
    let proof_total = proof_adv.len();
    let proof_shown: Vec<_> = proof_adv.into_iter().take(limit.max(1)).collect();

    // Advisory code-clone detection: cross-file normalized structural
    // duplication via SymbolFact.shape_hash, with body_hash fallback for
    // pre-upgrade facts. Ignore-aware (reuse the coverage-exclusion globs),
    // size-floored, never gates green.
    let clone_patterns: Vec<glob::Pattern> = ignores
        .iter()
        .filter_map(|i| glob::Pattern::new(&i.pattern).ok())
        .collect();
    let clone_rollup = code_clone_dispositions(
        snapshot,
        clone_suggestions(snapshot, &clone_patterns),
        decision_notes,
        hypotheses,
    );
    let clone_total = clone_rollup.total;
    let clone_deliberate = clone_rollup.deliberate;
    let clone_tracked = clone_rollup.tracked;
    let clone_open = clone_rollup.open;
    advisory_adjudicated.extend(clone_rollup.adjudicated);
    let clone_shown: Vec<_> = clone_rollup
        .open_advisories
        .into_iter()
        .take(limit.max(1))
        .collect();

    let total = report.open.len();
    let (coded, tagged) = (report.coded_intents, report.tagged_coded_intents);
    let (coded_layers, declared_layers) = (report.coded_layers, report.declared_layers);
    let mut smells = report.open;
    let open_by_kind = kind_counts(&smells);
    smells.truncate(limit);
    let mut adjudicated = report.adjudicated;
    adjudicated.append(&mut advisory_adjudicated);
    let adjudicated_by_kind = adjudicated_kind_counts(&adjudicated);

    if summary {
        let blind = coded.saturating_sub(tagged);
        if printer.json {
            printer.print_json(&serde_json::json!({
                "summary": true,
                "total": total,
                "shown": smells.len(),
                "open_by_kind": open_by_kind,
                "top": smells.iter().map(|s| serde_json::json!({
                    "kind": s.kind,
                    "summary": s.summary,
                    "remedy": s.remedy,
                })).collect::<Vec<_>>(),
                "adjudicated_total": adjudicated.len(),
                "adjudicated_by_kind": adjudicated_by_kind,
                "coded_intents": coded,
                "tagged_coded_intents": tagged,
                "untagged_coded_intents": blind,
                "vocab_terms": registry,
                "coded_layers": coded_layers,
                "declared_layers": declared_layers,
                "cochange_suggestions_total": suggestions_total,
                "shotgun_surgery_total": shotgun_total,
                "proof_locality_suggestions_total": proof_total,
                "code_clones_total": clone_total,
                "code_clones_deliberate": clone_deliberate,
                "code_clones_tracked": clone_tracked,
                "code_clones_open": clone_open,
                "note": "Summary mode omits per-finding evidence, teaching, adjudication bodies, and advisory bodies. Advisory totals count open advisories after current decision-note adjudication; code_clones_total counts physical clone groups and code_clones_* reports their dispositions.",
            }));
        } else {
            println!("── loom smells summary ──────────────────────────────────────────────");
            println!("  open findings: {total}");
            for (kind, count) in &open_by_kind {
                println!("    {kind}: {count}");
            }
            println!("  adjudicated findings: {}", adjudicated.len());
            println!("  co-change advisories: {suggestions_total}");
            println!("  shotgun-surgery advisories: {shotgun_total}");
            println!("  proof-locality advisories: {proof_total}");
            println!("  code clones: {clone_total} — {clone_deliberate} deliberate, {clone_tracked} tracked, {clone_open} open");
            println!("  tagged coded intents: {tagged}/{coded}");
            if blind > 0 {
                println!("  duplicate detector blind spot: {blind} untagged coded intent(s)");
            }
            if declared_layers == 0 && coded_layers >= 2 {
                println!(
                    "  layering detector unarmed: {coded_layers} coded layer(s), no declared order"
                );
            }
            for s in &smells {
                println!("  - [{}] {}", s.kind, s.summary);
                println!("    remedy: {}", s.remedy);
            }
            println!("  Full detail: `loom smells --json`.");
        }
        return Ok(());
    }
    if printer.json {
        printer.print_json(&serde_json::json!({
            "total": total,
            "shown": smells.len(),
            "smells": smells,
            "adjudicated_total": adjudicated.len(),
            "adjudicated": adjudicated,
            "coded_intents": coded,
            "tagged_coded_intents": tagged,
            "vocab_terms": registry,
            "coded_layers": coded_layers,
            "declared_layers": declared_layers,
            "cochange_suggestions": suggestions_shown,
            "cochange_suggestions_total": suggestions_total,
            "shotgun_surgery": shotgun_shown,
            "shotgun_surgery_total": shotgun_total,
            "proof_locality_suggestions": proof_shown,
            "proof_locality_suggestions_total": proof_total,
            "code_clones": clone_shown,
            "code_clones_total": clone_total,
            "code_clones_deliberate": clone_deliberate,
            "code_clones_tracked": clone_tracked,
            "code_clones_open": clone_open,
            "note": "Findings are suspicions computed from graph structure — resolve each via its remedy, ONE at a time after reading ITS code. A decision note is audit trail, not a fix: it must name the decomposition you considered and the concrete reason it is wrong for THIS finding, in terms true only of it — a ruling that restates the size/shape, or repeats one you used elsewhere, is rubber-stamping and loom rejects it (`loom note add --smell` bounces a vacuous/templated ruling; `loom doctor` flags templated clusters). OPEN findings gate green: phase=complete requires zero. `adjudicated` lists suppressed findings and advisories WITH their rulings — review them; each names what re-opens it. `cochange_suggestions`, `shotgun_surgery`, `proof_locality_suggestions`, and `code_clones` are ADVISORY — they never gate green, and current decision notes move them out of the open advisory buckets into `adjudicated`.",
        }));
        return Ok(());
    }

    println!("── loom smells (derived from graph structure — suspicions, not verdicts) ──");
    println!();
    if smells.is_empty() {
        if adjudicated.is_empty() {
            println!("  ✓ No open findings: no twins, no overlapping ownership, no scatter, no");
            println!("    tangles, no source-fact risks, every rule considered against every");
            println!("    coded intent. The audit gate is green.");
        } else {
            println!("  ✓ No OPEN findings — the audit gate is green.");
        }
    }
    for s in &smells {
        println!("  [{}]  (score {:.1})", s.kind, s.score);
        println!("    {}", s.summary);
        println!("    evidence: {}", s.evidence);
        println!("    remedy:   {}", s.remedy);
        println!("    teaches:  {}", s.teaching.principle);
        println!("    inspect:  {}", s.teaching.inspect.join(" · "));
        println!("    avoid:    {}", s.teaching.avoid.join(" · "));
        println!("    done:     {}", s.teaching.done_when);
        println!();
    }
    if total > smells.len() {
        println!(
            "  ({} more — `loom smells --limit {}`)",
            total - smells.len(),
            total
        );
    }
    if !suggestions_shown.is_empty() {
        println!();
        println!(
            "── co-change suggestions ({}) — ADVISORY (git evolutionary coupling; never gate green) ──",
            suggestions_total
        );
        println!();
        for s in &suggestions_shown {
            println!("  [{}]  (score {:.1})", s.kind, s.score);
            println!("    {}", s.summary);
            println!("    evidence: {}", s.evidence);
            println!("    remedy:   {}", s.remedy);
            println!();
        }
        if suggestions_total > suggestions_shown.len() {
            println!(
                "  ({} more — `loom smells --limit {}`)",
                suggestions_total - suggestions_shown.len(),
                suggestions_total
            );
        }
    }
    if !shotgun_shown.is_empty() {
        println!();
        println!(
            "── shotgun-surgery advisories ({}) — ADVISORY (wide git co-change; never gate green) ──",
            shotgun_total
        );
        println!();
        for s in &shotgun_shown {
            println!("  [{}]  (score {:.1})", s.kind, s.score);
            println!("    {}", s.summary);
            println!("    evidence: {}", s.evidence);
            println!("    remedy:   {}", s.remedy);
            println!();
        }
        if shotgun_total > shotgun_shown.len() {
            println!(
                "  ({} more — `loom smells --limit {}`)",
                shotgun_total - shotgun_shown.len(),
                shotgun_total
            );
        }
    }
    if !proof_shown.is_empty() {
        println!();
        println!(
            "── proof-locality advisories ({}) — ADVISORY (proven leaf, test lives elsewhere; never gate green) ──",
            proof_total
        );
        println!();
        for s in &proof_shown {
            println!("  [{}]  (score {:.1})", s.kind, s.score);
            println!("    {}", s.summary);
            println!("    evidence: {}", s.evidence);
            println!("    remedy:   {}", s.remedy);
            println!();
        }
        if proof_total > proof_shown.len() {
            println!(
                "  ({} more — `loom smells --limit {}`)",
                proof_total - proof_shown.len(),
                proof_total
            );
        }
    }
    if clone_total > 0 {
        println!();
        println!(
            "── code clones: {} — {} deliberate, {} tracked, {} open — ADVISORY (structurally duplicated code in unrelated files; never gate green) ──",
            clone_total, clone_deliberate, clone_tracked, clone_open
        );
        println!();
        if clone_shown.is_empty() {
            println!("  (no open clone advisories; all physical clone groups are deliberate or tracked)");
        } else {
            for s in &clone_shown {
                println!("  [{}]  (score {:.1})", s.kind, s.score);
                println!("    {}", s.summary);
                println!("    evidence: {}", s.evidence);
                println!("    remedy:   {}", s.remedy);
                println!();
            }
            if clone_open > clone_shown.len() {
                println!(
                    "  ({} more — `loom smells --limit {}`)",
                    clone_open - clone_shown.len(),
                    clone_open
                );
            }
        }
    }
    if !adjudicated.is_empty() {
        println!();
        println!(
            "── adjudicated ({}) — suppressed by recorded rulings, not by absence ──────",
            adjudicated.len()
        );
        println!();
        for a in &adjudicated {
            println!("  [{}]  {}", a.kind, a.summary);
            println!(
                "    ruling ({}, {}): {}",
                a.ruled_by,
                &a.ruled_at[..a.ruled_at.len().min(19)],
                a.ruling
            );
            println!("    re-opens when: {}", a.reopens_when);
            println!("    teaches: {}", a.teaching.principle);
            println!("    done:    {}", a.teaching.done_when);
            println!();
        }
        // The passive aspiration ("five findings all ruled deliberate must
        // never look alike") made active: if the rulings reuse one template,
        // say so where they're shown — uniformity reads as rubber-stamping.
        let ruling_texts: Vec<&str> = adjudicated.iter().map(|a| a.ruling.as_str()).collect();
        let templated = crate::gate::count_templated_rulings(&ruling_texts, 3);
        if templated >= 3 {
            println!(
                "  ⚠ {templated} of {} adjudications reuse one ruling template — that uniformity",
                adjudicated.len()
            );
            println!(
                "    reads as batch rubber-stamping, not per-finding inspection. Re-audit each"
            );
            println!(
                "    on its own code (`loom doctor` lists the clusters); a real ruling is true"
            );
            println!("    only of its own finding.");
            println!();
        }
        println!("  A ruling you disagree with is overruled through the work, not the ledger:");
        println!("  propose the change (`loom hypothesis add … --target <intent>`) — adoption");
        println!("  restructures the graph and the ruling's subject with it.");
    }
    // The instrument's own coverage, disclosed next to its readings:
    // duplicated_responsibility has a weak lexical fallback for untagged coded
    // pairs, but registered tags are still the high-signal detector.
    let blind = coded - tagged;
    if coded >= 2 && blind > 0 {
        println!();
        if registry == 0 {
            println!("  ⚠ duplicated_responsibility is unarmed: no vocabulary registered, and");
            println!(
                "    {blind} of {coded} coded intent(s) are untagged — only the weaker lexical"
            );
            println!("    fallback can catch same-responsibility pairs in unrelated code. Seed");
            println!(
                "    terms (`loom vocab add`), then tag (`loom intent tag add <intent> <term>`)."
            );
        } else {
            println!(
                "  ⚠ under-armed: {blind} of {coded} coded intent(s) carry no registered tag —"
            );
            println!("    duplicated_responsibility falls back to lexical similarity for those");
            println!("    pairs, but tag collisions are stronger. `loom vocab list` shows the");
            println!("    registry; tag with `loom intent tag add <intent> <term>`.");
        }
    }
    // Same doctrine for the layering instrument: layers in use but no
    // declared order means imports pointing up the architecture are invisible
    // — say so where the readings are.
    if declared_layers == 0 && coded_layers >= 2 {
        println!();
        println!(
            "  ⚠ layering_violation is unarmed: {coded_layers} layers in use across coded intents"
        );
        println!("    but no layer order declared — imports pointing up the architecture are");
        println!("    invisible. Declare it: `loom layer order <top> … <bottom>` (top layer");
        println!("    first; `loom layer list` shows usage; undeclared layers stay exempt).");
    }
    if !smells.is_empty() {
        println!();
        println!("  Resolve each via its remedy, ONE finding at a time after reading ITS code. A");
        println!(
            "  decision note is audit trail, not a fix: name the decomposition you considered"
        );
        println!(
            "  and why it's wrong HERE — restating the size, or reusing a ruling from another"
        );
        println!("  finding, is rubber-stamping (loom rejects it). Open findings gate green.");
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) struct CodeCloneDispositions {
    pub(crate) total: usize,
    pub(crate) deliberate: usize,
    pub(crate) tracked: usize,
    pub(crate) open: usize,
    pub(crate) open_advisories: Vec<Smell>,
    pub(crate) adjudicated: Vec<AdjudicatedSmell>,
}

pub(crate) fn code_clone_dispositions(
    snapshot: &QuerySnapshot,
    advisories: Vec<Smell>,
    decision_notes: &[crate::types::Note],
    hypotheses: &[crate::types::Hypothesis],
) -> CodeCloneDispositions {
    let total = advisories.len();
    let (not_deliberate, adjudicated) =
        split_advisories_for_adjudication(snapshot, advisories, decision_notes);
    let deliberate = adjudicated.len();
    let mut open_advisories = Vec::new();
    let mut tracked = 0usize;

    for advisory in not_deliberate {
        if active_refactor_hypothesis_tracks_clone(snapshot, &advisory, hypotheses) {
            tracked += 1;
        } else {
            open_advisories.push(advisory);
        }
    }

    CodeCloneDispositions {
        total,
        deliberate,
        tracked,
        open: open_advisories.len(),
        open_advisories,
        adjudicated,
    }
}

/// Heuristic only: clone advisories do not carry a first-class refactor edge, so
/// an active hypothesis tracks a clone when its claim/proposal/outcome names any
/// participating file path. Decision notes still win first, keeping the three
/// clone dispositions disjoint.
fn active_refactor_hypothesis_tracks_clone(
    snapshot: &QuerySnapshot,
    advisory: &Smell,
    hypotheses: &[crate::types::Hypothesis],
) -> bool {
    let paths = codefile_paths_in_clone_evidence(snapshot, &advisory.evidence);
    !paths.is_empty()
        && hypotheses.iter().any(|hypothesis| {
            active_hypothesis_status(hypothesis.status.as_str())
                && paths
                    .iter()
                    .any(|path| hypothesis_mentions_path(hypothesis, path))
        })
}

fn active_hypothesis_status(status: &str) -> bool {
    matches!(status, "proposed" | "supported" | "adopted")
}

fn hypothesis_mentions_path(hypothesis: &crate::types::Hypothesis, path: &str) -> bool {
    hypothesis.claim.contains(path)
        || hypothesis.proposal.contains(path)
        || hypothesis.predicted_outcome.contains(path)
}

pub(crate) fn split_advisories_for_adjudication(
    snapshot: &QuerySnapshot,
    advisories: Vec<Smell>,
    decision_notes: &[crate::types::Note],
) -> (Vec<Smell>, Vec<AdjudicatedSmell>) {
    let latest_decision = latest_decision_by_target(decision_notes);
    let mut open = Vec::new();
    let mut adjudicated = Vec::new();

    for advisory in advisories {
        let Some(anchor) = advisory_anchor(snapshot, &advisory) else {
            open.push(advisory);
            continue;
        };
        let Some(note) = anchor
            .target_ids
            .iter()
            .filter_map(|target| latest_decision.get(target.as_str()).copied())
            .filter(|note| {
                anchor.newest_structure.is_empty() || note.created_at > anchor.newest_structure
            })
            .max_by(|a, b| a.created_at.cmp(&b.created_at))
        else {
            open.push(advisory);
            continue;
        };
        adjudicated.push(AdjudicatedSmell {
            kind: advisory.kind,
            summary: advisory.summary,
            ruling: note.text.clone(),
            ruled_by: note.author.clone(),
            ruled_at: note.created_at.clone(),
            reopens_when: anchor.reopens_when,
            teaching: advisory.teaching,
        });
    }

    (open, adjudicated)
}

struct AdvisoryAnchor {
    target_ids: Vec<String>,
    newest_structure: String,
    reopens_when: String,
}

fn latest_decision_by_target(notes: &[crate::types::Note]) -> HashMap<&str, &crate::types::Note> {
    let mut latest: HashMap<&str, &crate::types::Note> = HashMap::new();
    for note in notes {
        if note.kind != "decision" || note.target_id.is_empty() {
            continue;
        }
        latest
            .entry(note.target_id.as_str())
            .and_modify(|existing| {
                if note.created_at > existing.created_at {
                    *existing = note;
                }
            })
            .or_insert(note);
    }
    latest
}

fn advisory_anchor(snapshot: &QuerySnapshot, advisory: &Smell) -> Option<AdvisoryAnchor> {
    match advisory.kind.as_str() {
        "cochange_coupling" => {
            let ids = edge_explore_ids(&advisory.remedy)?;
            let newest = latest_intent_structure(snapshot, &ids);
            Some(AdvisoryAnchor {
                target_ids: ids,
                newest_structure: newest,
                reopens_when:
                    "either co-changing intent is redefined, re-grounded, or re-proven after the ruling"
                        .into(),
            })
        }
        "shotgun_surgery" | "nonlocal_proof" => {
            let id = flag_value(&advisory.remedy, "--intent")?;
            let newest = latest_intent_structure(snapshot, std::slice::from_ref(&id));
            let reopens_when = if advisory.kind == "shotgun_surgery" {
                "the intent is redefined or receives a newer grounding after the ruling"
            } else {
                "the intent's grounding or validation proof changes after the ruling"
            };
            Some(AdvisoryAnchor {
                target_ids: vec![id],
                newest_structure: newest,
                reopens_when: reopens_when.into(),
            })
        }
        "code_clone" => {
            let target = flag_value(&advisory.remedy, "--smell")?;
            if !target.starts_with("code_clone:") {
                return None;
            }
            Some(AdvisoryAnchor {
                target_ids: vec![target],
                newest_structure: String::new(),
                reopens_when: "the clone's normalized shape changes".into(),
            })
        }
        _ => None,
    }
}

fn edge_explore_ids(remedy: &str) -> Option<Vec<String>> {
    let rest = remedy.split("loom edge explore ").nth(1)?;
    let ids: Vec<String> = rest
        .split_whitespace()
        .take(2)
        .map(clean_token)
        .filter(|s| !s.is_empty())
        .collect();
    (ids.len() == 2).then_some(ids)
}

fn flag_value(remedy: &str, flag: &str) -> Option<String> {
    let rest = remedy.split(flag).nth(1)?;
    rest.split_whitespace().next().map(clean_token)
}

fn clean_token(token: &str) -> String {
    token
        .trim_matches(|c: char| c == '`' || c == '\'' || c == '"' || c == ',' || c == ';')
        .to_string()
}

fn latest_intent_structure(snapshot: &QuerySnapshot, intent_ids: &[String]) -> String {
    let mut newest = String::new();
    for id in intent_ids {
        if let Some(intent) = snapshot.intents.iter().find(|intent| intent.id == *id) {
            newest = max_time(newest, intent.updated_at.clone());
        }
        for im in snapshot.implements.iter().filter(|im| im.intent_id == *id) {
            newest = max_time(newest, im.created_at.clone());
            newest = max_time(newest, im.last_inspected.clone());
        }
        for edge in snapshot
            .validates
            .iter()
            .filter(|edge| edge.intent_id == *id)
        {
            newest = max_time(newest, edge.created_at.clone());
            if let Some(validation) = snapshot
                .validations
                .iter()
                .find(|validation| validation.id == edge.validation_id)
            {
                newest = max_time(newest, validation.last_run.clone());
            }
        }
    }
    newest
}

fn codefile_paths_in_clone_evidence<'a>(snapshot: &'a QuerySnapshot, evidence: &str) -> Vec<&'a str> {
    snapshot
        .codefiles
        .iter()
        .filter_map(|codefile| {
            clone_evidence_mentions_path(evidence, codefile.path.as_str())
                .then_some(codefile.path.as_str())
        })
        .collect()
}

fn clone_evidence_mentions_path(evidence: &str, path: &str) -> bool {
    evidence
        .match_indices(path)
        .any(|(idx, _)| evidence.as_bytes().get(idx + path.len()) == Some(&b':'))
}

fn max_time(current: String, candidate: String) -> String {
    if candidate > current {
        candidate
    } else {
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CodeFile, Hypothesis, Intent, Note, SymbolFact};

    #[test]
    fn stale_edge_with_no_grounding_is_no_grounding_tier_not_broken() {
        // A concept-level edge (endpoint intent has no IMPLEMENTS grounding)
        // scores no_grounding — there is nothing to re-inspect, so it must NOT
        // read as broken (which would send a driver re-grounding nothing).
        let edge = score_stale_edge("relates_to", "A ↔ B", vec![], std::path::Path::new("."));
        assert_eq!(edge.tier, "no_grounding");
        assert_eq!(edge.weight, 0);
        assert!(edge.note.contains("concept edge"));
    }

    #[test]
    fn stale_edges_sort_broken_first_then_drift_by_blast_radius() {
        // The queue triages: broken (re-ground) before drift (re-inspect), and
        // within drift the biggest blast radius first. no_grounding is last.
        let mut rows = [
            StaleEdge {
                kind: "relates_to",
                endpoints: "small".into(),
                tier: "drift".into(),
                weight: 3,
                files: vec![],
                note: "n".into(),
            },
            StaleEdge {
                kind: "relates_to",
                endpoints: "big".into(),
                tier: "drift".into(),
                weight: 109,
                files: vec![],
                note: "n".into(),
            },
            StaleEdge {
                kind: "implements",
                endpoints: "gone".into(),
                tier: "broken".into(),
                weight: 1,
                files: vec![],
                note: "n".into(),
            },
            StaleEdge {
                kind: "relates_to",
                endpoints: "concept".into(),
                tier: "no_grounding".into(),
                weight: 0,
                files: vec![],
                note: "n".into(),
            },
        ];
        rows.sort_by(|a, b| {
            a.tier_rank()
                .cmp(&b.tier_rank())
                .then_with(|| b.weight.cmp(&a.weight))
                .then_with(|| a.endpoints.cmp(&b.endpoints))
        });
        assert_eq!(rows[0].endpoints, "gone", "broken is first");
        assert_eq!(
            rows[1].endpoints, "big",
            "drift ranked by blast radius desc"
        );
        assert_eq!(rows[2].endpoints, "small");
        assert_eq!(rows[3].endpoints, "concept", "no_grounding is last");
    }

    fn intent(id: &str, updated_at: &str) -> Intent {
        Intent {
            id: id.into(),
            name: id.into(),
            description: "test intent".into(),
            criterion: String::new(),
            abstraction_level: "feature".into(),
            domain: "test".into(),
            layer: String::new(),
            source_refs: Vec::new(),
            status: "proposed".into(),
            aspect: String::new(),
            lifecycle: "implemented".into(),
            created_at: updated_at.into(),
            updated_at: updated_at.into(),
            tags: Vec::new(),
            visibility: "internal".into(),
            boundary: "inbound".into(),
        }
    }

    fn decision(target_id: &str, created_at: &str) -> Note {
        Note {
            id: format!("note-{target_id}-{created_at}"),
            kind: "decision".into(),
            text: "co-change is deliberate for this fixture".into(),
            author: "human".into(),
            target_kind: "intent".into(),
            target_id: target_id.into(),
            audience: String::new(),
            created_at: created_at.into(),
        }
    }

    fn cochange_advisory() -> Smell {
        Smell {
            kind: "cochange_coupling".into(),
            score: 3.0,
            summary: "two intents move together".into(),
            evidence: "co-change: a.rs ↔ b.rs (3x)".into(),
            remedy: "loom edge explore intent-a intent-b".into(),
            teaching: crate::db::queries::SmellTeaching {
                principle: "test".into(),
                inspect: Vec::new(),
                avoid: Vec::new(),
                done_when: "test".into(),
            },
        }
    }

    fn clone_sym(name: &str, body_hash: &str, shape_hash: &str, line_start: usize) -> SymbolFact {
        SymbolFact {
            label: format!("fn {name}"),
            name: name.into(),
            kind: "fn".into(),
            visibility: "private".into(),
            line_start,
            line_end: line_start + 9,
            is_test: false,
            string_literals: Vec::new(),
            panic_marker_count: 0,
            panic_markers: Vec::new(),
            body_hash: body_hash.into(),
            shape_hash: shape_hash.into(),
        }
    }

    fn clone_cf(path: &str, fact: SymbolFact) -> CodeFile {
        CodeFile {
            id: path.into(),
            path: path.into(),
            language: "rust".into(),
            last_modified: String::new(),
            imports: Vec::new(),
            symbols: vec![fact.label.clone()],
            symbol_facts: vec![fact],
            content_hash: String::new(),
        }
    }

    fn clone_hypothesis(text: &str) -> Hypothesis {
        Hypothesis {
            id: "hyp-tracked-clone".into(),
            name: "tracked clone refactor".into(),
            claim: text.into(),
            proposal: text.into(),
            predicted_outcome: "the clone collapses to one implementation".into(),
            status: "proposed".into(),
            author: "tester".into(),
            evidence: String::new(),
            inspected_by: String::new(),
            last_inspected: String::new(),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    fn empty_report() -> SmellReport {
        SmellReport {
            open: Vec::new(),
            adjudicated: Vec::new(),
            coded_intents: 0,
            tagged_coded_intents: 0,
            coded_layers: 0,
            declared_layers: 0,
        }
    }

    #[test]
    fn render_json_reports_code_clone_disposition_rollup() {
        let snapshot = QuerySnapshot::from_parts(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![
                clone_cf(
                    "src/deliberate_a.rs",
                    clone_sym("deliberate_a", "BODY_DA", "SHAPE_DELIBERATE", 1),
                ),
                clone_cf(
                    "src/deliberate_b.rs",
                    clone_sym("deliberate_b", "BODY_DB", "SHAPE_DELIBERATE", 20),
                ),
                clone_cf(
                    "src/tracked_a.rs",
                    clone_sym("tracked_a", "BODY_TA", "SHAPE_TRACKED", 40),
                ),
                clone_cf(
                    "src/tracked_b.rs",
                    clone_sym("tracked_b", "BODY_TB", "SHAPE_TRACKED", 60),
                ),
                clone_cf("src/open_a.rs", clone_sym("open_a", "BODY_OA", "SHAPE_OPEN", 80)),
                clone_cf("src/open_b.rs", clone_sym("open_b", "BODY_OB", "SHAPE_OPEN", 100)),
            ],
            Some(Vec::new()),
        );
        let printer = crate::output::Printer::capturing(true);
        render(
            std::path::Path::new("."),
            &snapshot,
            empty_report(),
            0,
            &[],
            &[decision("code_clone:SHAPE_DELIBERATE", "2026-01-01T00:00:00Z")],
            &[clone_hypothesis("dedupe src/tracked_a.rs after the release")],
            10,
            false,
            &printer,
        )
        .expect("rendering smells JSON should succeed");

        let captured = printer.captured().expect("json should be captured");
        let json: serde_json::Value =
            serde_json::from_str(&captured).expect("rendered smells output is JSON");
        assert_eq!(json["code_clones_total"], 3);
        assert_eq!(json["code_clones_deliberate"], 1);
        assert_eq!(json["code_clones_tracked"], 1);
        assert_eq!(json["code_clones_open"], 1);
        assert_eq!(
            json["code_clones"].as_array().expect("open clone list").len(),
            1,
            "only unadjudicated and untracked clones stay in the open advisory list"
        );
    }

    #[test]
    fn advisory_decision_note_suppresses_when_current() {
        let snapshot = QuerySnapshot::from_parts(
            vec![
                intent("intent-a", "2026-01-01T00:00:00Z"),
                intent("intent-b", "2026-01-02T00:00:00Z"),
            ],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
        );

        let (open, adjudicated) = split_advisories_for_adjudication(
            &snapshot,
            vec![cochange_advisory()],
            &[decision("intent-a", "2026-01-01T12:00:00Z")],
        );
        assert_eq!(open.len(), 1, "older-than-pair decision must not suppress");
        assert!(adjudicated.is_empty());

        let (open, adjudicated) = split_advisories_for_adjudication(
            &snapshot,
            vec![cochange_advisory()],
            &[decision("intent-a", "2026-01-03T00:00:00Z")],
        );
        assert!(open.is_empty());
        assert_eq!(adjudicated.len(), 1);
        assert_eq!(adjudicated[0].kind, "cochange_coupling");
        assert_eq!(adjudicated[0].ruled_by, "human");
    }
}
