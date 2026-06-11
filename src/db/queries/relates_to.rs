//! RELATES_TO edge queries — the main intent↔intent grid.
//!
//! Reliability rule: never match/filter a RELATES_TO edge by its own property
//! (`r.id`, `r.inspection_status`) — grafeo 0.5.x does that unreliably. Match by
//! endpoint nodes (a RELATES_TO edge is unique per ordered pair) or scan all and
//! filter in Rust. See the project memory `grafeo-relationship-matching`.

use anyhow::Result;
use grafeo::Value;
use std::collections::HashMap;

use crate::db::schema::esc;
use crate::db::LoomDb;
use crate::types::RelatesTo;

use super::intent::get_intent;
use super::row::{col_map, f64_val, get, str_val};

pub fn get_relates_to(db: &dyn LoomDb, id: &str) -> Result<Option<RelatesTo>> {
    // Resolve by scanning all edges and matching the id in Rust. Filtering a
    // relationship by its own property in the query is unreliable in grafeo
    // 0.5.x; a full traversal is not. Prefer get_relates_to_between when the
    // endpoints are known — it is a direct node-keyed lookup.
    Ok(list_relates_to(db, None)?.into_iter().find(|e| e.id == id))
}

pub fn get_relates_to_between(
    db: &dyn LoomDb,
    from_id: &str,
    to_id: &str,
) -> Result<Option<RelatesTo>> {
    let q = format!(
        "MATCH (a:Intent {{id: '{from}'}})-[r:RELATES_TO]->(b:Intent {{id: '{to}'}}) \
         RETURN r.id, r.inspection_status, r.criterion, r.confidence, r.evidence, \
                r.last_inspected, r.inspected_by, r.priority_score, r.notes, \
                a.id AS from_id, a.name AS from_name, \
                b.id AS to_id, b.name AS to_name",
        from = esc(from_id),
        to   = esc(to_id),
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    Ok(result.rows().first().map(|row| row_to_relates_to(row, &cols)))
}

pub fn get_or_create_relates_to(
    db: &dyn LoomDb,
    edge_id: &str,
    from_id: &str,
    to_id: &str,
    now: &str,
) -> Result<RelatesTo> {
    // Check: does an edge already exist between these two intents? Resolve it
    // by endpoints (node-keyed lookup is reliable) rather than by edge id.
    if let Some(existing) = get_relates_to_between(db, from_id, to_id)? {
        return Ok(existing);
    }

    // Ensure both intent nodes exist
    let verify_q = format!(
        "MATCH (a:Intent {{id: '{}'}}), (b:Intent {{id: '{}'}}) RETURN a.id, b.id",
        esc(from_id), esc(to_id)
    );
    let verify = db.execute(&verify_q)?;
    if verify.rows().is_empty() {
        anyhow::bail!(
            "Cannot create edge: one or both intents not found.\n\
             intent-a id: {}\n\
             intent-b id: {}\n\
             Run `loom intent list` to see available intents.",
            from_id, to_id
        );
    }

    // Create the edge
    let insert_q = format!(
        "MATCH (a:Intent {{id: '{from}'}}), (b:Intent {{id: '{to}'}}) \
         INSERT (a)-[:RELATES_TO {{id: '{eid}', inspection_status: 'uninspected', \
           criterion: '', confidence: 0.0, evidence: '', last_inspected: '', \
           inspected_by: '', priority_score: 0.0, notes: '', created_at: '{now}'}}]->(b)",
        from = esc(from_id),
        to   = esc(to_id),
        eid  = esc(edge_id),
        now  = esc(now),
    );
    db.execute(&insert_q)?;

    // Construct the result from the values we just inserted rather than reading
    // the relationship back. grafeo 0.5.x does not reliably return a freshly
    // INSERTed relationship by property within the same session — the
    // relationship index update races the subsequent read, which surfaced as
    // intermittent "Edge was just inserted but cannot be retrieved" errors.
    // Every field below mirrors the INSERT defaults above; only the endpoint
    // names need a lookup, and single-node reads are reliable.
    let from_name = get_intent(db, from_id)?.map(|i| i.name).unwrap_or_default();
    let to_name = get_intent(db, to_id)?.map(|i| i.name).unwrap_or_default();
    Ok(RelatesTo {
        id:                edge_id.to_string(),
        from_id:           from_id.to_string(),
        to_id:             to_id.to_string(),
        from_name,
        to_name,
        inspection_status: "uninspected".to_string(),
        criterion:         String::new(),
        confidence:        0.0,
        evidence:          String::new(),
        last_inspected:    String::new(),
        inspected_by:      String::new(),
        priority_score:    0.0,
        notes:             String::new(),
    })
}

pub fn list_relates_to(
    db: &dyn LoomDb,
    status_filter: Option<&str>,
) -> Result<Vec<RelatesTo>> {
    // Full traversal returning every edge with its properties is reliable;
    // filtering a relationship by its own property in the query (WHERE/inline)
    // is NOT reliable in grafeo 0.5.x. So we always scan and filter in Rust.
    let q = "MATCH (a:Intent)-[r:RELATES_TO]->(b:Intent) \
             RETURN r.id, r.inspection_status, r.criterion, r.confidence, r.evidence, \
                    r.last_inspected, r.inspected_by, r.priority_score, r.notes, \
                    a.id AS from_id, a.name AS from_name, \
                    b.id AS to_id, b.name AS to_name \
             ORDER BY r.priority_score DESC";
    let result = db.execute(q)?;
    let cols = col_map(&result);
    let mut edges: Vec<RelatesTo> =
        result.rows().iter().map(|row| row_to_relates_to(row, &cols)).collect();
    if let Some(s) = status_filter {
        edges.retain(|e| e.inspection_status == s);
    }
    Ok(edges)
}

// The RELATES_TO update helpers below identify the edge by its endpoint node
// ids rather than by the edge's own id property. There is at most one
// RELATES_TO edge per ordered (from, to) pair (enforced by
// get_or_create_relates_to), so the endpoints are a unique key — and matching a
// relationship via its endpoint nodes is reliable, whereas filtering by the
// relationship's own property is not in grafeo 0.5.x.

/// Set inspection_status = passing (was: grounded) with meta.
pub fn update_relates_to_ground(
    db: &dyn LoomDb,
    from_id: &str,
    to_id: &str,
    criterion: &str,
    confidence: f64,
    inspected_by: &str,
    now: &str,
) -> Result<bool> {
    let Some(prev) = get_relates_to_between(db, from_id, to_id)? else {
        return Ok(false);
    };
    db.execute(&format!(
        "MATCH (a:Intent {{id: '{from}'}})-[r:RELATES_TO]->(b:Intent {{id: '{to}'}}) \
         SET r.inspection_status = 'passing', r.criterion = '{crit}', \
             r.confidence = {conf}, r.inspected_by = '{by}', \
             r.last_inspected = '{now}'",
        from = esc(from_id),
        to   = esc(to_id),
        crit = esc(criterion),
        conf = confidence,
        by   = esc(inspected_by),
        now  = esc(now),
    ))?;
    super::note::record_transition(db, "edge", &prev.id, &prev.inspection_status, "passing", inspected_by, now)?;
    Ok(true)
}

/// Set inspection_status = failing (was: issue_found) with evidence.
pub fn update_relates_to_issue(
    db: &dyn LoomDb,
    from_id: &str,
    to_id: &str,
    criterion: &str,
    evidence: &str,
    confidence: f64,
    inspected_by: &str,
    now: &str,
) -> Result<bool> {
    let Some(prev) = get_relates_to_between(db, from_id, to_id)? else {
        return Ok(false);
    };
    db.execute(&format!(
        "MATCH (a:Intent {{id: '{from}'}})-[r:RELATES_TO]->(b:Intent {{id: '{to}'}}) \
         SET r.inspection_status = 'failing', r.criterion = '{crit}', \
             r.evidence = '{ev}', r.confidence = {conf}, r.inspected_by = '{by}', \
             r.last_inspected = '{now}'",
        from = esc(from_id),
        to   = esc(to_id),
        crit = esc(criterion),
        ev   = esc(evidence),
        conf = confidence,
        by   = esc(inspected_by),
        now  = esc(now),
    ))?;
    super::note::record_transition(db, "edge", &prev.id, &prev.inspection_status, "failing", inspected_by, now)?;
    Ok(true)
}

/// Set inspection_status = independent (replaces old CONFIRMED_INDEPENDENT edge type).
pub fn update_relates_to_independent(
    db: &dyn LoomDb,
    from_id: &str,
    to_id: &str,
    notes: &str,
    inspected_by: &str,
    now: &str,
) -> Result<bool> {
    let Some(prev) = get_relates_to_between(db, from_id, to_id)? else {
        return Ok(false);
    };
    db.execute(&format!(
        "MATCH (a:Intent {{id: '{from}'}})-[r:RELATES_TO]->(b:Intent {{id: '{to}'}}) \
         SET r.inspection_status = 'independent', r.notes = '{notes}', \
             r.inspected_by = '{by}', r.last_inspected = '{now}'",
        from  = esc(from_id),
        to    = esc(to_id),
        notes = esc(notes),
        by    = esc(inspected_by),
        now   = esc(now),
    ))?;
    super::note::record_transition(db, "edge", &prev.id, &prev.inspection_status, "independent", inspected_by, now)?;
    Ok(true)
}

/// Stamp a RELATES_TO edge with RUNTIME evidence from a saga run (the
/// consumer plane): `status` is passing (both steps of the pair executed and
/// passed) or failing (the boundary where the chain broke). An existing
/// non-empty criterion is PRESERVED — execution evidence refines the
/// analyzer's contract, it does not overwrite it; `default_criterion` only
/// fills a blank. Evidence always carries the run detail, so a green edge can
/// say "proven by execution", not just "read and believed".
pub fn stamp_relates_to_runtime(
    db: &dyn LoomDb,
    from_id: &str,
    to_id: &str,
    status: &str,
    default_criterion: &str,
    evidence: &str,
    confidence: f64,
    inspected_by: &str,
    now: &str,
) -> Result<bool> {
    let Some(prev) = get_relates_to_between(db, from_id, to_id)? else {
        return Ok(false);
    };
    let criterion = if prev.criterion.trim().is_empty() {
        default_criterion
    } else {
        &prev.criterion
    };
    db.execute(&format!(
        "MATCH (a:Intent {{id: '{from}'}})-[r:RELATES_TO]->(b:Intent {{id: '{to}'}}) \
         SET r.inspection_status = '{status}', r.criterion = '{crit}', \
             r.evidence = '{ev}', r.confidence = {conf}, r.inspected_by = '{by}', \
             r.last_inspected = '{now}'",
        from   = esc(from_id),
        to     = esc(to_id),
        status = esc(status),
        crit   = esc(criterion),
        ev     = esc(evidence),
        conf   = confidence,
        by     = esc(inspected_by),
        now    = esc(now),
    ))?;
    super::note::record_transition(db, "edge", &prev.id, &prev.inspection_status, status, inspected_by, now)?;
    Ok(true)
}

/// Fix a failing edge: set inspection_status = passing and propagate
/// needs_reverification to currently-passing neighbours.
pub fn fix_edge(
    db: &dyn LoomDb,
    edge_id: &str,
    description: &str,
    fixed_by: &str,
    now: &str,
) -> Result<bool> {
    // Resolve the edge's endpoints by scanning (reliable) — relationship
    // id-property matching is not.
    let edge = match get_relates_to(db, edge_id)? {
        Some(e) => e,
        None => return Ok(false),
    };
    let from_id = edge.from_id.clone();
    let to_id = edge.to_id.clone();
    super::note::record_transition(db, "edge", &edge.id, &edge.inspection_status, "passing", fixed_by, now)?;

    // Mark the edge as passing (fixed), keyed by its endpoints.
    db.execute(&format!(
        "MATCH (a:Intent {{id: '{from}'}})-[r:RELATES_TO]->(b:Intent {{id: '{to}'}}) \
         SET r.inspection_status = 'passing', r.notes = '{desc}', \
             r.last_inspected = '{now}'",
        from = esc(&from_id),
        to   = esc(&to_id),
        desc = esc(description),
        now  = esc(now),
    ))?;

    // Propagate needs_reverification to currently-passing/independent neighbours.
    // Read each endpoint's edges (node-keyed, reliable), filter in Rust, then
    // update each qualifying neighbour by its own endpoints.
    let mut flagged: std::collections::HashSet<String> = std::collections::HashSet::new();
    for node_id in [&from_id, &to_id] {
        for nb in edges_for_intent(db, node_id)? {
            if nb.id == edge_id || !flagged.insert(nb.id.clone()) {
                continue;
            }
            if nb.inspection_status == "passing" || nb.inspection_status == "independent" {
                db.execute(&format!(
                    "MATCH (a:Intent {{id: '{from}'}})-[r:RELATES_TO]->(b:Intent {{id: '{to}'}}) \
                     SET r.inspection_status = 'needs_reverification'",
                    from = esc(&nb.from_id),
                    to   = esc(&nb.to_id),
                ))?;
            }
        }
    }

    Ok(true)
}

/// All RELATES_TO edges touching a given intent (in + out), deduplicated.
pub fn edges_for_intent(db: &dyn LoomDb, intent_id: &str) -> Result<Vec<RelatesTo>> {
    let out_q = format!(
        "MATCH (a:Intent {{id: '{id}'}})-[r:RELATES_TO]->(b:Intent) \
         RETURN r.id, r.inspection_status, r.criterion, r.confidence, r.evidence, \
                r.last_inspected, r.inspected_by, r.priority_score, r.notes, \
                a.id AS from_id, a.name AS from_name, \
                b.id AS to_id, b.name AS to_name",
        id = esc(intent_id)
    );
    let in_q = format!(
        "MATCH (a:Intent)-[r:RELATES_TO]->(b:Intent {{id: '{id}'}}) \
         RETURN r.id, r.inspection_status, r.criterion, r.confidence, r.evidence, \
                r.last_inspected, r.inspected_by, r.priority_score, r.notes, \
                a.id AS from_id, a.name AS from_name, \
                b.id AS to_id, b.name AS to_name",
        id = esc(intent_id)
    );
    let mut edges = Vec::new();
    for q in &[out_q, in_q] {
        let result = db.execute(q)?;
        let cols = col_map(&result);
        for row in result.rows() {
            edges.push(row_to_relates_to(row, &cols));
        }
    }
    let mut seen = std::collections::HashSet::new();
    edges.retain(|e| seen.insert(e.id.clone()));
    Ok(edges)
}

pub fn unresolved_edges_for_intent(db: &dyn LoomDb, intent_id: &str) -> Result<Vec<RelatesTo>> {
    let edges = edges_for_intent(db, intent_id)?;
    Ok(edges.into_iter().filter(|e| {
        matches!(
            e.inspection_status.as_str(),
            "uninspected" | "failing" | "needs_reverification"
        )
    }).collect())
}

/// Recently updated passing edges (were: recent_fixes).
pub fn recent_passing(db: &dyn LoomDb, limit: usize) -> Result<Vec<RelatesTo>> {
    let mut edges = list_relates_to(db, Some("passing"))?;
    edges.sort_by(|a, b| b.last_inspected.cmp(&a.last_inspected));
    edges.truncate(limit);
    Ok(edges)
}

fn row_to_relates_to(row: &[Value], cols: &HashMap<&str, usize>) -> RelatesTo {
    RelatesTo {
        id:                str_val(get(row, cols, "r.id")),
        from_id:           str_val(get(row, cols, "from_id")),
        to_id:             str_val(get(row, cols, "to_id")),
        from_name:         str_val(get(row, cols, "from_name")),
        to_name:           str_val(get(row, cols, "to_name")),
        inspection_status: str_val(get(row, cols, "r.inspection_status")),
        criterion:         str_val(get(row, cols, "r.criterion")),
        confidence:        f64_val(get(row, cols, "r.confidence")),
        evidence:          str_val(get(row, cols, "r.evidence")),
        last_inspected:    str_val(get(row, cols, "r.last_inspected")),
        inspected_by:      str_val(get(row, cols, "r.inspected_by")),
        priority_score:    f64_val(get(row, cols, "r.priority_score")),
        notes:             str_val(get(row, cols, "r.notes")),
    }
}
