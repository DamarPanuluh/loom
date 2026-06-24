use std::collections::{HashMap, HashSet, VecDeque};

use super::{adjudicate, teaching_for, AdjudicatedSmell, Smell, SmellCtx};
use crate::db::queries::snapshot::QuerySnapshot;

/// Shared reopen-trigger disclosure for the coupling-plane detectors, which
/// reopen when a new IMPLEMENTS grounding changes the import graph.
const REOPENS_ON_NEW_GROUNDING: &str = "a new grounding lands on the importing intent";

/// Coupling plane — undeclared imports and layer-order violations.
pub(super) fn detect_coupling_plane(
    ctx: &SmellCtx,
    smells: &mut Vec<Smell>,
    adj: &mut Vec<AdjudicatedSmell>,
) {
    detect_undeclared_coupling(
        ctx.snapshot,
        &ctx.intents_on_file,
        &ctx.linked,
        &ctx.name_of,
        smells,
    );
    detect_layering_violation(
        ctx.snapshot,
        ctx.intents,
        &ctx.intents_on_file,
        ctx.layer_order,
        &ctx.name_of,
        &ctx.newest_grounding,
        ctx.governs,
        &ctx.rule_kind,
        &ctx.last_decision,
        smells,
        adj,
    );
    detect_transitive_layering_violation(
        ctx.snapshot,
        ctx.intents,
        &ctx.intents_on_file,
        ctx.layer_order,
        &ctx.name_of,
        &ctx.newest_grounding,
        &ctx.last_decision,
        smells,
        adj,
    );
}
/// 6. Undeclared coupling — the physical plane contradicts the semantic: file A
/// statically imports file B, but the intents owning A and B have no recorded
/// relationship.
fn detect_undeclared_coupling(
    snapshot: &QuerySnapshot,
    intents_on_file: &HashMap<&str, Vec<&str>>,
    linked: &HashSet<(&str, &str)>,
    name_of: &HashMap<&str, &str>,
    smells: &mut Vec<Smell>,
) {
    let mut pair_files: HashMap<(String, String), Vec<String>> = HashMap::new();
    for cf in &snapshot.codefiles {
        let Some(owners_a) = intents_on_file.get(cf.path.as_str()) else {
            continue;
        };
        for target in &cf.imports {
            let Some(owners_b) = intents_on_file.get(target.as_str()) else {
                continue;
            };
            let example = format!("{} → {}", cf.path, target);
            let mut seen_pairs: HashSet<(String, String)> = HashSet::new();
            for a in owners_a {
                for b in owners_b {
                    if a == b || linked.contains(&(*a, *b)) {
                        continue;
                    }
                    let key = if a < b {
                        (a.to_string(), b.to_string())
                    } else {
                        (b.to_string(), a.to_string())
                    };
                    if seen_pairs.insert(key.clone()) {
                        pair_files.entry(key).or_default().push(example.clone());
                    }
                }
            }
        }
    }
    for ((a, b), examples) in pair_files {
        let (na, nb) = (
            name_of.get(a.as_str()).copied().unwrap_or(&a),
            name_of.get(b.as_str()).copied().unwrap_or(&b),
        );
        smells.push(Smell {
            kind: "undeclared_coupling".into(),
            score: 4.0 + examples.len() as f64,
            summary: format!(
                "code of '{}' imports code of '{}' but no relationship is recorded",
                na, nb
            ),
            evidence: format!("imports: {}", examples.join(", ")),
            remedy: format!(
                "loom edge explore {} {}  → the code says they touch; ground the contract (or untangle the import)",
                a, b
            ),
            teaching: teaching_for("undeclared_coupling"),
        });
    }
}

/// 6b. Layering violation — the declared order judging the import graph: code
/// owned by a LOWER layer imports code owned by a HIGHER layer. A decision note
/// on the importing intent accepts the up-dependency; a new grounding re-opens.
fn detect_layering_violation(
    snapshot: &QuerySnapshot,
    intents: &[crate::types::Intent],
    intents_on_file: &HashMap<&str, Vec<&str>>,
    layer_order: &[String],
    name_of: &HashMap<&str, &str>,
    newest_grounding: &HashMap<&str, &str>,
    governs: &[crate::types::Governs],
    rule_kind: &HashMap<&str, &str>,
    last_decision: &HashMap<&str, &crate::types::Note>,
    smells: &mut Vec<Smell>,
    adjudicated_out: &mut Vec<AdjudicatedSmell>,
) {
    let layer_rank: HashMap<String, usize> = layer_order
        .iter()
        .cloned()
        .enumerate()
        .map(|(rank, layer)| (layer, rank))
        .collect();
    if layer_rank.is_empty() {
        return;
    }
    let layer_of: HashMap<&str, &str> = intents
        .iter()
        .map(|i| (i.id.as_str(), i.layer.as_str()))
        .collect();
    let mut pair_files: HashMap<(String, String), Vec<String>> = HashMap::new();
    for cf in &snapshot.codefiles {
        let Some(owners_a) = intents_on_file.get(cf.path.as_str()) else {
            continue;
        };
        for target in &cf.imports {
            let Some(owners_b) = intents_on_file.get(target.as_str()) else {
                continue;
            };
            for a in owners_a {
                for b in owners_b {
                    let (Some(&ra), Some(&rb)) = (
                        layer_of.get(*a).and_then(|d| layer_rank.get(*d)),
                        layer_of.get(*b).and_then(|d| layer_rank.get(*d)),
                    ) else {
                        continue; // undeclared layer — exempt
                    };
                    if a == b || ra <= rb {
                        continue;
                    }
                    let example = format!("{} → {}", cf.path, target);
                    let entry = pair_files
                        .entry((a.to_string(), b.to_string()))
                        .or_default();
                    if !entry.contains(&example) {
                        entry.push(example);
                    }
                }
            }
        }
    }
    for ((a, b), examples) in pair_files {
        let (na, nb) = (
            name_of.get(a.as_str()).copied().unwrap_or(&a),
            name_of.get(b.as_str()).copied().unwrap_or(&b),
        );
        let (da, db_) = (
            layer_of.get(a.as_str()).copied().unwrap_or(""),
            layer_of.get(b.as_str()).copied().unwrap_or(""),
        );
        if let Some(note) = adjudicate(
            last_decision,
            "layering_violation",
            a.as_str(),
            newest_grounding.get(a.as_str()).copied().unwrap_or(""),
        ) {
            adjudicated_out.push(AdjudicatedSmell {
                kind: "layering_violation".into(),
                summary: format!(
                    "'{na}' ({da}) depends on '{nb}' ({db_}) against the declared layer order"
                ),
                ruling: note.text.clone(),
                ruled_by: note.author.clone(),
                ruled_at: note.created_at.clone(),
                reopens_when: REOPENS_ON_NEW_GROUNDING.into(),
                teaching: teaching_for("layering_violation"),
            });
            continue;
        }
        smells.push(Smell {
            kind: "layering_violation".into(),
            score: 6.0 + examples.len() as f64,
            summary: format!(
                "'{na}' ({da}) depends on '{nb}' ({db_}) against the declared layer order"
            ),
            evidence: format!(
                "`loom layer order` puts '{da}' below '{db_}', but the dependency points UP: {} (a recorded relationship does not excuse direction)",
                examples.join(", ")
            ),
            remedy: format!(
                "invert the dependency: whatever '{da}' code reaches up to use belongs at or below '{da}' — move it down (or extract it into a lower shared module) so '{db_}' imports it instead of being imported; if the ARCHITECTURE changed, redeclare it: `loom layer order <top> … <bottom>`; if this up-dependency is DELIBERATE, record the call: `loom note add --smell \"layering_violation:{a}\" --kind decision --text \"<why this layer may reach up>\"` resolves this finding (a new grounding re-opens it)"
            ),
            teaching: teaching_for("layering_violation"),
        });
        let arch_passes = governs.iter().any(|g| {
            g.intent_id == a
                && g.inspection_status == "passing"
                && rule_kind.get(g.rule_id.as_str()).copied() == Some("architecture")
        });
        if arch_passes {
            smells.push(Smell {
                kind: super::KIND_ARCH_VERDICT_CONTRADICTS.into(),
                score: 9.0,
                summary: format!(
                    "'{na}' carries a PASSING architecture rule but its dependencies violate the declared layer order"
                ),
                evidence: format!(
                    "an architecture-category GOVERNS verdict passes on '{na}', yet the layering detector flags it: {}",
                    examples.join(", ")
                ),
                remedy: format!(
                    "reconcile the two: re-inspect the architecture rule on '{na}' (`loom rule verdict` — the verdict may be wrong), or fix the layer order (`loom layer order …`); a passing architecture verdict must not coexist with an open layering violation"
                ),
                teaching: teaching_for(super::KIND_ARCH_VERDICT_CONTRADICTS),
            });
        }
    }
}

/// 6c. Transitive layering violation — an up-the-order dependency CLEAN at every
/// single hop (6b never fires) but illegal across the whole path, routed through
/// UNLAYERED intermediates.
fn detect_transitive_layering_violation(
    snapshot: &QuerySnapshot,
    intents: &[crate::types::Intent],
    intents_on_file: &HashMap<&str, Vec<&str>>,
    layer_order: &[String],
    name_of: &HashMap<&str, &str>,
    newest_grounding: &HashMap<&str, &str>,
    last_decision: &HashMap<&str, &crate::types::Note>,
    smells: &mut Vec<Smell>,
    adjudicated_out: &mut Vec<AdjudicatedSmell>,
) {
    let layer_rank: HashMap<&str, usize> = layer_order
        .iter()
        .enumerate()
        .map(|(r, l)| (l.as_str(), r))
        .collect();
    if layer_rank.is_empty() {
        return;
    }
    let layer_of: HashMap<&str, &str> = intents
        .iter()
        .map(|i| (i.id.as_str(), i.layer.as_str()))
        .collect();
    let rank = |id: &str| layer_of.get(id).and_then(|l| layer_rank.get(*l)).copied();
    let mut adj: HashMap<&str, HashSet<&str>> = HashMap::new();
    let mut direct: HashSet<(&str, &str)> = HashSet::new();
    for cf in &snapshot.codefiles {
        let Some(owners_a) = intents_on_file.get(cf.path.as_str()) else {
            continue;
        };
        for target in &cf.imports {
            let Some(owners_b) = intents_on_file.get(target.as_str()) else {
                continue;
            };
            for a in owners_a {
                for b in owners_b {
                    if a == b {
                        continue;
                    }
                    direct.insert((*a, *b));
                    if let (Some(ra), Some(rb)) = (rank(a), rank(b)) {
                        if ra > rb {
                            continue; // directly-violating hop — 6b owns it
                        }
                    }
                    adj.entry(*a).or_default().insert(*b);
                }
            }
        }
    }
    let mut layered: Vec<&str> = intents
        .iter()
        .filter(|i| i.status != "deprecated" && rank(i.id.as_str()).is_some())
        .map(|i| i.id.as_str())
        .collect();
    layered.sort();
    for &a in &layered {
        let Some(ra) = rank(a) else {
            continue;
        };
        let mut parent: HashMap<&str, &str> = HashMap::new();
        let mut seen: HashSet<&str> = HashSet::new();
        let mut q: VecDeque<&str> = VecDeque::new();
        seen.insert(a);
        q.push_back(a);
        while let Some(v) = q.pop_front() {
            if let Some(nbrs) = adj.get(v) {
                let mut ns: Vec<&str> = nbrs.iter().copied().collect();
                ns.sort(); // deterministic path reconstruction
                for w in ns {
                    if seen.insert(w) {
                        parent.insert(w, v);
                        q.push_back(w);
                    }
                }
            }
        }
        let mut reached: Vec<&str> = seen.iter().copied().filter(|&c| c != a).collect();
        reached.sort();
        for c in reached {
            let Some(rc) = rank(c) else {
                continue;
            };
            if rc >= ra || direct.contains(&(a, c)) {
                continue; // not a violating direction, or 6b's direct case
            }
            let mut path = vec![c];
            let mut cur = c;
            while let Some(&p) = parent.get(cur) {
                path.push(p);
                if p == a {
                    break;
                }
                cur = p;
            }
            path.reverse();
            if path.len() < 3 {
                continue; // need at least one intermediate
            }
            let trail = path
                .iter()
                .map(|id| {
                    let n = name_of.get(*id).copied().unwrap_or(id);
                    let l = layer_of.get(*id).copied().unwrap_or("");
                    if l.is_empty() {
                        format!("'{n}' (unlayered)")
                    } else {
                        format!("'{n}' ({l})")
                    }
                })
                .collect::<Vec<_>>()
                .join(" → ");
            let (na, nc) = (
                name_of.get(a).copied().unwrap_or(a),
                name_of.get(c).copied().unwrap_or(c),
            );
            let (la, lc) = (
                layer_of.get(a).copied().unwrap_or(""),
                layer_of.get(c).copied().unwrap_or(""),
            );
            let summary = format!(
                "'{na}' ({la}) transitively depends on '{nc}' ({lc}) against the declared layer order — clean at every hop"
            );
            if let Some(note) = adjudicate(
                last_decision,
                "transitive_layering_violation",
                a,
                newest_grounding.get(a).copied().unwrap_or(""),
            ) {
                adjudicated_out.push(AdjudicatedSmell {
                    kind: "transitive_layering_violation".into(),
                    summary,
                    ruling: note.text.clone(),
                    ruled_by: note.author.clone(),
                    ruled_at: note.created_at.clone(),
                    reopens_when: REOPENS_ON_NEW_GROUNDING.into(),
                    teaching: teaching_for("transitive_layering_violation"),
                });
                continue;
            }
            smells.push(Smell {
                kind: "transitive_layering_violation".into(),
                score: 6.0 + (path.len() - 2) as f64,
                summary,
                evidence: format!(
                    "every hop is clean (6b sees nothing), but the chain routes a deeper layer up to a shallower one through unlayered intermediate(s): {trail}"
                ),
                remedy: format!(
                    "fix the END-TO-END direction: whatever '{la}' reaches up to use belongs at or below '{la}' (move it down / extract a lower shared module); OR give the unlayered intermediate(s) a `--layer` so the direct check governs each hop; OR if this up-dependency is DELIBERATE, record it: `loom note add --smell \"transitive_layering_violation:{a}\" --kind decision --text \"<why this layer may reach up>\"` (a new grounding re-opens it)"
                ),
                teaching: teaching_for("transitive_layering_violation"),
            });
        }
    }
}
