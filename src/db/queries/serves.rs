//! SERVES edge queries — Persona → Intent, fully inspectable.
//!
//! SERVES is a claim: "this intent actually serves this persona." Like
//! RELATES_TO it must be verified against code, not assumed from the
//! declaration. Sync propagates needs_reverification when the intent's code
//! changes (same one-hop rule as RELATES_TO/GOVERNS).

use anyhow::Result;
use grafeo::Value;
use std::collections::HashMap;

use crate::db::schema::{edge, esc};
use crate::db::LoomDb;
use crate::types::ServesEdge;

use super::row::{col_map, f64_val, get, str_val};
use super::note::record_transition;

pub fn get_or_create_serves(
    db: &dyn LoomDb,
    persona_id: &str,
    intent_id: &str,
    now: &str,
) -> Result<ServesEdge> {
    let q = format!(
        "MATCH (p:Persona {{id: '{pid}'}}), (i:Intent {{id: '{iid}'}}) \
         MERGE (p)-[r:SERVES]->(i) \
         ON CREATE SET r.inspection_status = 'uninspected', \
           r.criterion = '', r.confidence = 0.0, r.evidence = '', \
           r.last_inspected = '', r.inspected_by = '', r.priority_score = 0.0, \
           r.notes = '', r.created_at = '{now}' \
         RETURN r.inspection_status, r.criterion, r.confidence, r.evidence, \
                r.last_inspected, r.inspected_by, r.priority_score, r.notes, r.created_at, \
                p.id AS persona_id, p.name AS persona_name, \
                i.id AS intent_id, i.name AS intent_name",
        pid = esc(persona_id),
        iid = esc(intent_id),
        now = esc(now),
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    match result.rows().first() {
        Some(row) => Ok(row_to_serves(row, &cols)),
        None => anyhow::bail!(
            "Cannot create SERVES edge: persona or intent not found.\n\
             persona id: {}\n\
             intent id: {}\n\
             Run `loom persona list` and `loom intent list` to see available nodes.",
            persona_id, intent_id
        ),
    }
}

pub fn get_serves_between(
    db: &dyn LoomDb,
    persona_id: &str,
    intent_id: &str,
) -> Result<Option<ServesEdge>> {
    let q = format!(
        "MATCH (p:Persona {{id: '{pid}'}})-[r:SERVES]->(i:Intent {{id: '{iid}'}}) \
         RETURN r.inspection_status, r.criterion, r.confidence, r.evidence, \
                r.last_inspected, r.inspected_by, r.priority_score, r.notes, r.created_at, \
                p.id AS persona_id, p.name AS persona_name, \
                i.id AS intent_id, i.name AS intent_name",
        pid = esc(persona_id),
        iid = esc(intent_id),
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    Ok(result.rows().first().map(|row| row_to_serves(row, &cols)))
}

pub fn list_serves(
    db: &dyn LoomDb,
    status_filter: Option<&str>,
) -> Result<Vec<ServesEdge>> {
    let where_clause = match status_filter {
        Some(s) => format!("WHERE r.inspection_status = '{}' ", esc(s)),
        None => String::new(),
    };
    let q = format!(
        "MATCH (p:Persona)-[r:SERVES]->(i:Intent) {where_clause}\
         RETURN r.inspection_status, r.criterion, r.confidence, r.evidence, \
                r.last_inspected, r.inspected_by, r.priority_score, r.notes, r.created_at, \
                p.id AS persona_id, p.name AS persona_name, \
                i.id AS intent_id, i.name AS intent_name \
         ORDER BY r.priority_score DESC"
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    let mut edges: Vec<ServesEdge> =
        result.rows().iter().map(|row| row_to_serves(row, &cols)).collect();
    if let Some(s) = status_filter {
        edges.retain(|e| e.inspection_status == s);
    }
    Ok(edges)
}

pub fn list_all_serves(db: &dyn LoomDb) -> Result<Vec<ServesEdge>> {
    list_serves(db, None)
}

pub fn list_serves_for_persona(db: &dyn LoomDb, persona_id: &str) -> Result<Vec<ServesEdge>> {
    let q = format!(
        "MATCH (p:Persona {{id: '{pid}'}})-[r:SERVES]->(i:Intent) \
         RETURN r.inspection_status, r.criterion, r.confidence, r.evidence, \
                r.last_inspected, r.inspected_by, r.priority_score, r.notes, r.created_at, \
                p.id AS persona_id, p.name AS persona_name, \
                i.id AS intent_id, i.name AS intent_name",
        pid = esc(persona_id),
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    Ok(result.rows().iter().map(|row| row_to_serves(row, &cols)).collect())
}

pub fn list_serves_for_intent(db: &dyn LoomDb, intent_id: &str) -> Result<Vec<ServesEdge>> {
    let q = format!(
        "MATCH (p:Persona)-[r:SERVES]->(i:Intent {{id: '{iid}'}}) \
         RETURN r.inspection_status, r.criterion, r.confidence, r.evidence, \
                r.last_inspected, r.inspected_by, r.priority_score, r.notes, r.created_at, \
                p.id AS persona_id, p.name AS persona_name, \
                i.id AS intent_id, i.name AS intent_name",
        iid = esc(intent_id),
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    Ok(result.rows().iter().map(|row| row_to_serves(row, &cols)).collect())
}

pub fn update_serves_ground(
    db: &dyn LoomDb,
    persona_id: &str,
    intent_id: &str,
    criterion: &str,
    evidence: &str,
    confidence: f64,
    inspected_by: &str,
    now: &str,
) -> Result<bool> {
    let Some(prev) = get_serves_between(db, persona_id, intent_id)? else {
        return Ok(false);
    };
    db.execute_with_params(
        &format!(
            "MATCH (p:Persona {{id: $pid}})-[r:SERVES]->(i:Intent {{id: $iid}}) \
             SET r.inspection_status = 'passing', r.criterion = $crit, \
                 r.evidence = $ev, r.confidence = {conf}, r.inspected_by = $by, \
                 r.last_inspected = $now",
            conf = confidence,
        ),
        super::row::sparams(&[
            ("pid", persona_id), ("iid", intent_id), ("crit", criterion),
            ("ev", evidence), ("by", inspected_by), ("now", now),
        ]),
    )?;
    record_transition(db, "edge", &prev.id, &prev.inspection_status, "passing", inspected_by, now)?;
    Ok(true)
}

pub fn update_serves_issue(
    db: &dyn LoomDb,
    persona_id: &str,
    intent_id: &str,
    criterion: &str,
    evidence: &str,
    confidence: f64,
    inspected_by: &str,
    now: &str,
) -> Result<bool> {
    let Some(prev) = get_serves_between(db, persona_id, intent_id)? else {
        return Ok(false);
    };
    db.execute_with_params(
        &format!(
            "MATCH (p:Persona {{id: $pid}})-[r:SERVES]->(i:Intent {{id: $iid}}) \
             SET r.inspection_status = 'failing', r.criterion = $crit, \
                 r.evidence = $ev, r.confidence = {conf}, r.inspected_by = $by, \
                 r.last_inspected = $now",
            conf = confidence,
        ),
        super::row::sparams(&[
            ("pid", persona_id), ("iid", intent_id), ("crit", criterion),
            ("ev", evidence), ("by", inspected_by), ("now", now),
        ]),
    )?;
    record_transition(db, "edge", &prev.id, &prev.inspection_status, "failing", inspected_by, now)?;
    Ok(true)
}

pub fn update_serves_independent(
    db: &dyn LoomDb,
    persona_id: &str,
    intent_id: &str,
    notes: &str,
    inspected_by: &str,
    now: &str,
) -> Result<bool> {
    let Some(prev) = get_serves_between(db, persona_id, intent_id)? else {
        return Ok(false);
    };
    db.execute_with_params(
        "MATCH (p:Persona {id: $pid})-[r:SERVES]->(i:Intent {id: $iid}) \
         SET r.inspection_status = 'independent', r.notes = $notes, \
             r.inspected_by = $by, r.last_inspected = $now",
        super::row::sparams(&[
            ("pid", persona_id), ("iid", intent_id), ("notes", notes),
            ("by", inspected_by), ("now", now),
        ]),
    )?;
    record_transition(db, "edge", &prev.id, &prev.inspection_status, "independent", inspected_by, now)?;
    Ok(true)
}

/// Flip passing SERVES edges for a given intent to needs_reverification.
/// Called from sync when the intent's code changes.
/// Returns the count of edges flipped.
pub fn flag_serves_for_intent(
    db: &dyn LoomDb,
    intent_id: &str,
    cause: &str,
    now: &str,
    serves_by_intent: &HashMap<&str, Vec<&ServesEdge>>,
    already_flagged: &mut std::collections::HashSet<String>,
) -> Result<usize> {
    let mut count = 0usize;
    let Some(edges) = serves_by_intent.get(intent_id) else {
        return Ok(0);
    };
    for edge in edges {
        if (edge.inspection_status == "passing" || edge.inspection_status == "independent")
            && already_flagged.insert(edge.id.clone())
        {
            db.execute(&format!(
                "MATCH (p:Persona {{id: '{pid}'}})-[r:SERVES]->(i:Intent {{id: '{iid}'}}) \
                 SET r.inspection_status = 'needs_reverification'",
                pid = esc(&edge.persona_id),
                iid = esc(&edge.intent_id),
            ))?;
            record_transition(
                db, "edge", &edge.id, &edge.inspection_status,
                "needs_reverification", cause, now,
            )?;
            count += 1;
        }
    }
    Ok(count)
}

fn row_to_serves(row: &[Value], cols: &HashMap<&str, usize>) -> ServesEdge {
    let persona_id = str_val(get(row, cols, "persona_id"));
    let intent_id  = str_val(get(row, cols, "intent_id"));
    ServesEdge {
        id: crate::db::schema::edge_key(edge::SERVES, &persona_id, &intent_id),
        persona_id,
        intent_id,
        persona_name:      str_val(get(row, cols, "persona_name")),
        intent_name:       str_val(get(row, cols, "intent_name")),
        inspection_status: str_val(get(row, cols, "r.inspection_status")),
        criterion:         str_val(get(row, cols, "r.criterion")),
        confidence:        f64_val(get(row, cols, "r.confidence")),
        evidence:          str_val(get(row, cols, "r.evidence")),
        last_inspected:    str_val(get(row, cols, "r.last_inspected")),
        inspected_by:      str_val(get(row, cols, "r.inspected_by")),
        priority_score:    f64_val(get(row, cols, "r.priority_score")),
        notes:             str_val(get(row, cols, "r.notes")),
        created_at:        str_val(get(row, cols, "r.created_at")),
    }
}
