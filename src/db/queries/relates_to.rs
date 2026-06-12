//! RELATES_TO edge queries — the main intent↔intent grid.
//!
//! Reliability rule (revised after the grafeo 0.5.42 probes —
//! tests/grafeo_probe.rs): edge-property filters are deterministic EXCEPT the
//! property NAME `id`, which in filter position resolves to grafeo's internal
//! edge id instead of the user property (`WHERE r.id = X` matches nothing,
//! ever). So: status filters may live in the query; edge-ID lookups must match
//! by endpoint nodes (a RELATES_TO edge is unique per ordered pair) or scan
//! all and filter in Rust. See the project memory `grafeo-relationship-matching`.

use anyhow::Result;
use grafeo::Value;
use std::collections::HashMap;

use crate::db::schema::esc;
use crate::db::LoomDb;
use crate::types::RelatesTo;

use super::row::{col_map, f64_val, get, str_val};

pub fn get_relates_to(db: &dyn LoomDb, id: &str) -> Result<Option<RelatesTo>> {
    // Resolve by scanning all edges and matching the id in Rust. `WHERE r.id`
    // is the one edge-property filter grafeo gets wrong (the name `id` resolves
    // to the INTERNAL edge id in filter position — matches nothing, ever).
    // Prefer get_relates_to_between when the endpoints are known — it is a
    // direct node-keyed lookup.
    Ok(list_relates_to(db, None)?.into_iter().find(|e| e.id == id))
}

pub fn get_relates_to_between(
    db: &dyn LoomDb,
    from_id: &str,
    to_id: &str,
) -> Result<Option<RelatesTo>> {
    let q = format!(
        "MATCH (a:Intent {{id: '{from}'}})-[r:RELATES_TO]->(b:Intent {{id: '{to}'}}) \
         RETURN r.inspection_status, r.criterion, r.confidence, r.evidence, \
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
    from_id: &str,
    to_id: &str,
    now: &str,
) -> Result<RelatesTo> {
    // One MERGE does the whole get-or-create: match by endpoints (RELATES_TO
    // is unique per ordered pair), create with defaults if absent, and RETURN
    // the edge's actual properties either way — both paths verified on grafeo
    // 0.5.42 (tests/grafeo_probe.rs, merge+return / merge-create+return).
    // Replaces the old three-trip exists-check → verify → INSERT +
    // construct-in-Rust dance.
    let q = format!(
        "MATCH (a:Intent {{id: '{from}'}}), (b:Intent {{id: '{to}'}}) \
         MERGE (a)-[r:RELATES_TO]->(b) \
         ON CREATE SET r.inspection_status = 'uninspected', \
           r.criterion = '', r.confidence = 0.0, r.evidence = '', \
           r.last_inspected = '', r.inspected_by = '', r.priority_score = 0.0, \
           r.notes = '', r.created_at = '{now}' \
         RETURN r.inspection_status, r.criterion, r.confidence, r.evidence, \
                r.last_inspected, r.inspected_by, r.priority_score, r.notes, \
                a.id AS from_id, a.name AS from_name, \
                b.id AS to_id, b.name AS to_name",
        from = esc(from_id),
        to   = esc(to_id),
        now  = esc(now),
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    match result.rows().first() {
        Some(row) => Ok(row_to_relates_to(row, &cols)),
        // MERGE returned nothing ⇒ the MATCH found no endpoints.
        None => anyhow::bail!(
            "Cannot create edge: one or both intents not found.\n\
             intent-a id: {}\n\
             intent-b id: {}\n\
             Run `loom intent list` to see available intents.",
            from_id, to_id
        ),
    }
}

pub fn list_relates_to(
    db: &dyn LoomDb,
    status_filter: Option<&str>,
) -> Result<Vec<RelatesTo>> {
    // The status filter is pushed into the query: edge-property EQUALITY
    // filtering is deterministic on grafeo 0.5.42 (50/50 set-then-filter
    // cycles, in-memory and persistent — tests/grafeo_probe.rs). Only the
    // property NAME `id` is broken in filter position (it resolves to the
    // internal edge id there), so edge-id lookups still scan. The Rust
    // retain stays as a zero-cost guard: if a grafeo upgrade ever regresses
    // the pushdown, results shrink to correct instead of silently widening.
    let where_clause = match status_filter {
        Some(s) => format!("WHERE r.inspection_status = '{}' ", esc(s)),
        None => String::new(),
    };
    let q = format!(
        "MATCH (a:Intent)-[r:RELATES_TO]->(b:Intent) {where_clause}\
         RETURN r.inspection_status, r.criterion, r.confidence, r.evidence, \
                r.last_inspected, r.inspected_by, r.priority_score, r.notes, \
                a.id AS from_id, a.name AS from_name, \
                b.id AS to_id, b.name AS to_name \
         ORDER BY r.priority_score DESC"
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    let mut edges: Vec<RelatesTo> =
        result.rows().iter().map(|row| row_to_relates_to(row, &cols)).collect();
    if let Some(s) = status_filter {
        edges.retain(|e| e.inspection_status == s);
    }
    Ok(edges)
}

// The RELATES_TO update helpers below identify the edge by its endpoint node
// ids: since schema v4 the ordered (from, to) pair IS the edge's identity
// (at most one RELATES_TO per pair, enforced by get_or_create_relates_to;
// the derived `rt:<from>:<to>` key is just this pair spelled out).

/// Set inspection_status = passing (was: grounded) with meta. `evidence` is
/// what the inspection actually found (optional for ground — "" is honest
/// when the criterion says it all); it is ALWAYS written, so a re-ground
/// never leaves a previous failing verdict's evidence behind the new green.
pub fn update_relates_to_ground(
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
    db.execute_with_params(
        &format!(
            "MATCH (a:Intent {{id: $from}})-[r:RELATES_TO]->(b:Intent {{id: $to}}) \
             SET r.inspection_status = 'passing', r.criterion = $crit, \
                 r.evidence = $ev, r.confidence = {conf}, r.inspected_by = $by, \
                 r.last_inspected = $now",
            conf = confidence,
        ),
        super::row::sparams(&[
            ("from", from_id), ("to", to_id), ("crit", criterion),
            ("ev", evidence), ("by", inspected_by), ("now", now),
        ]),
    )?;
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
    db.execute_with_params(
        &format!(
            "MATCH (a:Intent {{id: $from}})-[r:RELATES_TO]->(b:Intent {{id: $to}}) \
             SET r.inspection_status = 'failing', r.criterion = $crit, \
                 r.evidence = $ev, r.confidence = {conf}, r.inspected_by = $by, \
                 r.last_inspected = $now",
            conf = confidence,
        ),
        super::row::sparams(&[
            ("from", from_id), ("to", to_id), ("crit", criterion),
            ("ev", evidence), ("by", inspected_by), ("now", now),
        ]),
    )?;
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
    db.execute_with_params(
        "MATCH (a:Intent {id: $from})-[r:RELATES_TO]->(b:Intent {id: $to}) \
         SET r.inspection_status = 'independent', r.notes = $notes, \
             r.inspected_by = $by, r.last_inspected = $now",
        super::row::sparams(&[
            ("from", from_id), ("to", to_id), ("notes", notes),
            ("by", inspected_by), ("now", now),
        ]),
    )?;
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
    db.execute_with_params(
        &format!(
            "MATCH (a:Intent {{id: $from}})-[r:RELATES_TO]->(b:Intent {{id: $to}}) \
             SET r.inspection_status = $status, r.criterion = $crit, \
                 r.evidence = $ev, r.confidence = {conf}, r.inspected_by = $by, \
                 r.last_inspected = $now",
            conf = confidence,
        ),
        super::row::sparams(&[
            ("from", from_id), ("to", to_id), ("status", status),
            ("crit", criterion), ("ev", evidence),
            ("by", inspected_by), ("now", now),
        ]),
    )?;
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
         RETURN r.inspection_status, r.criterion, r.confidence, r.evidence, \
                r.last_inspected, r.inspected_by, r.priority_score, r.notes, \
                a.id AS from_id, a.name AS from_name, \
                b.id AS to_id, b.name AS to_name",
        id = esc(intent_id)
    );
    let in_q = format!(
        "MATCH (a:Intent)-[r:RELATES_TO]->(b:Intent {{id: '{id}'}}) \
         RETURN r.inspection_status, r.criterion, r.confidence, r.evidence, \
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
    let from_id = str_val(get(row, cols, "from_id"));
    let to_id = str_val(get(row, cols, "to_id"));
    RelatesTo {
        // v4: edge identity is DERIVED from the endpoints (unique per ordered
        // pair) — nothing is stored, nothing can go stale, and the key is the
        // same on every machine the graph travels to.
        id:                crate::db::schema::edge_key(crate::db::schema::edge::RELATES_TO, &from_id, &to_id),
        from_id,
        to_id,
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
