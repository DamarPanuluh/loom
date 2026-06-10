//! Validation node queries (the proof objects). The VALIDATES *edge* lives in
//! `validates.rs`.

use anyhow::Result;
use grafeo::Value;
use std::collections::HashMap;

use crate::db::schema::esc;
use crate::db::LoomDb;
use crate::types::Validation;

use super::row::{col_map, get, str_val};

pub fn insert_validation(db: &dyn LoomDb, v: &Validation) -> Result<()> {
    let q = format!(
        "INSERT (:Validation {{id: '{id}', name: '{name}', description: '{desc}', \
         validation_type: '{vtype}', command: '{cmd}', \
         last_run: '{lrun}', last_result: '{lres}'}})",
        id    = esc(&v.id),
        name  = esc(&v.name),
        desc  = esc(&v.description),
        vtype = esc(&v.validation_type),
        cmd   = esc(&v.command),
        lrun  = esc(&v.last_run),
        lres  = esc(&v.last_result),
    );
    db.execute(&q)?;
    Ok(())
}

pub fn get_validation(db: &dyn LoomDb, id: &str) -> Result<Option<Validation>> {
    let q = format!(
        "MATCH (v:Validation {{id: '{}'}}) \
         RETURN v.id, v.name, v.description, v.validation_type, \
                v.command, v.last_run, v.last_result",
        esc(id)
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    Ok(result.rows().first().map(|row| row_to_validation(row, &cols)))
}

/// Resolve a validation key (exact id, exact name, or unique name fragment) to
/// its id — mirrors `resolve_intent`/`resolve_rule` for consistent UX.
pub fn resolve_validation(db: &dyn LoomDb, key: &str) -> Result<String> {
    let vs = list_validations(db)?;
    if vs.iter().any(|v| v.id == key) {
        return Ok(key.to_string());
    }
    let kl = key.to_lowercase();
    let exact: Vec<_> = vs.iter().filter(|v| v.name.to_lowercase() == kl).collect();
    if exact.len() == 1 {
        return Ok(exact[0].id.clone());
    }
    let subs: Vec<_> = vs.iter().filter(|v| v.name.to_lowercase().contains(&kl)).collect();
    match subs.len() {
        1 => Ok(subs[0].id.clone()),
        0 => anyhow::bail!(
            "No validation matches '{}' (by id, name, or fragment). Run `loom validation list`.",
            key
        ),
        _ => anyhow::bail!(
            "'{}' is ambiguous — matches {} validations. Use the id (`loom validation list`).",
            key, subs.len()
        ),
    }
}

pub fn list_validations(db: &dyn LoomDb) -> Result<Vec<Validation>> {
    let q = "MATCH (v:Validation) \
             RETURN v.id, v.name, v.description, v.validation_type, \
                    v.command, v.last_run, v.last_result \
             ORDER BY v.name";
    let result = db.execute(q)?;
    let cols = col_map(&result);
    Ok(result.rows().iter().map(|row| row_to_validation(row, &cols)).collect())
}

pub fn update_validation_result(
    db: &dyn LoomDb,
    id: &str,
    last_result: &str,
    last_run: &str,
) -> Result<bool> {
    let check = db.execute(&format!(
        "MATCH (v:Validation {{id: '{}'}}) RETURN v.id", esc(id)
    ))?;
    if check.rows().is_empty() {
        return Ok(false);
    }
    db.execute(&format!(
        "MATCH (v:Validation {{id: '{}'}}) \
         SET v.last_result = '{}', v.last_run = '{}'",
        esc(id), esc(last_result), esc(last_run)
    ))?;
    Ok(true)
}

/// Fix a validation's definition — command and/or description. A changed
/// command makes the old result a claim about a *different* proof, so the
/// caller resets last_result/edges (see `commands::validation`). Returns false
/// when the validation doesn't exist.
pub fn update_validation_definition(
    db: &dyn LoomDb,
    id: &str,
    command: Option<&str>,
    description: Option<&str>,
) -> Result<bool> {
    let mut sets = Vec::new();
    if let Some(c) = command {
        sets.push(format!("v.command = '{}'", esc(c)));
    }
    if let Some(d) = description {
        sets.push(format!("v.description = '{}'", esc(d)));
    }
    if sets.is_empty() {
        return Ok(get_validation(db, id)?.is_some());
    }
    let check = db.execute(&format!(
        "MATCH (v:Validation {{id: '{}'}}) RETURN v.id", esc(id)
    ))?;
    if check.rows().is_empty() {
        return Ok(false);
    }
    db.execute(&format!(
        "MATCH (v:Validation {{id: '{}'}}) SET {}",
        esc(id),
        sets.join(", ")
    ))?;
    Ok(true)
}

/// Hard-delete a validation: the node, its VALIDATES edges, and any notes
/// targeting those edges. For removing mistakes (the validation analogue of
/// `intent delete`) — intents whose only proof dies become provably unproven
/// again, which the validate queue surfaces. Returns false if not found.
pub fn delete_validation(db: &dyn LoomDb, id: &str) -> Result<bool> {
    let check = db.execute(&format!(
        "MATCH (v:Validation {{id: '{}'}}) RETURN v.id", esc(id)
    ))?;
    if check.rows().is_empty() {
        return Ok(false);
    }
    // Prune notes targeting the VALIDATES edges that die with the node.
    let edges = db.execute(&format!(
        "MATCH (v:Validation {{id: '{}'}})-[e:VALIDATES]->(:Intent) RETURN e.id AS eid",
        esc(id)
    ))?;
    let cols = super::row::col_map(&edges);
    for row in edges.rows() {
        let eid = super::row::str_val(super::row::get(row, &cols, "eid"));
        if !eid.is_empty() {
            db.execute(&format!(
                "MATCH (note:Note) WHERE note.target_id = '{}' DETACH DELETE note",
                esc(&eid)
            ))?;
        }
    }
    db.execute(&format!(
        "MATCH (v:Validation {{id: '{}'}}) DETACH DELETE v", esc(id)
    ))?;
    Ok(true)
}

fn row_to_validation(row: &[Value], cols: &HashMap<&str, usize>) -> Validation {
    Validation {
        id:              str_val(get(row, cols, "v.id")),
        name:            str_val(get(row, cols, "v.name")),
        description:     str_val(get(row, cols, "v.description")),
        validation_type: str_val(get(row, cols, "v.validation_type")),
        command:         str_val(get(row, cols, "v.command")),
        last_run:        str_val(get(row, cols, "v.last_run")),
        last_result:     str_val(get(row, cols, "v.last_result")),
    }
}
