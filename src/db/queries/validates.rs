//! VALIDATES edge queries (Validation → Intent). The Validation *node* lives in
//! `validation.rs`.

use anyhow::Result;

use std::collections::{HashMap, HashSet};
use crate::db::schema::esc;
use crate::db::LoomDb;
use crate::types::{ValidatesEdge, Validation};

#[cfg(test)]
use super::implements::intent_ids_implementing_codefile;
use super::row::{col_map, get, str_val};
use super::validation::get_validation;

pub fn insert_validates(
    db: &dyn LoomDb,
    edge_id: &str,
    validation_id: &str,
    intent_id: &str,
    notes: &str,
    now: &str,
) -> Result<()> {
    let check_v = db.execute(&format!(
        "MATCH (v:Validation {{id: '{}'}}) RETURN v.id", esc(validation_id)
    ))?;
    if check_v.rows().is_empty() {
        anyhow::bail!("Validation '{}' not found", validation_id);
    }
    let check_i = db.execute(&format!(
        "MATCH (i:Intent {{id: '{}'}}) RETURN i.id", esc(intent_id)
    ))?;
    if check_i.rows().is_empty() {
        anyhow::bail!("Intent '{}' not found", intent_id);
    }
    db.execute(&format!(
        "MATCH (v:Validation {{id: '{vid}'}}), (i:Intent {{id: '{iid}'}}) \
         INSERT (v)-[:VALIDATES {{id: '{eid}', inspection_status: 'uninspected', \
           notes: '{notes}', created_at: '{now}'}}]->(i)",
        vid   = esc(validation_id),
        iid   = esc(intent_id),
        eid   = esc(edge_id),
        notes = esc(notes),
        now   = esc(now),
    ))?;
    Ok(())
}

/// Set the per-intent proof verdict (and evidence note) on every VALIDATES edge
/// from this validation. Matched via the validation endpoint node — the reliable
/// pattern — so it's safe to SET the edge property. Returns how many intents the
/// validation links to. Used by `loom validation mark` (manual/async proofs).
pub fn set_validates_status_for_validation(
    db: &dyn LoomDb,
    validation_id: &str,
    status: &str,
    notes: &str,
) -> Result<usize> {
    let count_q = format!(
        "MATCH (v:Validation {{id: '{vid}'}})-[e:VALIDATES]->(i:Intent) RETURN i.id AS x",
        vid = esc(validation_id)
    );
    let n = db.execute(&count_q)?.rows().len();
    db.execute(&format!(
        "MATCH (v:Validation {{id: '{vid}'}})-[e:VALIDATES]->(:Intent) \
         SET e.inspection_status = '{status}', e.notes = '{notes}'",
        vid = esc(validation_id),
        status = esc(status),
        notes = esc(notes),
    ))?;
    Ok(n)
}

/// Return ALL VALIDATES edges in the graph — one query for bulk operations.
/// For per-intent lookups in a loop, prefer this + filter in Rust over
/// calling `list_validates_for_intent` N times.
pub fn list_all_validates(db: &dyn LoomDb) -> Result<Vec<ValidatesEdge>> {
    let q = "MATCH (v:Validation)-[e:VALIDATES]->(i:Intent) \
             RETURN e.id, e.inspection_status, e.notes, \
                    v.id AS validation_id, v.name AS validation_name, \
                    i.id AS intent_id, i.name AS intent_name";
    let result = db.execute(q)?;
    let cols = col_map(&result);
    Ok(result.rows().iter().map(|row| ValidatesEdge {
        id:                str_val(get(row, &cols, "e.id")),
        validation_id:     str_val(get(row, &cols, "validation_id")),
        intent_id:         str_val(get(row, &cols, "intent_id")),
        validation_name:   str_val(get(row, &cols, "validation_name")),
        intent_name:       str_val(get(row, &cols, "intent_name")),
        inspection_status: str_val(get(row, &cols, "e.inspection_status")),
        notes:             str_val(get(row, &cols, "e.notes")),
    }).collect())
}

/// Return all VALIDATES edges pointing to an intent, with Validation details.
pub fn list_validates_for_intent(
    db: &dyn LoomDb,
    intent_id: &str,
) -> Result<Vec<ValidatesEdge>> {
    let q = format!(
        "MATCH (v:Validation)-[e:VALIDATES]->(i:Intent {{id: '{id}'}}) \
         RETURN e.id, e.inspection_status, e.notes, \
                v.id AS validation_id, v.name AS validation_name, \
                i.id AS intent_id, i.name AS intent_name",
        id = esc(intent_id)
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    Ok(result.rows().iter().map(|row| ValidatesEdge {
        id:                str_val(get(row, &cols, "e.id")),
        validation_id:     str_val(get(row, &cols, "validation_id")),
        intent_id:         str_val(get(row, &cols, "intent_id")),
        validation_name:   str_val(get(row, &cols, "validation_name")),
        intent_name:       str_val(get(row, &cols, "intent_name")),
        inspection_status: str_val(get(row, &cols, "e.inspection_status")),
        notes:             str_val(get(row, &cols, "e.notes")),
    }).collect())
}

/// Return full Validation objects for all validations linked to an intent.
pub fn validations_for_intent(db: &dyn LoomDb, intent_id: &str) -> Result<Vec<Validation>> {
    let edges = list_validates_for_intent(db, intent_id)?;
    let mut result = Vec::new();
    for edge in &edges {
        if let Some(v) = get_validation(db, &edge.validation_id)? {
            result.push(v);
        }
    }
    Ok(result)
}

/// Mark all Validations linked to intents that implement a CodeFile as not_run.
/// Returns count of validations invalidated.
#[cfg(test)]
pub fn invalidate_validations_for_codefile(
    db: &dyn LoomDb,
    codefile_id: &str,
) -> Result<usize> {
    let intent_ids = intent_ids_implementing_codefile(db, codefile_id)?;
    invalidate_validations_for_intents(db, &intent_ids)
}

pub fn invalidate_validations_for_intents_with_indexes(
    db: &dyn LoomDb,
    intent_ids: &[String],
    edges_by_intent: &HashMap<&str, Vec<&ValidatesEdge>>,
    val_by_id: &HashMap<&str, &Validation>,
    already_invalidated: &mut HashSet<String>,
) -> Result<usize> {
    let mut count = 0usize;
    for iid in intent_ids {
        let Some(edges) = edges_by_intent.get(iid.as_str()) else {
            continue;
        };
        for edge in edges {
            // Skip already-not_run (nothing to invalidate) and `blocked` — a
            // blocked proof is waiting on something external; a code change
            // doesn't unblock it, and flipping it to not_run would erase the
            // recorded reason it can't run.
            if let Some(v) = val_by_id.get(edge.validation_id.as_str()) {
                if v.last_result != "not_run"
                    && v.last_result != "blocked"
                    && already_invalidated.insert(v.id.clone())
                {
                    db.execute(&format!(
                        "MATCH (v:Validation {{id: '{}'}}) SET v.last_result = 'not_run'",
                        esc(&v.id)
                    ))?;
                    count += 1;
                }
            }
        }
    }
    Ok(count)
}

/// Mark all Validations linked to the given implementing intents as not_run.
/// Callers that already resolved CodeFile → Intent ownership use this to avoid
/// repeating that lookup for every changed file.
#[cfg(test)]
pub fn invalidate_validations_for_intents(
    db: &dyn LoomDb,
    intent_ids: &[String],
) -> Result<usize> {
    let mut count = 0usize;
    for iid in intent_ids {
        let edges = list_validates_for_intent(db, iid)?;
        for edge in &edges {
            // Skip already-not_run (nothing to invalidate) and `blocked` — a
            // blocked proof is waiting on something external; a code change
            // doesn't unblock it, and flipping it to not_run would erase the
            // recorded reason it can't run.
            if let Some(v) = get_validation(db, &edge.validation_id)? {
                if v.last_result != "not_run" && v.last_result != "blocked" {
                    db.execute(&format!(
                        "MATCH (v:Validation {{id: '{}'}}) SET v.last_result = 'not_run'",
                        esc(&v.id)
                    ))?;
                    count += 1;
                }
            }
        }
    }
    Ok(count)
}
