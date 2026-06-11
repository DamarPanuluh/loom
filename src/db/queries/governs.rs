//! GOVERNS edge queries (QualityRule → Intent).

use anyhow::Result;
use grafeo::Value;
use std::collections::HashMap;

use crate::db::schema::esc;
use crate::db::LoomDb;
use crate::types::Governs;

use super::row::{col_map, f64_val, get, str_val};

pub fn insert_governs(
    db: &dyn LoomDb,
    rule_id: &str,
    intent_id: &str,
    criterion: &str,
    now: &str,
) -> Result<()> {
    // Default `uninspected`, not `passing`: applying a quality rule asserts it
    // *applies*, not that the intent *complies*. Green must be earned — the
    // quality agent inspects (`loom rule check`) and sets passing/failing.
    // One MERGE trip (verify + idempotent insert); see insert_implements.
    let r = db.execute_with_params(
        "MATCH (r:QualityRule {id: $rid}), (i:Intent {id: $iid}) \
         MERGE (r)-[e:GOVERNS]->(i) \
         ON CREATE SET e.inspection_status = 'uninspected', \
           e.criterion = $crit, e.confidence = 0.0, e.evidence = '', \
           e.last_inspected = '', e.inspected_by = '', e.notes = '', \
           e.created_at = $now \
         RETURN e.inspection_status",
        super::row::sparams(&[
            ("rid", rule_id), ("iid", intent_id), ("crit", criterion), ("now", now),
        ]),
    )?;
    if r.rows().is_empty() {
        let check_rule = db.execute(&format!(
            "MATCH (r:QualityRule {{id: '{}'}}) RETURN r.id", esc(rule_id)
        ))?;
        if check_rule.rows().is_empty() {
            anyhow::bail!("QualityRule '{}' not found — `loom rule list` shows registered rules.", rule_id);
        }
        anyhow::bail!("Intent '{}' not found — `loom intent list`.", intent_id);
    }
    Ok(())
}

/// Resolve a GOVERNS edge by its endpoints (rule → intent), the reliable key:
/// there is at most one GOVERNS edge per (rule, intent) pair.
pub fn get_governs_between(
    db: &dyn LoomDb,
    rule_id: &str,
    intent_id: &str,
) -> Result<Option<Governs>> {
    Ok(list_governs_for_intent(db, intent_id)?
        .into_iter()
        .find(|g| g.rule_id == rule_id))
}

/// Record the quality verdict on a GOVERNS edge: passing (complies) or failing
/// (violates), with the criterion inspected against and the evidence found.
/// This is how GOVERNS green is *earned* — `loom rule apply` only asserts the
/// rule applies. Endpoint-matched SET — the
/// (rule, intent) pair IS the edge's identity (schema v4).
/// Returns false if no GOVERNS edge exists.
#[allow(clippy::too_many_arguments)]
pub fn update_governs_verdict(
    db: &dyn LoomDb,
    rule_id: &str,
    intent_id: &str,
    status: &str,
    criterion: &str,
    evidence: &str,
    confidence: f64,
    inspected_by: &str,
    now: &str,
) -> Result<bool> {
    let Some(prev) = get_governs_between(db, rule_id, intent_id)? else {
        return Ok(false);
    };
    db.execute_with_params(
        &format!(
            "MATCH (r:QualityRule {{id: $rid}})-[e:GOVERNS]->(i:Intent {{id: $iid}}) \
             SET e.inspection_status = $status, e.criterion = $crit, \
                 e.evidence = $ev, e.confidence = {conf}, \
                 e.inspected_by = $by, e.last_inspected = $now",
            conf = confidence,
        ),
        super::row::sparams(&[
            ("rid", rule_id), ("iid", intent_id), ("status", status),
            ("crit", criterion), ("ev", evidence),
            ("by", inspected_by), ("now", now),
        ]),
    )?;
    super::note::record_transition(db, "edge", &prev.id, &prev.inspection_status, status, inspected_by, now)?;
    Ok(true)
}

pub fn list_governs_for_intent(db: &dyn LoomDb, intent_id: &str) -> Result<Vec<Governs>> {
    let q = format!(
        "MATCH (r:QualityRule)-[e:GOVERNS]->(i:Intent {{id: '{id}'}}) \
         RETURN e.inspection_status, e.criterion, e.confidence, e.evidence, \
                e.last_inspected, e.inspected_by, e.notes, \
                r.id AS rule_id, r.name AS rule_name, \
                i.id AS intent_id, i.name AS intent_name",
        id = esc(intent_id)
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    Ok(result.rows().iter().map(|row| row_to_governs(row, &cols)).collect())
}

/// Ripple support for `loom sync`: a quality verdict is a claim about code, so
/// when code implementing an intent changes, its *passing* GOVERNS edges go
/// `needs_reverification` — green must be re-earned. Failing/uninspected edges
/// are left alone (they are already open work). A non-empty `cause` (e.g.
/// "src/db/mod.rs changed") is recorded as a transition note on each flipped
/// edge, so the staleness explains itself. Returns the count flagged.
#[cfg(test)]
pub fn flag_governs_for_intent(
    db: &dyn LoomDb,
    intent_id: &str,
    cause: &str,
    now: &str,
) -> Result<usize> {
    let mut count = 0usize;
    for g in list_governs_for_intent(db, intent_id)? {
        if g.inspection_status == "passing" {
            db.execute(&format!(
                "MATCH (r:QualityRule {{id: '{rid}'}})-[e:GOVERNS]->(i:Intent {{id: '{iid}'}}) \
                 SET e.inspection_status = 'needs_reverification'",
                rid = esc(&g.rule_id),
                iid = esc(intent_id),
            ))?;
            if !cause.is_empty() {
                super::note::record_sync_flip(
                    db, "edge", &g.id, "passing", "needs_reverification", cause, now,
                )?;
            }
            count += 1;
        }
    }
    Ok(count)
}

/// Scan every GOVERNS edge.
pub fn list_all_governs(db: &dyn LoomDb) -> Result<Vec<Governs>> {
    list_governs_filtered(db, None)
}

pub fn list_all_failing_governs(db: &dyn LoomDb) -> Result<Vec<Governs>> {
    list_governs_filtered(db, Some("failing"))
}

/// Status filtering is pushed into the query: edge-property EQUALITY filters
/// are deterministic on grafeo 0.5.42 (verified under stress in
/// tests/grafeo_probe.rs — only the property NAME `id` is broken in filter
/// position). The Rust retain stays as a zero-cost guard: a regression in a
/// future grafeo would shrink results to correct, never silently widen them.
fn list_governs_filtered(db: &dyn LoomDb, status: Option<&str>) -> Result<Vec<Governs>> {
    let where_clause = match status {
        Some(s) => format!("WHERE e.inspection_status = '{}' ", esc(s)),
        None => String::new(),
    };
    let q = format!(
        "MATCH (r:QualityRule)-[e:GOVERNS]->(i:Intent) {where_clause}\
         RETURN e.inspection_status, e.criterion, e.confidence, e.evidence, \
                e.last_inspected, e.inspected_by, e.notes, \
                r.id AS rule_id, r.name AS rule_name, \
                i.id AS intent_id, i.name AS intent_name \
         ORDER BY e.last_inspected DESC"
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    let mut edges: Vec<Governs> =
        result.rows().iter().map(|row| row_to_governs(row, &cols)).collect();
    if let Some(s) = status {
        edges.retain(|g| g.inspection_status == s);
    }
    Ok(edges)
}

fn row_to_governs(row: &[Value], cols: &HashMap<&str, usize>) -> Governs {
    let rule_id = str_val(get(row, cols, "rule_id"));
    let intent_id = str_val(get(row, cols, "intent_id"));
    Governs {
        id:                crate::db::schema::edge_key(crate::db::schema::edge::GOVERNS, &rule_id, &intent_id),
        rule_id,
        intent_id,
        rule_name:         str_val(get(row, cols, "rule_name")),
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
