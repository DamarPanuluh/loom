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
    edge_id: &str,
    intent_id: &str,
    codefile_id: &str,
    locator: &str,
    notes: &str,
    now: &str,
) -> Result<()> {
    // Verify both nodes exist
    let check_intent = db.execute(&format!(
        "MATCH (i:Intent {{id: '{}'}}) RETURN i.id", esc(intent_id)
    ))?;
    if check_intent.rows().is_empty() {
        anyhow::bail!("Intent '{}' not found", intent_id);
    }
    let check_cf = db.execute(&format!(
        "MATCH (cf:CodeFile {{id: '{}'}}) RETURN cf.id", esc(codefile_id)
    ))?;
    if check_cf.rows().is_empty() {
        anyhow::bail!("CodeFile '{}' not found. Add it with `loom codefile add` first.", codefile_id);
    }
    // Endpoint-pair uniqueness is the invariant every edge update relies on
    // (edges are matched by endpoints, never by their own id) — enforce it at
    // insert: re-grounding the same pair is a no-op, like get_or_create.
    let existing = db.execute(&format!(
        "MATCH (i:Intent {{id: '{}'}})-[e:IMPLEMENTS]->(cf:CodeFile {{id: '{}'}}) RETURN i.id",
        esc(intent_id), esc(codefile_id)
    ))?;
    if !existing.rows().is_empty() {
        return Ok(());
    }
    db.execute(&format!(
        "MATCH (i:Intent {{id: '{iid}'}}), (cf:CodeFile {{id: '{cfid}'}}) \
         INSERT (i)-[:IMPLEMENTS {{id: '{eid}', inspection_status: 'passing', \
           criterion: '', confidence: 0.0, evidence: '', last_inspected: '', \
           inspected_by: '', locator: '{locator}', notes: '{notes}', created_at: '{now}'}}]->(cf)",
        iid     = esc(intent_id),
        cfid    = esc(codefile_id),
        eid     = esc(edge_id),
        locator = esc(locator),
        notes   = esc(notes),
        now     = esc(now),
    ))?;
    Ok(())
}

pub fn list_implements_for_intent(
    db: &dyn LoomDb,
    intent_id: &str,
) -> Result<Vec<Implements>> {
    let q = format!(
        "MATCH (i:Intent {{id: '{id}'}})-[e:IMPLEMENTS]->(cf:CodeFile) \
         RETURN e.id, e.inspection_status, e.criterion, e.confidence, e.evidence, \
                e.last_inspected, e.inspected_by, e.locator, e.notes, \
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
             RETURN e.id, e.inspection_status, e.criterion, e.confidence, e.evidence, \
                    e.last_inspected, e.inspected_by, e.locator, e.notes, \
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
    Implements {
        id:                str_val(get(row, cols, "e.id")),
        intent_id:         str_val(get(row, cols, "intent_id")),
        codefile_id:       str_val(get(row, cols, "codefile_id")),
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
    }
}
