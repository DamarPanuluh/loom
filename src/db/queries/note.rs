//! Note annotation queries — append-only free-text memory.
//!
//! Notes are plain nodes filtered by target in Rust, never reached by
//! relationship traversal, so they only use grafeo's reliable query paths.

use anyhow::Result;
use grafeo::Value;
use std::collections::HashMap;

use crate::db::schema::{esc, label, prop};
use crate::db::LoomDb;
use crate::types::Note;

use super::row::{col_map, get, str_val};

pub fn insert_note(db: &dyn LoomDb, note: &Note) -> Result<()> {
    let q = format!(
        "INSERT (:{lbl} {{{id}: '{}', {kind}: '{}', {text}: '{}', {author}: '{}', \
         {tkind}: '{}', {tid}: '{}', {created}: '{}'}})",
        esc(&note.id),
        esc(&note.kind),
        esc(&note.text),
        esc(&note.author),
        esc(&note.target_kind),
        esc(&note.target_id),
        esc(&note.created_at),
        lbl = label::NOTE,
        id = prop::ID,
        kind = prop::KIND,
        text = prop::TEXT,
        author = prop::AUTHOR,
        tkind = prop::TARGET_KIND,
        tid = prop::TARGET_ID,
        created = prop::CREATED_AT,
    );
    db.execute(&q)?;
    Ok(())
}

/// All notes, newest last, optionally filtered (in Rust) by target id and/or
/// kind. Scanning + filtering in Rust keeps this on the reliable query path.
pub fn list_notes(
    db: &dyn LoomDb,
    target_id: Option<&str>,
    kind: Option<&str>,
) -> Result<Vec<Note>> {
    let q = format!(
        "MATCH (n:{lbl}) \
         RETURN n.{id}, n.{kind}, n.{text}, n.{author}, n.{tkind}, n.{tid}, n.{created} \
         ORDER BY n.{created}",
        lbl = label::NOTE,
        id = prop::ID,
        kind = prop::KIND,
        text = prop::TEXT,
        author = prop::AUTHOR,
        tkind = prop::TARGET_KIND,
        tid = prop::TARGET_ID,
        created = prop::CREATED_AT,
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    let mut notes: Vec<Note> =
        result.rows().iter().map(|row| row_to_note(row, &cols)).collect();
    if let Some(t) = target_id {
        notes.retain(|n| n.target_id == t);
    }
    if let Some(k) = kind {
        notes.retain(|n| n.kind == k);
    }
    Ok(notes)
}

/// Notes attached to a specific target (an intent or edge id).
pub fn notes_for_target(db: &dyn LoomDb, target_id: &str) -> Result<Vec<Note>> {
    list_notes(db, Some(target_id), None)
}

fn row_to_note(row: &[Value], cols: &HashMap<&str, usize>) -> Note {
    Note {
        id:          str_val(get(row, cols, "n.id")),
        kind:        str_val(get(row, cols, "n.kind")),
        text:        str_val(get(row, cols, "n.text")),
        author:      str_val(get(row, cols, "n.author")),
        target_kind: str_val(get(row, cols, "n.target_kind")),
        target_id:   str_val(get(row, cols, "n.target_id")),
        created_at:  str_val(get(row, cols, "n.created_at")),
    }
}
