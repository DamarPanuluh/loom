//! HIERARCHY edge queries (parent/child intent zoom).

use anyhow::Result;

use crate::db::schema::esc;
use crate::db::LoomDb;
use crate::types::Hierarchy;

use super::row::{col_map, get, str_val};

pub fn insert_hierarchy(
    db: &dyn LoomDb,
    edge_id: &str,
    parent_id: &str,
    child_id: &str,
    notes: &str,
    now: &str,
) -> Result<()> {
    let verify_q = format!(
        "MATCH (a:Intent {{id: '{}'}}), (b:Intent {{id: '{}'}}) RETURN a.id, b.id",
        esc(parent_id), esc(child_id)
    );
    if db.execute(&verify_q)?.rows().is_empty() {
        anyhow::bail!(
            "Cannot create HIERARCHY: one or both intents not found.\n\
             parent id: {}\nchild id: {}",
            parent_id, child_id
        );
    }

    // HIERARCHY is a TREE, not a free graph: every intent has at most one parent,
    // and adding an edge must not close a cycle. Enforcing this at insert time
    // (not just flagging in `loom doctor`) keeps the completeness spine well-formed
    // by construction and teaches the model — cross-cutting links are RELATES_TO.
    let existing = list_all_hierarchy(db)?;
    if let Some((p, _)) = existing.iter().find(|(_, c)| c == child_id) {
        if p == parent_id {
            anyhow::bail!("HIERARCHY {} -> {} already exists.", parent_id, child_id);
        }
        anyhow::bail!(
            "Cannot add parent: intent '{}' already has parent '{}'.\n\
             HIERARCHY is a tree — each intent has exactly one parent. Use \
             `loom edge explore` (RELATES_TO) for cross-cutting links.",
            child_id, p
        );
    }
    // Cycle check: a cycle would form iff the new child can already reach the new
    // parent by following child links (covers the self-parent case too).
    if reaches(&existing, child_id, parent_id) {
        anyhow::bail!(
            "Cannot add HIERARCHY {} -> {}: it would create a cycle (the child is \
             already an ancestor of the parent).",
            parent_id, child_id
        );
    }

    db.execute(&format!(
        "MATCH (a:Intent {{id: '{par}'}}), (b:Intent {{id: '{chi}'}}) \
         INSERT (a)-[:HIERARCHY {{id: '{eid}', \
           notes: '{notes}', created_at: '{now}'}}]->(b)",
        par   = esc(parent_id),
        chi   = esc(child_id),
        eid   = esc(edge_id),
        notes = esc(notes),
        now   = esc(now),
    ))?;
    Ok(())
}

pub fn list_hierarchy_for_intent(db: &dyn LoomDb, intent_id: &str) -> Result<Vec<Hierarchy>> {
    // children
    let out_q = format!(
        "MATCH (p:Intent {{id: '{id}'}})-[e:HIERARCHY]->(c:Intent) \
         RETURN e.id, e.notes, \
                p.id AS parent_id, p.name AS parent_name, \
                c.id AS child_id, c.name AS child_name",
        id = esc(intent_id)
    );
    // parents
    let in_q = format!(
        "MATCH (p:Intent)-[e:HIERARCHY]->(c:Intent {{id: '{id}'}}) \
         RETURN e.id, e.notes, \
                p.id AS parent_id, p.name AS parent_name, \
                c.id AS child_id, c.name AS child_name",
        id = esc(intent_id)
    );
    let mut edges = Vec::new();
    for q in &[out_q, in_q] {
        let result = db.execute(q)?;
        let cols = col_map(&result);
        for row in result.rows() {
            edges.push(Hierarchy {
                id:               str_val(get(row, &cols, "e.id")),
                parent_id:        str_val(get(row, &cols, "parent_id")),
                child_id:         str_val(get(row, &cols, "child_id")),
                parent_name:      str_val(get(row, &cols, "parent_name")),
                child_name:       str_val(get(row, &cols, "child_name")),
                notes:            str_val(get(row, &cols, "e.notes")),
            });
        }
    }
    let mut seen = std::collections::HashSet::new();
    edges.retain(|e| seen.insert(e.id.clone()));
    Ok(edges)
}

/// All HIERARCHY edges as (parent_id, child_id) pairs. Node-anchored RETURN
/// (reliable — never matches on the relationship's own properties). The basis
/// for every tree-shape check.
pub fn list_all_hierarchy(db: &dyn LoomDb) -> Result<Vec<(String, String)>> {
    let r = db.execute(
        "MATCH (p:Intent)-[e:HIERARCHY]->(c:Intent) RETURN p.id AS p, c.id AS c",
    )?;
    let cols = col_map(&r);
    Ok(r.rows()
        .iter()
        .map(|row| (str_val(get(row, &cols, "p")), str_val(get(row, &cols, "c"))))
        .collect())
}

/// True if `to` is reachable from `from` following parent→child edges (so
/// `reaches(child, parent)` detects the cycle a new parent→child edge would
/// close). Reflexive: `reaches(x, x)` is true, which rejects self-parenting.
fn reaches(edges: &[(String, String)], from: &str, to: &str) -> bool {
    let mut adj: std::collections::HashMap<&str, Vec<&str>> = std::collections::HashMap::new();
    for (p, c) in edges {
        adj.entry(p.as_str()).or_default().push(c.as_str());
    }
    let mut stack = vec![from];
    let mut seen = std::collections::HashSet::new();
    while let Some(n) = stack.pop() {
        if n == to {
            return true;
        }
        if !seen.insert(n) {
            continue;
        }
        if let Some(children) = adj.get(n) {
            stack.extend(children.iter().copied());
        }
    }
    false
}
