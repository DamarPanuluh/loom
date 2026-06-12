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

use anyhow::Result;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

use crate::db::LoomDb;
use crate::types::Intent;

use super::codefile::list_codefiles;
use super::hierarchy::list_all_hierarchy;
use super::implements::intents_with_implements;
use super::intent::list_active_intents;
use super::row::{col_map, get, str_val};
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
    /// CodeFile paths reached by no IMPLEMENTS edge — code no intent explains.
    pub unreached_codefiles: Vec<String>,
    /// True when the spine is sound: tree well-formed, every implemented leaf
    /// realized, every CodeFile reached. (non_system_roots is advisory, excluded.)
    pub complete: bool,
}

/// Compute the vertical-completeness spine. Pure graph analysis — cheap enough
/// for `loom status` / `loom report` on any realistic graph.
pub fn vertical_completeness(db: &dyn LoomDb) -> Result<VerticalCompleteness> {
    let intents = list_active_intents(db)?;
    let hier = list_all_hierarchy(db)?;

    // Tree shape: parent multiplicity, who-is-a-child, who-is-a-parent.
    let mut parent_count: HashMap<&str, usize> = HashMap::new();
    let mut is_child: HashSet<&str> = HashSet::new();
    let mut is_parent: HashSet<&str> = HashSet::new();
    for (p, c) in &hier {
        *parent_count.entry(c.as_str()).or_insert(0) += 1;
        is_child.insert(c.as_str());
        is_parent.insert(p.as_str());
    }

    let name_of: HashMap<&str, &str> =
        intents.iter().map(|i| (i.id.as_str(), i.name.as_str())).collect();
    let resolve = |id: &str| name_of.get(id).copied().unwrap_or(id).to_string();

    let mut multi_parent: Vec<String> = parent_count
        .iter()
        .filter(|(_, &n)| n > 1)
        .map(|(id, _)| resolve(id))
        .collect();
    multi_parent.sort();

    let cycle = has_cycle(&hier);

    let roots: Vec<&Intent> = intents.iter().filter(|i| !is_child.contains(i.id.as_str())).collect();
    let leaves: Vec<&Intent> = intents.iter().filter(|i| !is_parent.contains(i.id.as_str())).collect();

    let mut non_system_roots: Vec<String> = roots
        .iter()
        .filter(|i| i.abstraction_level != "system")
        .map(|i| i.name.clone())
        .collect();
    non_system_roots.sort();

    // The join (semantic → physical): an implemented leaf with no code is
    // unrealized. `planned` leaves legitimately have no code yet (build mode
    // drives those); `needs_change` is surfaced by the build phase.
    let realized = intents_with_implements(db)?;
    let mut unrealized_leaves: Vec<String> = leaves
        .iter()
        .filter(|i| i.lifecycle == "implemented" && !realized.contains(&i.id))
        .map(|i| i.name.clone())
        .collect();
    unrealized_leaves.sort();

    // The join (physical → semantic): a CodeFile no intent reaches is code with
    // no recorded purpose.
    let reached = reached_codefile_paths(db)?;
    let mut unreached_codefiles: Vec<String> = list_codefiles(db)?
        .into_iter()
        .map(|c| c.path)
        .filter(|p| !reached.contains(p))
        .collect();
    unreached_codefiles.sort();

    // An empty graph isn't "complete" — there's nothing realized yet. Requiring
    // ≥1 intent stops the pulse from showing a vacuous vertical ✓ on `phase=empty`.
    let complete = !intents.is_empty()
        && multi_parent.is_empty()
        && !cycle
        && unrealized_leaves.is_empty()
        && unreached_codefiles.is_empty();

    Ok(VerticalCompleteness {
        intents: intents.len() as i64,
        roots: roots.len() as i64,
        leaves: leaves.len() as i64,
        multi_parent,
        cycle,
        non_system_roots,
        unrealized_leaves,
        unreached_codefiles,
        complete,
    })
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

    let name_of: HashMap<&str, &str> =
        intents.iter().map(|i| (i.id.as_str(), i.name.as_str())).collect();
    let resolve = |id: &str| name_of.get(id).copied().unwrap_or(id).to_string();

    let mut multi_parent: Vec<String> = parent_count
        .iter()
        .filter(|(_, &n)| n > 1)
        .map(|(id, _)| resolve(id))
        .collect();
    multi_parent.sort();

    let cycle = has_cycle(hier);

    let roots: Vec<&Intent> = intents.iter().filter(|i| !is_child.contains(i.id.as_str())).collect();
    let leaves: Vec<&Intent> = intents.iter().filter(|i| !is_parent.contains(i.id.as_str())).collect();

    let mut non_system_roots: Vec<String> = roots
        .iter()
        .filter(|i| i.abstraction_level != "system")
        .map(|i| i.name.clone())
        .collect();
    non_system_roots.sort();

    let realized = &snapshot.with_code;
    let mut unrealized_leaves: Vec<String> = leaves
        .iter()
        .filter(|i| i.lifecycle == "implemented" && !realized.contains(&i.id))
        .map(|i| i.name.clone())
        .collect();
    unrealized_leaves.sort();

    let active_ids: HashSet<&str> = intents.iter().map(|i| i.id.as_str()).collect();
    let reached: HashSet<&str> = snapshot
        .implements
        .iter()
        .filter(|edge| active_ids.contains(edge.intent_id.as_str()))
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
        && unreached_codefiles.is_empty();

    VerticalCompleteness {
        intents: intents.len() as i64,
        roots: roots.len() as i64,
        leaves: leaves.len() as i64,
        multi_parent,
        cycle,
        non_system_roots,
        unrealized_leaves,
        unreached_codefiles,
        complete,
    }
}

/// Distinct CodeFile paths reached by at least one IMPLEMENTS edge.
fn reached_codefile_paths(db: &dyn LoomDb) -> Result<HashSet<String>> {
    // Status returned as a column and filtered in Rust: a file owned ONLY by a
    // retired intent is UNREACHED — retiring the sole owner surfaces the file
    // as a vertical gap instead of leaving it covered by dead design.
    let r = db.execute(
        "MATCH (i:Intent)-[e:IMPLEMENTS]->(cf:CodeFile) RETURN cf.path AS p, i.status AS s",
    )?;
    let cols = col_map(&r);
    Ok(r.rows()
        .iter()
        .filter(|row| str_val(get(row, &cols, "s")) != "deprecated")
        .map(|row| str_val(get(row, &cols, "p")))
        .collect())
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
                let e = indeg.get_mut(ch).expect("child in indeg");
                *e -= 1;
                if *e == 0 {
                    queue.push(ch);
                }
            }
        }
    }
    processed < nodes.len()
}
