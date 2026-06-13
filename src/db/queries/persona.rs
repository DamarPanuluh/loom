//! Persona node queries — CRUD for the audience-segment plane.

use anyhow::Result;
use grafeo::Value;
use std::collections::HashMap;

use crate::db::schema::esc;
use crate::db::LoomDb;
use crate::types::Persona;

use super::row::{col_map, get, str_val};

pub fn insert_persona(db: &dyn LoomDb, p: &Persona) -> Result<()> {
    db.execute(&format!(
        "INSERT (:{label} {{id: '{id}', name: '{name}', description: '{desc}', \
         author: '{author}', created_at: '{created}', updated_at: '{updated}'}})",
        label   = crate::db::schema::label::PERSONA,
        id      = esc(&p.id),
        name    = esc(&p.name),
        desc    = esc(&p.description),
        author  = esc(&p.author),
        created = esc(&p.created_at),
        updated = esc(&p.updated_at),
    ))?;
    Ok(())
}

pub fn get_persona(db: &dyn LoomDb, id_or_name: &str) -> Result<Option<Persona>> {
    // Try exact id first.
    let by_id = db.execute(&format!(
        "MATCH (p:Persona {{id: '{v}'}}) \
         RETURN p.id, p.name, p.description, p.author, p.created_at, p.updated_at",
        v = esc(id_or_name),
    ))?;
    let cols = col_map(&by_id);
    if let Some(row) = by_id.rows().first() {
        return Ok(Some(row_to_persona(row, &cols)));
    }

    // Then exact name.
    let by_name = db.execute(&format!(
        "MATCH (p:Persona {{name: '{v}'}}) \
         RETURN p.id, p.name, p.description, p.author, p.created_at, p.updated_at",
        v = esc(id_or_name),
    ))?;
    let cols = col_map(&by_name);
    if let Some(row) = by_name.rows().first() {
        return Ok(Some(row_to_persona(row, &cols)));
    }

    // Then unique name fragment (case-insensitive substring).
    let all = list_personas(db)?;
    let lower = id_or_name.to_lowercase();
    let mut hits: Vec<Persona> = all
        .into_iter()
        .filter(|p| p.name.to_lowercase().contains(&lower))
        .collect();
    match hits.len() {
        0 => Ok(None),
        1 => Ok(Some(hits.remove(0))),
        _ => anyhow::bail!(
            "Ambiguous persona fragment '{}' matches {} personas — use more of the name or the exact id.\n  {}",
            id_or_name,
            hits.len(),
            hits.iter().map(|p| format!("{} ({})", p.name, p.id)).collect::<Vec<_>>().join("\n  ")
        ),
    }
}

/// Resolve a persona by id/name/fragment; error if not found.
pub fn resolve_persona(db: &dyn LoomDb, id_or_name: &str) -> Result<String> {
    match get_persona(db, id_or_name)? {
        Some(p) => Ok(p.id),
        None => anyhow::bail!(
            "Persona '{}' not found.\n  Run `loom persona list` to see registered personas.",
            id_or_name
        ),
    }
}

pub fn list_personas(db: &dyn LoomDb) -> Result<Vec<Persona>> {
    let r = db.execute(
        "MATCH (p:Persona) \
         RETURN p.id, p.name, p.description, p.author, p.created_at, p.updated_at \
         ORDER BY p.name",
    )?;
    let cols = col_map(&r);
    Ok(r.rows().iter().map(|row| row_to_persona(row, &cols)).collect())
}

fn row_to_persona(row: &[Value], cols: &HashMap<&str, usize>) -> Persona {
    Persona {
        id:          str_val(get(row, cols, "p.id")),
        name:        str_val(get(row, cols, "p.name")),
        description: str_val(get(row, cols, "p.description")),
        author:      str_val(get(row, cols, "p.author")),
        created_at:  str_val(get(row, cols, "p.created_at")),
        updated_at:  str_val(get(row, cols, "p.updated_at")),
    }
}
