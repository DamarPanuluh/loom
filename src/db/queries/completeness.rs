//! Vertical completeness — the binding, mechanically-verifiable spine.
//!
//! Two planes meet here. The **physical** plane is provable: a CodeFile node must
//! correspond to a real file, and every file is either grounded (≥1 IMPLEMENTS),
//! explicitly excluded, or a gap (see `loom coverage`). The **semantic** plane is
//! LLM judgment — but its *structure* is checkable: HIERARCHY must be a well-formed
//! tree (unique parent, no cycles), and the "join" between planes must hold —
//! every implemented leaf intent is realized (≥1 IMPLEMENTS) and every CodeFile is
//! reached by ≥1 IMPLEMENTS.
//!
//! That spine is **vertical** (decompose intents down, realize them in code). It
//! is binding: `complete` is gated on it. The **horizontal** axis (RELATES_TO, the
//! N×N grid) is optional understanding/cleanup and is reported separately, never
//! gating completeness. See `stats::graph_state`.

use serde::Serialize;
use std::collections::{HashMap, HashSet};

use crate::types::Intent;

use super::snapshot::QuerySnapshot;

/// The verifiable state of the completeness spine. Every field is computed from
/// reliable, node-anchored queries + Rust-side graph analysis.
#[derive(Debug, Clone, Serialize)]
pub struct VerticalCompleteness {
    pub intents: i64,
    /// Intents with no HIERARCHY parent (the tree's roots).
    pub roots: i64,
    /// Intents with no HIERARCHY children (the tree's leaves — what maps to code).
    pub leaves: i64,
    /// Names of intents with more than one parent — a tree violation (hard).
    pub multi_parent: Vec<String>,
    /// A HIERARCHY cycle exists — a tree violation (hard).
    pub cycle: bool,
    /// Root intents whose abstraction_level isn't `system` (soft advisory only;
    /// does not gate completeness).
    pub non_system_roots: Vec<String>,
    /// Implemented leaf intents with no IMPLEMENTS edge — unrealized (the join
    /// from semantic → physical is broken). Either ground or decompose them.
    pub unrealized_leaves: Vec<String>,
    /// `to_be_removed` leaf intents that STILL carry an IMPLEMENTS grounding —
    /// the cleanup is not done: the code marked for deletion is still present.
    /// Falsifiable-by-absence: this clears (and the intent reads done) only once
    /// the grounding is gone.
    pub unremoved_leaves: Vec<String>,
    /// CodeFile paths reached by no IMPLEMENTS edge — code no intent explains.
    pub unreached_codefiles: Vec<String>,
    /// True when the spine is sound: tree well-formed, every implemented leaf
    /// realized, every CodeFile reached. (non_system_roots is advisory, excluded.)
    pub complete: bool,
}

pub fn vertical_completeness_from_snapshot(snapshot: &QuerySnapshot) -> VerticalCompleteness {
    let intents = &snapshot.intents;
    let hier = &snapshot.hierarchy;

    // Tree shape: parent multiplicity, who-is-a-child, who-is-a-parent.
    let mut parent_count: HashMap<&str, usize> = HashMap::new();
    let mut is_child: HashSet<&str> = HashSet::new();
    let mut is_parent: HashSet<&str> = HashSet::new();
    for (p, c) in hier {
        *parent_count.entry(c.as_str()).or_insert(0) += 1;
        is_child.insert(c.as_str());
        is_parent.insert(p.as_str());
    }

    let name_of: HashMap<&str, &str> = intents
        .iter()
        .map(|i| (i.id.as_str(), i.name.as_str()))
        .collect();
    let resolve = |id: &str| name_of.get(id).copied().unwrap_or(id).to_string();

    let mut multi_parent: Vec<String> = parent_count
        .iter()
        .filter(|(_, &n)| n > 1)
        .map(|(id, _)| resolve(id))
        .collect();
    multi_parent.sort();

    let cycle = has_cycle(hier);

    let roots: Vec<&Intent> = intents
        .iter()
        .filter(|i| !is_child.contains(i.id.as_str()))
        .collect();
    let leaves: Vec<&Intent> = intents
        .iter()
        .filter(|i| !is_parent.contains(i.id.as_str()))
        .collect();

    let mut non_system_roots: Vec<String> = roots
        .iter()
        .filter(|i| i.abstraction_level != "system")
        .map(|i| i.name.clone())
        .collect();
    non_system_roots.sort();

    // Realization uses CURRENT groundings only — a leaf whose only grounding
    // is stale (needs_reverification) or broken (failing) is NOT realized; the
    // map must match the territory. The cleanup spine below is the inverse
    // question ("is the to_be_removed code gone yet?") and still counts ANY
    // grounding, because a stale locator means the file changed, not that the
    // code was removed.
    let realized_current = &snapshot.with_current_code;
    let mut unrealized_leaves: Vec<String> = leaves
        .iter()
        .filter(|i| i.lifecycle == "implemented" && !realized_current.contains(&i.id))
        .map(|i| i.name.clone())
        .collect();
    unrealized_leaves.sort();

    // Cleanup spine: a to_be_removed leaf is "done" by ABSENCE — it gates the
    // spine while its code is still grounded, and clears once the grounding is
    // gone (the inverse of unrealized).
    let realized_any = &snapshot.with_code;
    let mut unremoved_leaves: Vec<String> = leaves
        .iter()
        .filter(|i| i.lifecycle == "to_be_removed" && realized_any.contains(&i.id))
        .map(|i| i.name.clone())
        .collect();
    unremoved_leaves.sort();

    let active_ids: HashSet<&str> = intents.iter().map(|i| i.id.as_str()).collect();
    // A file is "reached" only by a CURRENT grounding — a file reached solely
    // by stale/broken locators is not honestly grounded and surfaces as
    // unreached (the territory drifted from the map).
    let reached: HashSet<&str> = snapshot
        .implements
        .iter()
        .filter(|edge| {
            active_ids.contains(edge.intent_id.as_str())
                && edge.inspection_status != "needs_reverification"
                && edge.inspection_status != "failing"
        })
        .map(|edge| edge.codefile_path.as_str())
        .collect();
    let mut unreached_codefiles: Vec<String> = snapshot
        .codefiles
        .iter()
        .map(|c| c.path.clone())
        .filter(|p| !reached.contains(p.as_str()))
        .collect();
    unreached_codefiles.sort();

    let complete = !intents.is_empty()
        && multi_parent.is_empty()
        && !cycle
        && unrealized_leaves.is_empty()
        && unremoved_leaves.is_empty()
        && unreached_codefiles.is_empty();

    VerticalCompleteness {
        intents: intents.len() as i64,
        roots: roots.len() as i64,
        leaves: leaves.len() as i64,
        multi_parent,
        cycle,
        non_system_roots,
        unrealized_leaves,
        unremoved_leaves,
        unreached_codefiles,
        complete,
    }
}

/// Cycle detection over the parent→child digraph via Kahn's algorithm: if some
/// node never reaches in-degree 0, it sits on a cycle. (insert_hierarchy already
/// prevents cycles; this is the safety net for raw/migrated graphs `loom doctor`
/// scans.)
fn has_cycle(edges: &[(String, String)]) -> bool {
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut indeg: HashMap<&str, usize> = HashMap::new();
    let mut nodes: HashSet<&str> = HashSet::new();
    for (p, c) in edges {
        adj.entry(p.as_str()).or_default().push(c.as_str());
        *indeg.entry(c.as_str()).or_insert(0) += 1;
        indeg.entry(p.as_str()).or_insert(0);
        nodes.insert(p.as_str());
        nodes.insert(c.as_str());
    }
    let mut queue: Vec<&str> = nodes.iter().copied().filter(|n| indeg[n] == 0).collect();
    let mut processed = 0usize;
    while let Some(n) = queue.pop() {
        processed += 1;
        if let Some(children) = adj.get(n) {
            for &ch in children {
                if let Some(e) = indeg.get_mut(ch) {
                    *e -= 1;
                    if *e == 0 {
                        queue.push(ch);
                    }
                }
            }
        }
    }
    processed < nodes.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::{edge, edge_key};
    use crate::types::{CodeFile, Implements};

    fn intent(id: &str, name: &str, level: &str, lifecycle: &str) -> Intent {
        Intent {
            id: id.into(),
            name: name.into(),
            description: name.into(),
            criterion: String::new(),
            abstraction_level: level.into(),
            domain: String::new(),
            layer: String::new(),
            source_refs: Vec::new(),
            status: "active".into(),
            aspect: "happy".into(),
            tags: Vec::new(),
            visibility: "internal".into(),
            boundary: String::new(),
            lifecycle: lifecycle.into(),
            created_at: "t".into(),
            updated_at: "t".into(),
        }
    }

    fn grounding(intent_id: &str, cf_id: &str, path: &str, status: &str) -> Implements {
        Implements {
            id: edge_key(edge::IMPLEMENTS, intent_id, cf_id),
            intent_id: intent_id.into(),
            codefile_id: cf_id.into(),
            intent_name: intent_id.into(),
            codefile_path: path.into(),
            inspection_status: status.into(),
            criterion: String::new(),
            confidence: 0.0,
            evidence: String::new(),
            last_inspected: String::new(),
            inspected_by: String::new(),
            locator: String::new(),
            notes: String::new(),
            created_at: "t".into(),
        }
    }

    fn codefile(id: &str, path: &str) -> CodeFile {
        CodeFile {
            id: id.into(),
            path: path.into(),
            language: "rust".into(),
            last_modified: String::new(),
            imports: Vec::new(),
            symbols: Vec::new(),
            symbol_facts: Vec::new(),
            content_hash: String::new(),
        }
    }

    // FALSE-GREEN [map-vs-territory-reconcile-on-read]: a leaf grounded ONLY by
    // a stale (needs_reverification) or broken (failing) locator is NOT realized
    // — the map must match the territory. A passing grounding realizes; a stale-
    // only or failing-only leaf surfaces as unrealized and its file as unreached.
    #[test]
    fn stale_or_failing_grounding_is_not_realization() {
        let root = intent("root", "root", "system", "implemented");
        let leaf_pass = intent("a", "leaf A fresh", "feature", "implemented");
        let leaf_stale = intent("b", "leaf B stale", "feature", "implemented");
        let leaf_fail = intent("c", "leaf C failing", "feature", "implemented");
        let snapshot = QuerySnapshot::from_parts(
            vec![root, leaf_pass, leaf_stale, leaf_fail],
            vec![
                ("root".into(), "a".into()),
                ("root".into(), "b".into()),
                ("root".into(), "c".into()),
            ],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![
                grounding("a", "cfa", "src/a.rs", "passing"),
                grounding("b", "cfb", "src/b.rs", "needs_reverification"),
                grounding("c", "cfc", "src/c.rs", "failing"),
            ],
            vec![
                codefile("cfa", "src/a.rs"),
                codefile("cfb", "src/b.rs"),
                codefile("cfc", "src/c.rs"),
            ],
            None,
        );
        let vc = vertical_completeness_from_snapshot(&snapshot);
        // Only leaf A (passing) is realized; B (stale) and C (failing) are not.
        assert!(
            !vc.unrealized_leaves.contains(&"leaf A fresh".to_string()),
            "a passing grounding realizes: {:?}",
            vc.unrealized_leaves
        );
        assert!(
            vc.unrealized_leaves.contains(&"leaf B stale".to_string()),
            "a needs_reverification-only grounding is NOT realization: {:?}",
            vc.unrealized_leaves
        );
        assert!(
            vc.unrealized_leaves.contains(&"leaf C failing".to_string()),
            "a failing grounding is NOT realization: {:?}",
            vc.unrealized_leaves
        );
        // Files reached only by stale/broken locators are unreached.
        assert!(
            !vc.unreached_codefiles.contains(&"src/a.rs".to_string()),
            "src/a.rs is reached by a passing grounding: {:?}",
            vc.unreached_codefiles
        );
        assert!(
            vc.unreached_codefiles.contains(&"src/b.rs".to_string()),
            "src/b.rs (stale-only grounding) is unreached: {:?}",
            vc.unreached_codefiles
        );
        assert!(
            vc.unreached_codefiles.contains(&"src/c.rs".to_string()),
            "src/c.rs (failing grounding) is unreached: {:?}",
            vc.unreached_codefiles
        );
        assert!(
            !vc.complete,
            "stale/broken groundings must block vertical completeness: {vc:?}"
        );
    }
}
