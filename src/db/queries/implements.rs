//! IMPLEMENTS edge queries (Intent → CodeFile).

use anyhow::Result;
use grafeo::Value;
use std::collections::HashMap;

use crate::db::schema::esc;
use crate::db::LoomDb;
use crate::types::Implements;

use super::row::{col_map, f64_val, get, str_val};

pub fn insert_implements(
    db: &dyn LoomDb,
    intent_id: &str,
    codefile_id: &str,
    locator: &str,
    notes: &str,
    now: &str,
) -> Result<()> {
    // One MERGE does verify + idempotency + insert in a single trip:
    // endpoint-pair uniqueness is the invariant every edge update relies on,
    // and ON CREATE-only means re-grounding the same pair keeps the first
    // grounding (no ON MATCH). A row comes back iff both endpoints exist.
    let r = db.execute_with_params(
        "MATCH (i:Intent {id: $iid}), (cf:CodeFile {id: $cfid}) \
         MERGE (i)-[e:IMPLEMENTS]->(cf) \
         ON CREATE SET e.inspection_status = 'passing', \
           e.criterion = '', e.confidence = 0.0, e.evidence = '', \
           e.last_inspected = '', e.inspected_by = '', e.locator = $locator, \
           e.notes = $notes, e.created_at = $now \
         RETURN e.inspection_status",
        super::row::sparams(&[
            ("iid", intent_id), ("cfid", codefile_id),
            ("locator", locator), ("notes", notes), ("now", now),
        ]),
    )?;
    if r.rows().is_empty() {
        // The miss path pays for the precise teaching error.
        let check_intent = db.execute(&format!(
            "MATCH (i:Intent {{id: '{}'}}) RETURN i.id", esc(intent_id)
        ))?;
        if check_intent.rows().is_empty() {
            anyhow::bail!("Intent '{}' not found — `loom intent list`.", intent_id);
        }
        anyhow::bail!("CodeFile '{}' not found. Add it with `loom codefile add` first.", codefile_id);
    }
    Ok(())
}

pub fn list_implements_for_intent(
    db: &dyn LoomDb,
    intent_id: &str,
) -> Result<Vec<Implements>> {
    let q = format!(
        "MATCH (i:Intent {{id: '{id}'}})-[e:IMPLEMENTS]->(cf:CodeFile) \
         RETURN e.inspection_status, e.criterion, e.confidence, e.evidence, \
                e.last_inspected, e.inspected_by, e.locator, e.notes, e.created_at, \
                i.id AS intent_id, i.name AS intent_name, \
                cf.id AS codefile_id, cf.path AS codefile_path",
        id = esc(intent_id)
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    Ok(result.rows().iter().map(|row| row_to_implements(row, &cols)).collect())
}


/// Remove the IMPLEMENTS edge between an intent and a codefile (endpoint-
/// matched). The ungrounding half of `edge implement` — needed when a
/// grounding moves down to a child intent during decomposition. Returns false
/// if no such edge exists.
pub fn delete_implements(db: &dyn LoomDb, intent_id: &str, codefile_id: &str) -> Result<bool> {
    let existing = db.execute(&format!(
        "MATCH (i:Intent {{id: '{}'}})-[e:IMPLEMENTS]->(cf:CodeFile {{id: '{}'}}) RETURN i.id",
        esc(intent_id), esc(codefile_id)
    ))?;
    if existing.rows().is_empty() {
        return Ok(false);
    }
    db.execute(&format!(
        "MATCH (i:Intent {{id: '{}'}})-[e:IMPLEMENTS]->(cf:CodeFile {{id: '{}'}}) DELETE e",
        esc(intent_id), esc(codefile_id)
    ))?;
    Ok(true)
}

/// Every IMPLEMENTS edge in the graph (node-anchored scan — reliable). Used by
/// the `loom doctor` audit.
pub fn list_all_implements(db: &dyn LoomDb) -> Result<Vec<Implements>> {
    let q = "MATCH (i:Intent)-[e:IMPLEMENTS]->(cf:CodeFile) \
             RETURN e.inspection_status, e.criterion, e.confidence, e.evidence, \
                    e.last_inspected, e.inspected_by, e.locator, e.notes, e.created_at, \
                    i.id AS intent_id, i.name AS intent_name, \
                    cf.id AS codefile_id, cf.path AS codefile_path";
    let result = db.execute(q)?;
    let cols = col_map(&result);
    Ok(result.rows().iter().map(|row| row_to_implements(row, &cols)).collect())
}

/// IDs of every Intent that grounds at least one CodeFile (has ≥1 IMPLEMENTS).
/// Used by the realization check: an implemented leaf intent absent from this
/// set is unrealized. Node-anchored RETURN, so reliable.
pub fn intents_with_implements(db: &dyn LoomDb) -> Result<std::collections::HashSet<String>> {
    let r = db.execute(
        "MATCH (i:Intent)-[e:IMPLEMENTS]->(:CodeFile) RETURN i.id AS iid",
    )?;
    let cols = col_map(&r);
    Ok(r.rows().iter().map(|row| str_val(get(row, &cols, "iid"))).collect())
}

/// Return the IDs of all Intents that IMPLEMENT a given CodeFile.
#[cfg(test)]
pub fn intent_ids_implementing_codefile(
    db: &dyn LoomDb,
    codefile_id: &str,
) -> Result<Vec<String>> {
    let q = format!(
        "MATCH (i:Intent)-[:IMPLEMENTS]->(cf:CodeFile {{id: '{}'}}) \
         RETURN i.id AS iid",
        esc(codefile_id)
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    Ok(result.rows().iter().map(|row| str_val(get(row, &cols, "iid"))).collect())
}

fn row_to_implements(row: &[Value], cols: &HashMap<&str, usize>) -> Implements {
    let intent_id = str_val(get(row, cols, "intent_id"));
    let codefile_id = str_val(get(row, cols, "codefile_id"));
    Implements {
        id:                crate::db::schema::edge_key(crate::db::schema::edge::IMPLEMENTS, &intent_id, &codefile_id),
        intent_id,
        codefile_id,
        intent_name:       str_val(get(row, cols, "intent_name")),
        codefile_path:     str_val(get(row, cols, "codefile_path")),
        inspection_status: str_val(get(row, cols, "e.inspection_status")),
        criterion:         str_val(get(row, cols, "e.criterion")),
        confidence:        f64_val(get(row, cols, "e.confidence")),
        evidence:          str_val(get(row, cols, "e.evidence")),
        last_inspected:    str_val(get(row, cols, "e.last_inspected")),
        inspected_by:      str_val(get(row, cols, "e.inspected_by")),
        locator:           str_val(get(row, cols, "e.locator")),
        notes:             str_val(get(row, cols, "e.notes")),
        created_at:        str_val(get(row, cols, "e.created_at")),
    }
}
