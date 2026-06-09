//! Derived problem signals (`loom smells`) — the graph as an *instrument*,
//! not just a ledger.
//!
//! Everything else in loom records what an agent told it; this module computes
//! what nobody noticed: duplicate responsibility (split-brain), overlapping
//! ownership, fragmentation, files doing too much, and normative-plane gaps
//! (a QualityRule exists but was never held against an intent that has real
//! code — the measuring stick lying unused next to the thing it should
//! measure). Pure graph computation — no LLM judgment in the *flagging*; the
//! verdict on each finding stays with the inspecting agent, via the exact
//! remedy command each smell carries.

use anyhow::Result;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

use crate::db::LoomDb;

use super::governs::list_all_governs;
use super::hierarchy::list_all_hierarchy;
use super::implements::list_all_implements;
use super::intent::list_intents;
use super::relates_to::list_relates_to;
use super::rule::list_rules;

// Thresholds — deliberately conservative: a smell should be worth a look.
/// Name+description token overlap at/above this is a twin-intent suspicion.
pub const TWIN_SIMILARITY: f64 = 0.4;
/// Scatter thresholds are level-aware: a feature should be cohesive (few
/// files); a component legitimately spans a directory; a system intent
/// grounds to manifests and is never "scattered".
pub fn scatter_threshold(level: &str) -> Option<usize> {
    match level {
        "feature" => Some(4),
        "component" | "cross_cutting" => Some(10),
        _ => None, // system
    }
}
/// A file implemented by this many intents or more is tangled.
pub const TANGLE_INTENTS: usize = 3;

/// One derived finding, with the exact remedy that resolves it.
#[derive(Debug, Clone, Serialize)]
pub struct Smell {
    /// twin_intents | overlapping_ownership | scattered_intent | tangled_file
    /// | unmeasured_intents | unused_rule
    pub kind: String,
    /// Higher = look first (kind-relative magnitude).
    pub score: f64,
    /// One line: what looks wrong.
    pub summary: String,
    /// The computed numbers/names behind the suspicion.
    pub evidence: String,
    /// The exact command sequence that resolves or refutes it.
    pub remedy: String,
}

/// Tokenize a name+description into a normalized word set for overlap checks.
pub fn tokens(text: &str) -> HashSet<String> {
    const STOP: &[&str] = &[
        "the", "and", "via", "with", "for", "that", "this", "from", "into",
        "are", "its", "all", "one", "not", "has", "have", "can", "per",
    ];
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3 && !STOP.contains(w))
        .map(str::to_string)
        .collect()
}

/// Jaccard similarity of two token sets (0.0 when either is empty).
pub fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f64;
    let union = a.union(b).count() as f64;
    inter / union
}

/// Compute every smell, sorted by score (descending) within insertion order of
/// kind. Callers truncate for display.
pub fn compute_smells(db: &dyn LoomDb) -> Result<Vec<Smell>> {
    let intents = list_intents(db, None, None)?;
    let implements = list_all_implements(db)?;
    let hierarchy = list_all_hierarchy(db)?;
    let relates = list_relates_to(db, None)?;
    let rules = list_rules(db)?;
    let governs = list_all_governs(db)?;

    // Lookup structures.
    let mut linked: HashSet<(String, String)> = HashSet::new();
    for e in &relates {
        linked.insert((e.from_id.clone(), e.to_id.clone()));
        linked.insert((e.to_id.clone(), e.from_id.clone()));
    }
    for (p, c) in &hierarchy {
        linked.insert((p.clone(), c.clone()));
        linked.insert((c.clone(), p.clone()));
    }
    let mut files_of: HashMap<&str, HashSet<&str>> = HashMap::new();
    let mut intents_on_file: HashMap<&str, Vec<&str>> = HashMap::new();
    for im in &implements {
        files_of.entry(im.intent_id.as_str()).or_default().insert(im.codefile_path.as_str());
        intents_on_file.entry(im.codefile_path.as_str()).or_default().push(im.intent_id.as_str());
    }
    let name_of: HashMap<&str, &str> =
        intents.iter().map(|i| (i.id.as_str(), i.name.as_str())).collect();
    let toks: HashMap<&str, HashSet<String>> = intents
        .iter()
        .map(|i| (i.id.as_str(), tokens(&format!("{} {}", i.name, i.description))))
        .collect();

    let mut smells: Vec<Smell> = Vec::new();

    // 1. Twin intents — split-brain in the semantic plane: two intents at the
    //    same abstraction level that read like the same responsibility, with
    //    no recorded relationship between them.
    for i in 0..intents.len() {
        for j in (i + 1)..intents.len() {
            let (a, b) = (&intents[i], &intents[j]);
            if a.abstraction_level != b.abstraction_level
                || a.status == "deprecated"
                || b.status == "deprecated"
                || linked.contains(&(a.id.clone(), b.id.clone()))
            {
                continue;
            }
            let sim = jaccard(&toks[a.id.as_str()], &toks[b.id.as_str()]);
            if sim >= TWIN_SIMILARITY {
                smells.push(Smell {
                    kind: "twin_intents".into(),
                    score: sim * 10.0,
                    summary: format!(
                        "'{}' and '{}' read like the same responsibility twice",
                        a.name, b.name
                    ),
                    evidence: format!(
                        "name+description similarity {:.2} at the same level ({}), no edge between them",
                        sim, a.abstraction_level
                    ),
                    remedy: format!(
                        "loom edge explore {} {}  → ground a real relationship, mark independent with why, or deprecate one (loom intent delete / merge their criteria)",
                        a.id, b.id
                    ),
                });
            }
        }
    }

    // 2. Overlapping ownership — split-brain in the physical plane: two
    //    intents grounded in the same file with no recorded relationship.
    //    (Parent/child sharing a file is structure, not a smell — `linked`
    //    covers HIERARCHY too.)
    for i in 0..intents.len() {
        for j in (i + 1)..intents.len() {
            let (a, b) = (&intents[i], &intents[j]);
            if linked.contains(&(a.id.clone(), b.id.clone())) {
                continue;
            }
            let (Some(fa), Some(fb)) = (files_of.get(a.id.as_str()), files_of.get(b.id.as_str()))
            else {
                continue;
            };
            let shared: Vec<&&str> = fa.intersection(fb).collect();
            if !shared.is_empty() {
                let mut names: Vec<String> = shared.iter().map(|s| s.to_string()).collect();
                names.sort();
                smells.push(Smell {
                    kind: "overlapping_ownership".into(),
                    score: 3.0 * shared.len() as f64,
                    summary: format!(
                        "'{}' and '{}' both claim {} file(s) but no relationship is recorded",
                        a.name, b.name, shared.len()
                    ),
                    evidence: format!("shared: {}", names.join(", ")),
                    remedy: format!(
                        "loom edge explore {} {}  → who owns what? ground the contract or mark independent with why",
                        a.id, b.id
                    ),
                });
            }
        }
    }

    // 3. Scattered intent — one responsibility smeared across many files
    //    (threshold scales with abstraction level).
    for i in &intents {
        let (Some(files), Some(threshold)) = (
            files_of.get(i.id.as_str()),
            scatter_threshold(&i.abstraction_level),
        ) else {
            continue;
        };
        if files.len() >= threshold {
            smells.push(Smell {
                kind: "scattered_intent".into(),
                score: files.len() as f64,
                summary: format!(
                    "'{}' is grounded in {} files — responsibility may be fragmented",
                    i.name,
                    files.len()
                ),
                evidence: format!(
                    "a {}-level intent normally stays under {} files",
                    i.abstraction_level, threshold
                ),
                remedy: format!(
                    "decompose it: add child intents per cohesive slice, `loom edge hierarchy {} <child>`, re-ground the children",
                    i.id
                ),
            });
        }
    }

    // 4. Tangled file — one file serving many intents (this is `loom hotspots`
    //    made actionable with a threshold + remedy).
    for (path, iids) in &intents_on_file {
        let distinct: HashSet<&&str> = iids.iter().collect();
        if distinct.len() >= TANGLE_INTENTS {
            let mut names: Vec<&str> = distinct.iter().filter_map(|id| name_of.get(**id).copied()).collect();
            names.sort();
            smells.push(Smell {
                kind: "tangled_file".into(),
                score: distinct.len() as f64,
                summary: format!("{} serves {} distinct intents", path, distinct.len()),
                evidence: format!("intents: {}", names.join(" · ")),
                remedy: format!(
                    "consider splitting {} along intent lines, or mark the owning intent needs_change with the split as the criterion",
                    path
                ),
            });
        }
    }

    // 5. The measuring stick, unused — the normative plane only measures where
    //    someone thought to apply a rule. Surface every rule × intent-with-code
    //    pairing that was never considered (no GOVERNS edge of ANY state —
    //    `independent` records "considered, doesn't apply" and silences this).
    let considered: HashSet<(String, String)> = governs
        .iter()
        .map(|g| (g.rule_id.clone(), g.intent_id.clone()))
        .collect();
    for r in &rules {
        let unmeasured: Vec<&crate::types::Intent> = intents
            .iter()
            .filter(|i| {
                i.status != "deprecated"
                    && files_of.contains_key(i.id.as_str()) // has real code to measure
                    && !considered.contains(&(r.id.clone(), i.id.clone()))
            })
            .collect();
        if unmeasured.is_empty() {
            continue;
        }
        let sample: Vec<String> = unmeasured
            .iter()
            .take(3)
            .map(|i| format!("{} ({})", i.name, i.id))
            .collect();
        smells.push(Smell {
            kind: "unmeasured_intents".into(),
            score: unmeasured.len() as f64,
            summary: format!(
                "rule '{}' has never been held against {} intent(s) that have code",
                r.name,
                unmeasured.len()
            ),
            evidence: format!("e.g. {}", sample.join(" · ")),
            remedy: format!(
                "per intent: loom rule apply {} <intent-id>, inspect, then loom rule verdict {} <intent-id> --status passing|failing|independent (independent = measured, rule doesn't apply)",
                r.id, r.id
            ),
        });
    }

    // 6. Unused rule — a measuring stick connected to nothing at all.
    let used: HashSet<&str> = governs.iter().map(|g| g.rule_id.as_str()).collect();
    for r in &rules {
        if !used.contains(r.id.as_str()) {
            smells.push(Smell {
                kind: "unused_rule".into(),
                score: 5.0,
                summary: format!("rule '{}' governs nothing", r.name),
                evidence: "a quality rule with zero GOVERNS edges measures nothing".into(),
                remedy: format!(
                    "apply it where it belongs (loom rule apply {} <intent-id>) or delete it if it was a mistake",
                    r.id
                ),
            });
        }
    }

    smells.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    Ok(smells)
}
