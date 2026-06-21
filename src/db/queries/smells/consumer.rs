use std::collections::{HashMap, HashSet};

use super::{
    adjudicate, rfc3339_after, teaching_for, AdjudicatedSmell, Smell, SmellCtx, ASPECT_FAMILIES,
};
use crate::db::queries::snapshot::QuerySnapshot;

/// Consumer plane — journeys, aspects, symbol accountability, islands.
pub(super) fn detect_consumer_plane(
    ctx: &SmellCtx,
    smells: &mut Vec<Smell>,
    adj: &mut Vec<AdjudicatedSmell>,
) {
    detect_happy_path_only(
        ctx.snapshot,
        ctx.intents,
        ctx.hierarchy,
        &ctx.files_of,
        &ctx.name_of,
        &ctx.last_decision,
        smells,
        adj,
    );
    detect_unjourneyed_surface(
        ctx.snapshot,
        ctx.intents,
        ctx.hierarchy,
        &ctx.files_of,
        &ctx.last_decision,
        smells,
        adj,
    );
    detect_symbol_accountability(
        ctx.snapshot,
        ctx.intents,
        ctx.implements,
        ctx.notes,
        smells,
        adj,
    );
    detect_reciprocal_dependency(
        ctx.snapshot,
        ctx.intents,
        ctx.relates,
        &ctx.name_of,
        &ctx.last_decision,
        smells,
        adj,
    );
    detect_intent_island(
        ctx.intents,
        ctx.hierarchy,
        ctx.relates,
        &ctx.newest_grounding,
        &ctx.last_decision,
        smells,
        adj,
    );
}
/// 9. Happy path only — a feature group that declared its sunny-day intent
/// (aspect=happy/populated) but never realized and proved the required failure
/// or degradation siblings. The required path clears only when implemented,
/// grounded, and directly proven.
fn detect_happy_path_only(
    snapshot: &QuerySnapshot,
    intents: &[crate::types::Intent],
    hierarchy: &[(String, String)],
    files_of: &HashMap<&str, HashSet<&str>>,
    name_of: &HashMap<&str, &str>,
    last_decision: &HashMap<&str, &crate::types::Note>,
    smells: &mut Vec<Smell>,
    adjudicated_out: &mut Vec<AdjudicatedSmell>,
) {
    let mut child_aspects: HashMap<&str, HashSet<&str>> = HashMap::new();
    let mut satisfied_aspects: HashMap<&str, HashSet<&str>> = HashMap::new();
    let mut newest_aspect_child: HashMap<&str, &str> = HashMap::new();
    let by_id: HashMap<&str, &crate::types::Intent> =
        intents.iter().map(|i| (i.id.as_str(), i)).collect();
    let passed_validation_ids: HashSet<&str> = snapshot
        .validations
        .iter()
        .filter(|v| v.last_result == "passed")
        .map(|v| v.id.as_str())
        .collect();
    let directly_proven_intents: HashSet<&str> = snapshot
        .validates
        .iter()
        .filter(|e| passed_validation_ids.contains(e.validation_id.as_str()))
        .map(|e| e.intent_id.as_str())
        .collect();
    for (p, c) in hierarchy {
        let Some(child) = by_id.get(c.as_str()) else {
            continue;
        };
        if child.aspect.is_empty() {
            continue;
        }
        child_aspects
            .entry(p.as_str())
            .or_default()
            .insert(child.aspect.as_str());
        if child.lifecycle == "implemented"
            && files_of.contains_key(child.id.as_str())
            && directly_proven_intents.contains(child.id.as_str())
        {
            satisfied_aspects
                .entry(p.as_str())
                .or_default()
                .insert(child.aspect.as_str());
        }
        let e = newest_aspect_child.entry(p.as_str()).or_default();
        if rfc3339_after(child.created_at.as_str(), e) {
            *e = &child.created_at;
        }
    }
    for (parent_id, aspects) in &child_aspects {
        let satisfied = satisfied_aspects.get(parent_id);
        for (trigger, required) in ASPECT_FAMILIES {
            if !aspects.contains(trigger) {
                continue;
            }
            let missing: Vec<&str> = required
                .iter()
                .filter(|a| !satisfied.is_some_and(|s| s.contains(*a)))
                .copied()
                .collect();
            if missing.is_empty() {
                continue;
            }
            let pname = name_of.get(parent_id).copied().unwrap_or(parent_id);
            let summary = format!(
                "'{pname}' declares a '{trigger}' aspect but no realized+proven {} sibling",
                missing.join("/")
            );
            if let Some(note) = adjudicate(
                last_decision,
                "happy_path_only",
                parent_id,
                newest_aspect_child.get(parent_id).copied().unwrap_or(""),
            ) {
                adjudicated_out.push(AdjudicatedSmell {
                    kind: "happy_path_only".into(),
                    summary,
                    ruling: note.text.clone(),
                    ruled_by: note.author.clone(),
                    ruled_at: note.created_at.clone(),
                    reopens_when: "a new aspect-tagged child lands under this intent".into(),
                    teaching: teaching_for("happy_path_only"),
                });
                continue;
            }
            smells.push(Smell {
                kind: "happy_path_only".into(),
                score: 2.0 + 2.0 * missing.len() as f64,
                summary,
                evidence: format!(
                    "children carry aspects {{{}}}; realized+proven siblings {{{}}} — the '{trigger}' family's {} path(s) are not implemented, grounded, and directly proven",
                    {
                        let mut v: Vec<&str> = aspects.iter().copied().collect();
                        v.sort();
                        v.join(", ")
                    },
                    {
                        let mut v: Vec<&str> = satisfied
                            .map(|s| s.iter().copied().collect())
                            .unwrap_or_default();
                        v.sort();
                        v.join(", ")
                    },
                    missing.join("/")
                ),
                remedy: format!(
                    "realize and prove the missing path(s): loom intent add --aspect {first} --level feature … then loom edge hierarchy {parent_id} <child>, ground it with `loom edge implement`, and attach a passed validation; or record why it's N/A: loom note add --smell \"happy_path_only:{parent_id}\" --kind decision --text \"<why no {m} path>\" (resolves this finding; a new aspect-tagged child re-opens it)",
                    first = missing[0],
                    m = missing.join("/")
                ),
                teaching: teaching_for("happy_path_only"),
            });
        }
    }
}
/// 12. Unjourneyed surface — the consumer plane's completeness check: a
/// user_visible intent with real code that NO PASSED saga exercises end-to-end.
/// Two regimes: zero passed sagas → one aggregate finding on the root;
/// ≥1 passed saga → per-intent findings. Coverage propagates both ways through
/// the tree.
fn detect_unjourneyed_surface(
    snapshot: &QuerySnapshot,
    intents: &[crate::types::Intent],
    hierarchy: &[(String, String)],
    files_of: &HashMap<&str, HashSet<&str>>,
    last_decision: &HashMap<&str, &crate::types::Note>,
    smells: &mut Vec<Smell>,
    adjudicated_out: &mut Vec<AdjudicatedSmell>,
) {
    let parent_of: HashMap<&str, &str> = hierarchy
        .iter()
        .map(|(p, c)| (c.as_str(), p.as_str()))
        .collect();
    let all_saga_ids: HashSet<&str> = snapshot
        .validations
        .iter()
        .filter(|v| v.validation_type == "saga")
        .map(|v| v.id.as_str())
        .collect();
    let passed_saga_ids: HashSet<&str> = snapshot
        .validations
        .iter()
        .filter(|v| v.validation_type == "saga" && v.last_result == "passed")
        .map(|v| v.id.as_str())
        .collect();
    let journeyed: HashSet<&str> = snapshot
        .validates
        .iter()
        .filter(|e| passed_saga_ids.contains(e.validation_id.as_str()))
        .map(|e| e.intent_id.as_str())
        .collect();
    let mut covered: HashSet<&str> = journeyed.clone();
    for id in &journeyed {
        let mut cur = *id;
        let mut visited: HashSet<&str> = HashSet::new();
        while let Some(p) = parent_of.get(cur) {
            if !visited.insert(cur) {
                break;
            }
            covered.insert(p);
            cur = p;
        }
    }
    let mut children_of: HashMap<&str, Vec<&str>> = HashMap::new();
    for (p, c) in hierarchy {
        children_of.entry(p.as_str()).or_default().push(c.as_str());
    }
    let mut stack: Vec<&str> = journeyed.iter().copied().collect();
    let mut walked: HashSet<&str> = HashSet::new();
    while let Some(id) = stack.pop() {
        if !walked.insert(id) {
            continue;
        }
        if let Some(kids) = children_of.get(id) {
            for k in kids {
                covered.insert(k);
                stack.push(k);
            }
        }
    }
    let candidates: Vec<&crate::types::Intent> = intents
        .iter()
        .filter(|i| {
            i.visibility == "user_visible"
                && i.status != "deprecated"
                && i.abstraction_level != "system"
                && files_of.contains_key(i.id.as_str())
                && !covered.contains(i.id.as_str())
        })
        .collect();

    if passed_saga_ids.is_empty() {
        if !candidates.is_empty() {
            let is_child: HashSet<&str> = hierarchy.iter().map(|(_, c)| c.as_str()).collect();
            let mut roots: Vec<&crate::types::Intent> = intents
                .iter()
                .filter(|i| i.status != "deprecated" && !is_child.contains(i.id.as_str()))
                .collect();
            roots.sort_by_key(|i| (i.abstraction_level != "system", i.name.clone()));
            if let Some(root) = roots.first() {
                let newest_uv = intents
                    .iter()
                    .filter(|i| i.visibility == "user_visible")
                    .map(|i| i.created_at.as_str())
                    .max()
                    .unwrap_or("");
                let newest_saga_binding = snapshot
                    .validates
                    .iter()
                    .filter(|e| all_saga_ids.contains(e.validation_id.as_str()))
                    .map(|e| e.created_at.as_str())
                    .max()
                    .unwrap_or("");
                let newest_consumer_surface = std::cmp::max(newest_uv, newest_saga_binding);
                if let Some(note) = adjudicate(
                    last_decision,
                    "unjourneyed_surface",
                    root.id.as_str(),
                    newest_consumer_surface,
                ) {
                    adjudicated_out.push(AdjudicatedSmell {
                        kind: "unjourneyed_surface".into(),
                        summary: format!(
                            "no passed consumer journey — {} user_visible intent(s) never exercised end-to-end",
                            candidates.len()
                        ),
                        ruling: note.text.clone(),
                        ruled_by: note.author.clone(),
                        ruled_at: note.created_at.clone(),
                        reopens_when: "a new user_visible intent or saga binding lands after the ruling (or a first saga passes — per-intent gaps become visible)".into(),
                        teaching: teaching_for("unjourneyed_surface"),
                    });
                } else {
                    let sample: Vec<&str> =
                        candidates.iter().take(3).map(|i| i.name.as_str()).collect();
                    smells.push(Smell {
                        kind: "unjourneyed_surface".into(),
                        score: 3.0 + candidates.len() as f64,
                        summary: format!(
                            "no passed consumer journey — {} user_visible intent(s) are never exercised end-to-end",
                            candidates.len()
                        ),
                        evidence: format!(
                            "the product claims these are consumer-visible, but no passed saga touches any intent: e.g. {}",
                            sample.join(" · ")
                        ),
                        remedy: format!(
                            "narrate the first consumer journey: write the saga YAML (each step binds to the intent it exercises) and `loom saga add <spec.yaml>` (steps may spawn missing intents with --spawn-missing); if this product exposes NO consumer-reachable surface, record the call: `loom note add --smell \"unjourneyed_surface:{}\" --kind decision --text \"no consumer surface: <why>\"` resolves this finding (a new user_visible intent re-opens it)",
                            root.id
                        ),
                        teaching: teaching_for("unjourneyed_surface"),
                    });
                }
            }
        }
    } else {
        for i in candidates {
            if let Some(note) = adjudicate(
                last_decision,
                "unjourneyed_surface",
                i.id.as_str(),
                i.updated_at.as_str(),
            ) {
                adjudicated_out.push(AdjudicatedSmell {
                    kind: "unjourneyed_surface".into(),
                    summary: format!(
                        "'{}' is user_visible but no passed journey exercises it",
                        i.name
                    ),
                    ruling: note.text.clone(),
                    ruled_by: note.author.clone(),
                    ruled_at: note.created_at.clone(),
                    reopens_when: "the intent is redefined after the ruling".into(),
                    teaching: teaching_for("unjourneyed_surface"),
                });
                continue;
            }
            smells.push(Smell {
                kind: "unjourneyed_surface".into(),
                score: if i.abstraction_level == "component" { 5.0 } else { 4.0 },
                summary: format!(
                    "'{}' is user_visible but no passed consumer journey exercises it",
                    i.name
                ),
                evidence: format!(
                    "a {}-level intent ruled user_visible, grounded in code, reached by no passed saga (directly or via the tree)",
                    i.abstraction_level
                ),
                remedy: format!(
                    "extend a journey (or narrate a new one) with a step bound to this intent, then `loom saga add <spec.yaml>` + `loom saga run <name>`; if this surface is not consumer-reachable after all, the ruling is wrong — `loom intent confirm {id} --visibility internal`; if it IS consumer-visible but honestly un-journeyable, record the call: `loom note add --smell \"unjourneyed_surface:{id}\" --kind decision --text \"<why no journey>\"` resolves this finding (a redefinition re-opens it)",
                    id = i.id
                ),
                teaching: teaching_for("unjourneyed_surface"),
            });
        }
    }
}
/// 14. Symbol accountability — public or risky-file symbols without precise
/// ownership are real accountability gaps. Consumes the same instrument that
/// `loom coverage` renders in detail.
fn detect_symbol_accountability(
    snapshot: &QuerySnapshot,
    intents: &[crate::types::Intent],
    implements: &[crate::types::Implements],
    notes: &[crate::types::Note],
    smells: &mut Vec<Smell>,
    adjudicated_out: &mut Vec<AdjudicatedSmell>,
) {
    let report =
        crate::db::queries::symbol_accountability::symbol_accountability_from_parts_with_notes(
            &snapshot.codefiles,
            intents,
            implements,
            notes,
        );
    if !report.actionable_symbol_gaps.is_empty() {
        let examples: Vec<String> = report
            .actionable_symbol_gaps
            .iter()
            .take(5)
            .map(|gap| format!("{} @ {}:{}", gap.label, gap.path, gap.line_start))
            .collect();
        smells.push(Smell {
            kind: "symbol_accountability_gap".into(),
            score: 6.0 + report.actionable_symbol_gaps.len() as f64,
            summary: format!(
                "{} open actionable symbol gap(s): behavior-significant symbols lack precise ownership",
                report.actionable_symbol_gaps.len()
            ),
            evidence: format!(
                "symbol accountability: {} required, {} grounded, {} accepted, {} adjudicated, {} raw gap(s), {} open gap(s). Examples: {}",
                report.summary.required,
                report.summary.grounded,
                report.summary.accepted,
                report.summary.adjudicated,
                report.summary.raw_actionable_gaps,
                report.summary.actionable_gaps,
                examples.join(" · ")
            ),
            remedy: "Use `loom coverage --json` → actionable_symbol_gaps. For each top gap, inspect `loom codefile show <path>`, then refine the right IMPLEMENTS locator, split/add the behavior intent, or record a current decision note on the file/owning intent accepting broad ownership.".into(),
            teaching: teaching_for("symbol_accountability_gap"),
        });
    } else if let Some(gap) = report
        .adjudicated_symbol_gaps
        .iter()
        .max_by_key(|gap| gap.ruled_at.as_str())
    {
        adjudicated_out.push(AdjudicatedSmell {
            kind: "symbol_accountability_gap".into(),
            summary: format!(
                "{} raw symbol gap(s) accepted by current decision notes",
                report.summary.raw_actionable_gaps
            ),
            ruling: gap.ruling.clone(),
            ruled_by: gap.ruled_by.clone(),
            ruled_at: gap.ruled_at.clone(),
            reopens_when: gap.reopens_when.clone(),
            teaching: teaching_for("symbol_accountability_gap"),
        });
    }
}

/// 15. Reciprocal dependency — two intents where BOTH directed RELATES_TO rows
/// carry a real verdict: one undirected relationship stored twice. It
/// double-counts in degree/betweenness and the two rows can silently disagree.
fn detect_reciprocal_dependency(
    snapshot: &QuerySnapshot,
    intents: &[crate::types::Intent],
    relates: &[crate::types::RelatesTo],
    name_of: &HashMap<&str, &str>,
    last_decision: &HashMap<&str, &crate::types::Note>,
    smells: &mut Vec<Smell>,
    adjudicated_out: &mut Vec<AdjudicatedSmell>,
) {
    let active_ids: HashSet<&str> = intents
        .iter()
        .filter(|i| i.status != "deprecated")
        .map(|i| i.id.as_str())
        .collect();
    let mut grounded: HashMap<(&str, &str), &crate::types::RelatesTo> = HashMap::new();
    for e in relates {
        if e.inspection_status == "uninspected"
            || e.inspection_status == "independent"
            || e.from_id == e.to_id
            || !active_ids.contains(e.from_id.as_str())
            || !active_ids.contains(e.to_id.as_str())
        {
            continue;
        }
        grounded.insert((e.from_id.as_str(), e.to_id.as_str()), e);
    }
    for (&(a, b), fwd) in &grounded {
        if a >= b {
            continue; // each unordered pair once; the a<b guard dedupes it
        }
        let Some(rev) = grounded.get(&(b, a)) else {
            continue; // only one direction grounded — not a reciprocal pair
        };
        let (na, nb) = (
            name_of.get(a).copied().unwrap_or(a),
            name_of.get(b).copied().unwrap_or(b),
        );
        let pair_anchor = fwd.last_inspected.as_str().max(rev.last_inspected.as_str());
        let summary =
            format!("mutual RELATES_TO dependency: '{na}' ↔ '{nb}' (both directions grounded)");
        if let Some(note) = adjudicate(last_decision, "dependency_cycle", a, pair_anchor) {
            adjudicated_out.push(AdjudicatedSmell {
                kind: "dependency_cycle".into(),
                summary,
                ruling: note.text.clone(),
                ruled_by: note.author.clone(),
                ruled_at: note.created_at.clone(),
                reopens_when: "either direction's edge is re-inspected".into(),
                teaching: teaching_for("dependency_cycle"),
            });
        } else {
            let deg =
                *snapshot.degrees.get(a).unwrap_or(&0) + *snapshot.degrees.get(b).unwrap_or(&0);
            smells.push(Smell {
                kind: "dependency_cycle".into(),
                score: 6.0 + deg as f64,
                summary,
                evidence: format!(
                    "both directed rows are grounded — {na}→{nb} is {} and {nb}→{na} is {}. RELATES_TO is semantically undirected (the snapshot adds both directions for degree/centrality), so this is ONE relationship stored twice: it double-counts in degree/betweenness and skews `loom next` ranking, and the two verdicts can silently disagree.",
                    fwd.inspection_status, rev.inspection_status
                ),
                remedy: format!(
                    "`loom edge show rt:{a}:{b}` and `loom edge show rt:{b}:{a}`; decide which way the dependency really runs, then `loom edge explore <incidental-from> <incidental-to> independent` to retire the redundant direction (keep the better-grounded verdict). If '{na}' and '{nb}' are one responsibility, merge them. If the mutual relationship is DELIBERATE (true peers / a mutual contract), record it: `loom note add --smell \"dependency_cycle:{a}\" --kind decision --text \"<why both directions hold>\"` (re-inspecting either edge re-opens this)."
                ),
                teaching: teaching_for("dependency_cycle"),
            });
        }
    }
}

/// 16. Intent island — a subgraph with no path to a system-level root. The
/// UNDIRECTED connectivity over HIERARCHY + non-independent RELATES_TO
/// partitions intents into components; a component holding no system-level
/// intent cannot reach any product purpose. One finding per island. When the
/// graph has NO system root at all the detector is unarmed and stays silent.
fn detect_intent_island(
    intents: &[crate::types::Intent],
    hierarchy: &[(String, String)],
    relates: &[crate::types::RelatesTo],
    newest_grounding: &HashMap<&str, &str>,
    last_decision: &HashMap<&str, &crate::types::Note>,
    smells: &mut Vec<Smell>,
    adjudicated_out: &mut Vec<AdjudicatedSmell>,
) {
    let active: Vec<&crate::types::Intent> = intents
        .iter()
        .filter(|i| i.status != "deprecated")
        .collect();
    let n = active.len();
    let has_system = active.iter().any(|i| i.abstraction_level == "system");
    if !(has_system && n > 0) {
        return;
    }
    let idx: HashMap<&str, usize> = active
        .iter()
        .enumerate()
        .map(|(i, intent)| (intent.id.as_str(), i))
        .collect();
    let mut neighbors: Vec<HashSet<usize>> = vec![HashSet::new(); n];
    for (p, c) in hierarchy {
        if let (Some(&a), Some(&b)) = (idx.get(p.as_str()), idx.get(c.as_str())) {
            if a != b {
                neighbors[a].insert(b);
                neighbors[b].insert(a);
            }
        }
    }
    for e in relates {
        if e.inspection_status == "independent" {
            continue;
        }
        if let (Some(&a), Some(&b)) = (idx.get(e.from_id.as_str()), idx.get(e.to_id.as_str())) {
            if a != b {
                neighbors[a].insert(b);
                neighbors[b].insert(a);
            }
        }
    }
    let adjacency: Vec<Vec<usize>> = neighbors
        .into_iter()
        .map(|s| s.into_iter().collect())
        .collect();
    for comp in crate::db::queries::graph_algo::connected_components(n, &adjacency) {
        if comp
            .iter()
            .any(|&i| active[i].abstraction_level == "system")
        {
            continue; // reaches a system root — not an island
        }
        let mut members: Vec<&crate::types::Intent> = comp.iter().map(|&i| active[i]).collect();
        members.sort_by(|a, b| a.id.cmp(&b.id));
        let names: Vec<String> = members.iter().map(|i| format!("'{}'", i.name)).collect();
        let anchor = members[0]; // smallest id
        let island_anchor = members
            .iter()
            .filter_map(|i| newest_grounding.get(i.id.as_str()).copied())
            .max()
            .unwrap_or("");
        let summary = format!(
            "{} intent(s) form an island with no path to a system-level root: {}",
            members.len(),
            names.join(", ")
        );
        if let Some(note) = adjudicate(
            last_decision,
            "intent_island",
            anchor.id.as_str(),
            island_anchor,
        ) {
            adjudicated_out.push(AdjudicatedSmell {
                kind: "intent_island".into(),
                summary,
                ruling: note.text.clone(),
                ruled_by: note.author.clone(),
                ruled_at: note.created_at.clone(),
                reopens_when: "a member is re-grounded".into(),
                teaching: teaching_for("intent_island"),
            });
        } else {
            smells.push(Smell {
                kind: "intent_island".into(),
                score: 5.0 + members.len() as f64,
                summary,
                evidence: format!(
                    "no HIERARCHY or non-independent RELATES_TO path connects {} to any system-level intent: {}",
                    if members.len() == 1 { "this intent" } else { "these intents" },
                    names.join(", ")
                ),
                remedy: format!(
                    "attach the island: `loom edge hierarchy <parent> <child>` under its real parent, or `loom edge explore <a> <b>` to ground a relationship into the connected graph; if it is a genuinely separate top-level purpose, add a system intent for it; if the separation is DELIBERATE, record it: `loom note add --smell \"intent_island:{}\" --kind decision --text \"<why this subgraph is intentionally disconnected>\"` (re-grounding a member re-opens this)",
                    anchor.id
                ),
                teaching: teaching_for("intent_island"),
            });
        }
    }
}
