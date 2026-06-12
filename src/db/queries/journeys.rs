//! JOURNEYS edge queries — Persona → Validation (saga), structural.
//!
//! JOURNEYS is a structural binding (like HIERARCHY): no inspection state.
//! Its value is enabling persona-scoped journey coverage in `loom smells`
//! (unjourneyed_surface becomes persona-aware) and `loom persona show`.

use anyhow::Result;
use grafeo::Value;
use std::collections::HashMap;

use crate::db::schema::{edge, esc};
use crate::db::LoomDb;
use crate::types::JourneysEdge;

use super::row::{col_map, get, str_val};

pub fn get_or_create_journeys(
    db: &dyn LoomDb,
    persona_id: &str,
    validation_id: &str,
    now: &str,
) -> Result<JourneysEdge> {
    let q = format!(
        "MATCH (p:Persona {{id: '{pid}'}}), (v:Validation {{id: '{vid}'}}) \
         MERGE (p)-[r:JOURNEYS]->(v) \
         ON CREATE SET r.notes = '', r.created_at = '{now}' \
         RETURN r.notes, r.created_at, \
                p.id AS persona_id, p.name AS persona_name, \
                v.id AS validation_id, v.name AS validation_name",
        pid = esc(persona_id),
        vid = esc(validation_id),
        now = esc(now),
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    match result.rows().first() {
        Some(row) => Ok(row_to_journeys(row, &cols)),
        None => anyhow::bail!(
            "Cannot create JOURNEYS edge: persona or validation not found.\n\
             persona id: {}\n\
             validation id: {}\n\
             Run `loom persona list` and `loom validation list` to see available nodes.",
            persona_id, validation_id
        ),
    }
}

pub fn list_journeys_for_persona(db: &dyn LoomDb, persona_id: &str) -> Result<Vec<JourneysEdge>> {
    let q = format!(
        "MATCH (p:Persona {{id: '{pid}'}})-[r:JOURNEYS]->(v:Validation) \
         RETURN r.notes, r.created_at, \
                p.id AS persona_id, p.name AS persona_name, \
                v.id AS validation_id, v.name AS validation_name",
        pid = esc(persona_id),
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    Ok(result.rows().iter().map(|row| row_to_journeys(row, &cols)).collect())
}

pub fn list_journeys_for_validation(db: &dyn LoomDb, validation_id: &str) -> Result<Vec<JourneysEdge>> {
    let q = format!(
        "MATCH (p:Persona)-[r:JOURNEYS]->(v:Validation {{id: '{vid}'}}) \
         RETURN r.notes, r.created_at, \
                p.id AS persona_id, p.name AS persona_name, \
                v.id AS validation_id, v.name AS validation_name",
        vid = esc(validation_id),
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    Ok(result.rows().iter().map(|row| row_to_journeys(row, &cols)).collect())
}

pub fn list_all_journeys(db: &dyn LoomDb) -> Result<Vec<JourneysEdge>> {
    let q = "MATCH (p:Persona)-[r:JOURNEYS]->(v:Validation) \
             RETURN r.notes, r.created_at, \
                    p.id AS persona_id, p.name AS persona_name, \
                    v.id AS validation_id, v.name AS validation_name";
    let result = db.execute(q)?;
    let cols = col_map(&result);
    Ok(result.rows().iter().map(|row| row_to_journeys(row, &cols)).collect())
}

fn row_to_journeys(row: &[Value], cols: &HashMap<&str, usize>) -> JourneysEdge {
    let persona_id     = str_val(get(row, cols, "persona_id"));
    let validation_id  = str_val(get(row, cols, "validation_id"));
    JourneysEdge {
        id: crate::db::schema::edge_key(edge::JOURNEYS, &persona_id, &validation_id),
        persona_id,
        validation_id,
        persona_name:     str_val(get(row, cols, "persona_name")),
        validation_name:  str_val(get(row, cols, "validation_name")),
        notes:            str_val(get(row, cols, "r.notes")),
        created_at:       str_val(get(row, cols, "r.created_at")),
    }
}
