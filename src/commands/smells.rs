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
use std::collections::HashMap;

use crate::db::queries::{
    clone_suggestions, cochange_suggestions, proof_locality_suggestions,
    shotgun_surgery_suggestions, AdjudicatedSmell, QuerySnapshot, Smell, SmellReport,
};
use crate::db::{GraphReadHandle, GraphReadRepository};
use crate::output::Printer;

pub fn run(limit: usize, summary: bool, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let store = GraphReadHandle::open(&cwd)?;
    run_with_db(&store, &cwd, limit, summary, printer)
}

pub fn run_with_db(
    db: &dyn GraphReadRepository,
    root: &std::path::Path,
    limit: usize,
    summary: bool,
    printer: &Printer,
) -> Result<()> {
    let snapshot = db.query_snapshot()?;
    let report = db.smell_report(&snapshot)?;
    let registry = db.vocab_term_count()?;
    let ignores = db.list_ignores()?;
    let decision_notes = db.notes_by_kind("decision")?;
    render(
        root,
        &snapshot,
        report,
        registry,
        &ignores,
        &decision_notes,
        limit,
        summary,
        printer,
    )
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
    let (clone_adv, adjudicated_clones) = split_advisories_for_adjudication(
        snapshot,
        clone_suggestions(snapshot, &clone_patterns),
        decision_notes,
    );
    advisory_adjudicated.extend(adjudicated_clones);
    let clone_total = clone_adv.len();
    let clone_shown: Vec<_> = clone_adv.into_iter().take(limit.max(1)).collect();

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
                "note": "Summary mode omits per-finding evidence, teaching, adjudication bodies, and advisory bodies. Advisory totals count open advisories after current decision-note adjudication; suppressed advisories appear in adjudicated_by_kind.",
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
            println!("  code-clone advisories: {clone_total}");
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
            "note": "Findings are suspicions computed from graph structure — resolve or refute each via its remedy (an `independent` verdict / decision note is as valuable as a fix). OPEN findings gate green: phase=complete requires zero. `adjudicated` lists suppressed findings and advisories WITH their rulings — review them; each names what re-opens it. `cochange_suggestions`, `shotgun_surgery`, `proof_locality_suggestions`, and `code_clones` are ADVISORY — they never gate green, and current decision notes move them out of the open advisory buckets into `adjudicated`.",
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
    if !clone_shown.is_empty() {
        println!();
        println!(
            "── code clones ({}) — ADVISORY (structurally duplicated code in unrelated files; never gate green) ──",
            clone_total
        );
        println!();
        for s in &clone_shown {
            println!("  [{}]  (score {:.1})", s.kind, s.score);
            println!("    {}", s.summary);
            println!("    evidence: {}", s.evidence);
            println!("    remedy:   {}", s.remedy);
            println!();
        }
        if clone_total > clone_shown.len() {
            println!(
                "  ({} more — `loom smells --limit {}`)",
                clone_total - clone_shown.len(),
                clone_total
            );
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
        println!("  Resolve or refute each via its remedy — `independent`/a decision note is as");
        println!("  valuable as a fix. Open findings gate green: phase=complete requires zero.");
    }
    Ok(())
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
            let targets = codefile_ids_in_clone_evidence(snapshot, &advisory.evidence);
            if targets.is_empty() {
                return None;
            }
            let newest = latest_codefile_structure(snapshot, &targets);
            Some(AdvisoryAnchor {
                target_ids: targets,
                newest_structure: newest,
                reopens_when:
                    "any participating file is edited after the ruling, producing a new file timestamp/hash"
                        .into(),
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

fn codefile_ids_in_clone_evidence(snapshot: &QuerySnapshot, evidence: &str) -> Vec<String> {
    snapshot
        .codefiles
        .iter()
        .filter(|codefile| evidence.contains(&format!("{}:", codefile.path)))
        .map(|codefile| codefile.id.clone())
        .collect()
}

fn latest_codefile_structure(snapshot: &QuerySnapshot, codefile_ids: &[String]) -> String {
    let mut newest = String::new();
    for id in codefile_ids {
        if let Some(codefile) = snapshot
            .codefiles
            .iter()
            .find(|codefile| codefile.id == *id)
        {
            newest = max_time(newest, codefile.last_modified.clone());
        }
    }
    newest
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
    use crate::types::{Intent, Note};

    fn intent(id: &str, updated_at: &str) -> Intent {
        Intent {
            id: id.into(),
            name: id.into(),
            description: "test intent".into(),
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
