//! Delegation queries — subtrees owned by ANOTHER loom graph (federation).
//!
//! A delegation is the third coverage bucket between "grounded here" and
//! "ignored": files matching the pattern are covered by the CHILD graph named
//! by `target` (its committed export). The boundary is an artifact, so
//! `loom coverage` can verify the child export actually exists instead of
//! trusting a blanket exclusion.

use anyhow::Result;
use grafeo::Value;
use std::collections::HashMap;

use crate::db::schema::{label, prop};
use crate::db::LoomDb;
use crate::types::Delegation;

use super::row::{col_map, get, str_val};

pub fn insert_delegation(db: &dyn LoomDb, d: &Delegation) -> Result<()> {
    let q = format!(
        "INSERT (:{lbl} {{{id}: $id, {pattern}: $pattern, {target}: $target, \
         {author}: $author, {created}: $created}})",
        lbl = label::DELEGATION,
        id = prop::ID,
        pattern = prop::PATTERN,
        target = prop::TARGET,
        author = prop::AUTHOR,
        created = prop::CREATED_AT,
    );
    db.execute_with_params(
        &q,
        super::row::sparams(&[
            ("id", &d.id),
            ("pattern", &d.pattern),
            ("target", &d.target),
            ("author", &d.author),
            ("created", &d.created_at),
        ]),
    )?;
    Ok(())
}

pub fn list_delegations(db: &dyn LoomDb) -> Result<Vec<Delegation>> {
    let q = format!(
        "MATCH (n:{lbl}) RETURN n.{id}, n.{pattern}, n.{target}, n.{author}, n.{created} \
         ORDER BY n.{pattern}",
        lbl = label::DELEGATION,
        id = prop::ID,
        pattern = prop::PATTERN,
        target = prop::TARGET,
        author = prop::AUTHOR,
        created = prop::CREATED_AT,
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    Ok(result
        .rows()
        .iter()
        .map(|row| row_to_delegation(row, &cols))
        .collect())
}

fn row_to_delegation(row: &[Value], cols: &HashMap<&str, usize>) -> Delegation {
    Delegation {
        id: str_val(get(row, cols, "n.id")),
        pattern: str_val(get(row, cols, "n.pattern")),
        target: str_val(get(row, cols, "n.target")),
        author: str_val(get(row, cols, "n.author")),
        created_at: str_val(get(row, cols, "n.created_at")),
    }
}
