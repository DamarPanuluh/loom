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
