//! TARGETS edge queries (Hypothesis → Intent) — which intents an improvement
//! hypothesis would touch. Mirrors `governs.rs`: endpoint-matched, full
//! inspectable meta, default uninspected.

use anyhow::Result;
use grafeo::Value;
use std::collections::HashMap;

use crate::db::schema::esc;
use crate::db::LoomDb;
use crate::types::TargetsEdge;

use super::row::{col_map, f64_val, get, str_val};

pub fn insert_targets(
    db: &dyn LoomDb,
    hypothesis_id: &str,
    intent_id: &str,
    now: &str,
) -> Result<()> {
    // One MERGE trip (verify + idempotent insert); see insert_implements.
    let r = db.execute_with_params(
        "MATCH (h:Hypothesis {id: $hid}), (i:Intent {id: $iid}) \
         MERGE (h)-[e:TARGETS]->(i) \
         ON CREATE SET e.inspection_status = 'uninspected', \
           e.criterion = '', e.confidence = 0.0, e.evidence = '', \
           e.last_inspected = '', e.inspected_by = '', e.notes = '', \
           e.created_at = $now \
         RETURN e.inspection_status",
        super::row::sparams(&[
            ("hid", hypothesis_id), ("iid", intent_id), ("now", now),
        ]),
    )?;
    if r.rows().is_empty() {
        let check_h = db.execute(&format!(
            "MATCH (h:Hypothesis {{id: '{}'}}) RETURN h.id", esc(hypothesis_id)
        ))?;
        if check_h.rows().is_empty() {
            anyhow::bail!("Hypothesis '{}' not found — `loom hypothesis list`.", hypothesis_id);
        }
        anyhow::bail!("Intent '{}' not found — `loom intent list`.", intent_id);
    }
    Ok(())
}

/// Resolve a TARGETS edge by its endpoints — the reliable key (at most one
/// TARGETS edge per (hypothesis, intent) pair).
pub fn get_targets_between(
    db: &dyn LoomDb,
    hypothesis_id: &str,
    intent_id: &str,
) -> Result<Option<TargetsEdge>> {
    Ok(list_targets_for_hypothesis(db, hypothesis_id)?
        .into_iter()
        .find(|t| t.intent_id == intent_id))
}

pub fn list_targets_for_hypothesis(
    db: &dyn LoomDb,
    hypothesis_id: &str,
) -> Result<Vec<TargetsEdge>> {
    let q = format!(
        "MATCH (h:Hypothesis {{id: '{id}'}})-[e:TARGETS]->(i:Intent) \
         RETURN e.inspection_status, e.criterion, e.confidence, e.evidence, \
                e.last_inspected, e.inspected_by, e.notes, \
                h.id AS hypothesis_id, h.name AS hypothesis_name, \
                i.id AS intent_id, i.name AS intent_name",
        id = esc(hypothesis_id)
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    Ok(result.rows().iter().map(|row| row_to_targets(row, &cols)).collect())
}

/// Scan every TARGETS edge (doctor audit + sync ripple index).
pub fn list_all_targets(db: &dyn LoomDb) -> Result<Vec<TargetsEdge>> {
    let q = "MATCH (h:Hypothesis)-[e:TARGETS]->(i:Intent) \
             RETURN e.inspection_status, e.criterion, e.confidence, e.evidence, \
                    e.last_inspected, e.inspected_by, e.notes, \
                    h.id AS hypothesis_id, h.name AS hypothesis_name, \
                    i.id AS intent_id, i.name AS intent_name";
    let result = db.execute(q)?;
    let cols = col_map(&result);
    Ok(result.rows().iter().map(|row| row_to_targets(row, &cols)).collect())
}

pub fn set_targets_status_for_hypothesis(
    db: &dyn LoomDb,
    hypothesis_id: &str,
    status: &str,
    criterion: &str,
    evidence: &str,
    inspected_by: &str,
    now: &str,
) -> Result<usize> {
    let mut count = 0usize;
    for t in list_targets_for_hypothesis(db, hypothesis_id)? {
        db.execute(&format!(
            "MATCH (h:Hypothesis {{id: '{hid}'}})-[e:TARGETS]->(i:Intent {{id: '{iid}'}}) \
             SET e.inspection_status = '{status}', e.criterion = '{criterion}', \
                 e.confidence = 0.9, e.evidence = '{evidence}', \
                 e.last_inspected = '{now}', e.inspected_by = '{by}'",
            hid = esc(hypothesis_id),
            iid = esc(&t.intent_id),
            status = esc(status),
            criterion = esc(criterion),
            evidence = esc(evidence),
            now = esc(now),
            by = esc(inspected_by),
        ))?;
        count += 1;
    }
    Ok(count)
}

/// Ripple support for `loom sync`: a TARGETS edge is evidence that a
/// hypothesis was checked against the old code of its target intent. When that
/// target's grounded code changes, a previously earned passing edge must be
/// re-inspected.
#[cfg(test)]
pub fn flag_targets_for_intent(
    db: &dyn LoomDb,
    intent_id: &str,
    cause: &str,
    now: &str,
) -> Result<usize> {
    let mut count = 0usize;
    for t in list_all_targets(db)? {
        if t.intent_id == intent_id && t.inspection_status == "passing" {
            db.execute(&format!(
                "MATCH (h:Hypothesis {{id: '{hid}'}})-[e:TARGETS]->(i:Intent {{id: '{iid}'}}) \
                 SET e.inspection_status = 'needs_reverification', e.notes = '{notes}'",
                hid = esc(&t.hypothesis_id),
                iid = esc(intent_id),
                notes = esc(&format!("stale: {cause}")),
            ))?;
            if !cause.is_empty() {
                super::note::record_sync_flip(
                    db, "edge", &t.id, "passing", "needs_reverification", cause, now,
                )?;
            }
            count += 1;
        }
    }
    Ok(count)
}

fn row_to_targets(row: &[Value], cols: &HashMap<&str, usize>) -> TargetsEdge {
    let hypothesis_id = str_val(get(row, cols, "hypothesis_id"));
    let intent_id = str_val(get(row, cols, "intent_id"));
    TargetsEdge {
        id:                crate::db::schema::edge_key(crate::db::schema::edge::TARGETS, &hypothesis_id, &intent_id),
        hypothesis_id,
        intent_id,
        hypothesis_name:   str_val(get(row, cols, "hypothesis_name")),
        intent_name:       str_val(get(row, cols, "intent_name")),
        inspection_status: str_val(get(row, cols, "e.inspection_status")),
        criterion:         str_val(get(row, cols, "e.criterion")),
        confidence:        f64_val(get(row, cols, "e.confidence")),
        evidence:          str_val(get(row, cols, "e.evidence")),
        last_inspected:    str_val(get(row, cols, "e.last_inspected")),
        inspected_by:      str_val(get(row, cols, "e.inspected_by")),
        notes:             str_val(get(row, cols, "e.notes")),
    }
}
