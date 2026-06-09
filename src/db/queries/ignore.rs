//! Coverage exclusion patterns — the escape hatch, stored in the graph as
//! recorded decisions (pattern + reason), never a flat .loomignore file.

use anyhow::Result;
use grafeo::Value;
use std::collections::HashMap;

use crate::db::schema::{esc, label, prop};
use crate::db::LoomDb;
use crate::types::Ignore;

use super::row::{col_map, get, str_val};

pub fn insert_ignore(db: &dyn LoomDb, ig: &Ignore) -> Result<()> {
    let q = format!(
        "INSERT (:{lbl} {{{id}: '{}', {pattern}: '{}', {reason}: '{}', {author}: '{}', \
         {created}: '{}'}})",
        esc(&ig.id),
        esc(&ig.pattern),
        esc(&ig.reason),
        esc(&ig.author),
        esc(&ig.created_at),
        lbl = label::IGNORE,
        id = prop::ID,
        pattern = prop::PATTERN,
        reason = prop::REASON,
        author = prop::AUTHOR,
        created = prop::CREATED_AT,
    );
    db.execute(&q)?;
    Ok(())
}

pub fn list_ignores(db: &dyn LoomDb) -> Result<Vec<Ignore>> {
    let q = format!(
        "MATCH (n:{lbl}) RETURN n.{id}, n.{pattern}, n.{reason}, n.{author}, n.{created} \
         ORDER BY n.{pattern}",
        lbl = label::IGNORE,
        id = prop::ID,
        pattern = prop::PATTERN,
        reason = prop::REASON,
        author = prop::AUTHOR,
        created = prop::CREATED_AT,
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    Ok(result.rows().iter().map(|row| row_to_ignore(row, &cols)).collect())
}

fn row_to_ignore(row: &[Value], cols: &HashMap<&str, usize>) -> Ignore {
    Ignore {
        id:         str_val(get(row, cols, "n.id")),
        pattern:    str_val(get(row, cols, "n.pattern")),
        reason:     str_val(get(row, cols, "n.reason")),
        author:     str_val(get(row, cols, "n.author")),
        created_at: str_val(get(row, cols, "n.created_at")),
    }
}
