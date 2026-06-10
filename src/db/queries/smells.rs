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

use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

use crate::db::LoomDb;

use super::codefile::list_codefiles;
use super::governs::list_all_governs;
use super::hierarchy::list_all_hierarchy;
use super::implements::list_all_implements;
use super::intent::list_intents;
use super::note::list_notes;
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
    /// | unmeasured_intents | undeclared_coupling | recurrent_trouble
    /// | unused_rule | happy_path_only
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
            // Group the grounded files by directory — the mechanical clustering
            // evidence for a split. The flagging stays judgment-free: loom shows
            // where the files cluster; the driving LLM names the child intents.
            let mut by_dir: HashMap<&str, usize> = HashMap::new();
            for f in files {
                let dir = std::path::Path::new(f)
                    .parent()
                    .and_then(|p| p.to_str())
                    .filter(|d| !d.is_empty())
                    .unwrap_or(".");
                *by_dir.entry(dir).or_insert(0) += 1;
            }
            let mut dirs: Vec<(&str, usize)> = by_dir.into_iter().collect();
            dirs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            let clusters = dirs
                .iter()
                .map(|(d, n)| format!("{d} ({n})"))
                .collect::<Vec<_>>()
                .join(" · ");
            smells.push(Smell {
                kind: "scattered_intent".into(),
                score: files.len() as f64,
                summary: format!(
                    "'{}' is grounded in {} files — responsibility may be fragmented",
                    i.name,
                    files.len()
                ),
                evidence: format!(
                    "a {}-level intent normally stays under {} files; groundings cluster by directory: {}",
                    i.abstraction_level, threshold, clusters
                ),
                remedy: format!(
                    "split the INTENT, not the code (a too-coarse seed is normal): add a child intent per cohesive slice along the directory clusters, `loom edge hierarchy {id} <child>`, then move groundings down (`loom edge unimplement {id} '<dir>/**'` + `loom edge implement <child> …`); refactoring the code is a separate decision (see tangled_file)",
                    id = i.id
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
    //
    //    HIERARCHY-AWARE: a verdict INHERITS DOWN the tree. A rule held against
    //    a component covers that component's descendants (a child can still get
    //    its own, more specific edge). Measuring at the highest altitude where
    //    the evidence is honest is the *encouraged* strategy — without
    //    inheritance this smell punished it by re-flagging every leaf, inviting
    //    a busywork sweep of vacuous per-leaf verdicts.
    let considered: HashSet<(String, String)> = governs
        .iter()
        .map(|g| (g.rule_id.clone(), g.intent_id.clone()))
        .collect();
    let parent_of: HashMap<&str, &str> = hierarchy
        .iter()
        .map(|(p, c)| (c.as_str(), p.as_str()))
        .collect();
    // Considered directly OR via any ancestor's verdict on the same rule.
    // The tree is insert-enforced acyclic; the visited set is belt-and-braces.
    let considered_up = |rule_id: &str, intent_id: &str| -> bool {
        let mut cur = Some(intent_id);
        let mut visited: HashSet<&str> = HashSet::new();
        while let Some(id) = cur {
            if !visited.insert(id) {
                return false;
            }
            if considered.contains(&(rule_id.to_string(), id.to_string())) {
                return true;
            }
            cur = parent_of.get(id).copied();
        }
        false
    };
    for r in &rules {
        let unmeasured: Vec<&crate::types::Intent> = intents
            .iter()
            .filter(|i| {
                i.status != "deprecated"
                    && files_of.contains_key(i.id.as_str()) // has real code to measure
                    && !considered_up(&r.id, &i.id)
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
                "rule '{}' has never been held against {} intent(s) that have code (neither directly nor via an ancestor's verdict)",
                r.name,
                unmeasured.len()
            ),
            evidence: format!("e.g. {}", sample.join(" · ")),
            remedy: format!(
                "measure at the highest HONEST altitude: loom rule verdict {} <component> --status passing|failing|independent covers the component's descendants too (independent = measured, rule doesn't apply); drop to a leaf only where the rule has specific bite",
                r.id
            ),
        });
    }

    // 6. Undeclared coupling — the physical plane contradicts the semantic:
    //    file A statically imports file B, but the intents owning A and B have
    //    no recorded relationship. The strongest split-brain detector loom has,
    //    because it is grounded in the code itself, not in testimony.
    {
        let mut pair_files: HashMap<(String, String), Vec<String>> = HashMap::new();
        for cf in list_codefiles(db)? {
            let imports: Vec<String> = serde_json::from_str(&cf.imports)
                .with_context(|| format!("Malformed imports JSON for CodeFile '{}'", cf.path))?;
            let Some(owners_a) = intents_on_file.get(cf.path.as_str()) else { continue };
            for target in &imports {
                let Some(owners_b) = intents_on_file.get(target.as_str()) else { continue };
                for a in owners_a {
                    for b in owners_b {
                        if a == b || linked.contains(&(a.to_string(), b.to_string())) {
                            continue;
                        }
                        let key = if a < b {
                            (a.to_string(), b.to_string())
                        } else {
                            (b.to_string(), a.to_string())
                        };
                        let example = format!("{} → {}", cf.path, target);
                        let entry = pair_files.entry(key).or_default();
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
            });
        }
    }

    // 7. Recurrent trouble — the graph's memory of regressions: targets whose
    //    transition history keeps returning to failing / needs_change. A spot
    //    that broke twice will break a third time; it needs redesign, not
    //    another patch.
    {
        let mut trouble: HashMap<(String, String), usize> = HashMap::new();
        for n in list_notes(db, None, Some("transition"))? {
            if n.text.ends_with("→ failing") || n.text.ends_with("→ needs_change") {
                *trouble.entry((n.target_kind.clone(), n.target_id.clone())).or_insert(0) += 1;
            }
        }
        let edge_label: HashMap<&str, String> = {
            let mut m: HashMap<&str, String> = HashMap::new();
            for e in &relates {
                m.insert(e.id.as_str(), format!("{} × {}", e.from_name, e.to_name));
            }
            for g in &governs {
                m.insert(g.id.as_str(), format!("{} → {}", g.rule_name, g.intent_name));
            }
            m
        };
        for ((kind, id), count) in trouble {
            if count < 2 {
                continue;
            }
            let label = if kind == "intent" {
                name_of.get(id.as_str()).copied().unwrap_or(&id).to_string()
            } else {
                edge_label.get(id.as_str()).cloned().unwrap_or_else(|| id.clone())
            };
            smells.push(Smell {
                kind: "recurrent_trouble".into(),
                score: 2.0 * count as f64,
                summary: format!(
                    "'{}' has regressed {} times (transitions to failing/needs_change)",
                    label, count
                ),
                evidence: "see its transition notes (`loom note list --kind transition`)".into(),
                remedy: "recurring breakage means the criterion or the design is wrong — redesign the intent (decompose, re-specify the criterion) instead of patching again".into(),
            });
        }
    }

    // 8. Happy path only — the behavioral vantage point: a feature group that
    //    declared its sunny-day intent (aspect=happy) but never said what
    //    failure or degradation look like. Aspect-tagged siblings are the
    //    mechanical signal; the LLM decides whether sad/fallback are real
    //    requirements here or honestly N/A (record that as a decision note).
    {
        let mut child_aspects: HashMap<&str, HashSet<&str>> = HashMap::new();
        let aspect_of: HashMap<&str, &str> =
            intents.iter().map(|i| (i.id.as_str(), i.aspect.as_str())).collect();
        for (p, c) in &hierarchy {
            if let Some(a) = aspect_of.get(c.as_str()) {
                if !a.is_empty() {
                    child_aspects.entry(p.as_str()).or_default().insert(a);
                }
            }
        }
        for (parent_id, aspects) in &child_aspects {
            if !aspects.contains("happy") {
                continue;
            }
            let missing: Vec<&str> = ["sad", "fallback"]
                .iter()
                .filter(|a| !aspects.contains(*a))
                .copied()
                .collect();
            if missing.is_empty() {
                continue;
            }
            let pname = name_of.get(parent_id).copied().unwrap_or(parent_id);
            smells.push(Smell {
                kind: "happy_path_only".into(),
                score: 2.0 + 2.0 * missing.len() as f64,
                summary: format!(
                    "'{}' declares a happy path but no {} behavior",
                    pname,
                    missing.join("/")
                ),
                evidence: format!(
                    "children carry aspects {{{}}} — failure/degradation behavior is undeclared, so nothing verifies it",
                    {
                        let mut v: Vec<&str> = aspects.iter().copied().collect();
                        v.sort();
                        v.join(", ")
                    }
                ),
                remedy: format!(
                    "declare the missing path(s): loom intent add --aspect sad --level feature … then loom edge hierarchy {parent_id} <child> and ground it; or record why it's N/A: loom note add --intent {parent_id} --kind decision --text \"<why no {m} path>\"",
                    m = missing.join("/")
                ),
            });
        }
    }

    // 9. Unused rule — a measuring stick connected to nothing at all.
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
