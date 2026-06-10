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
         {tkind}: '{}', {tid}: '{}', {aud}: '{}', {created}: '{}'}})",
        esc(&note.id),
        esc(&note.kind),
        esc(&note.text),
        esc(&note.author),
        esc(&note.target_kind),
        esc(&note.target_id),
        esc(&note.audience),
        esc(&note.created_at),
        lbl = label::NOTE,
        id = prop::ID,
        kind = prop::KIND,
        text = prop::TEXT,
        author = prop::AUTHOR,
        tkind = prop::TARGET_KIND,
        tid = prop::TARGET_ID,
        aud = prop::AUDIENCE,
        created = prop::CREATED_AT,
    );
    db.execute(&q)?;
    Ok(())
}

/// Auto-record a status transition as an append-only `transition` note — the
/// graph's recurrence memory. Written by loom itself at every verdict change;
/// `loom smells` reads the history to surface targets that keep regressing.
/// Text format is machine-parseable: "<old> → <new>".
pub fn record_transition(
    db: &dyn LoomDb,
    target_kind: &str, // "edge" | "intent"
    target_id: &str,
    old_status: &str,
    new_status: &str,
    author: &str,
    now: &str,
) -> Result<()> {
    if old_status == new_status {
        return Ok(());
    }
    insert_note(db, &Note {
        id: uuid::Uuid::new_v4().to_string(),
        kind: "transition".to_string(),
        text: format!("{} → {}", if old_status.is_empty() { "?" } else { old_status }, new_status),
        author: author.to_string(),
        target_kind: target_kind.to_string(),
        target_id: target_id.to_string(),
        audience: String::new(),
        created_at: now.to_string(),
    })
}

/// Auto-record a `loom sync` staleness flip with its CAUSE — "why is this edge
/// needs_reverification?" answered in place. Same transition-note channel as
/// `record_transition`, with the triggering file appended: the text stays
/// machine-screenable (the recurrent-trouble smell matches on the "→ <status>"
/// suffix of *verdict* transitions, which a "(sync: …)" tail never fakes) while
/// `loom edge show` / `loom next` surface the explanation with no extra lookup.
pub fn record_sync_flip(
    db: &dyn LoomDb,
    target_kind: &str, // "edge" | "intent"
    target_id: &str,
    old_status: &str,
    new_status: &str,
    cause: &str, // e.g. "src/db/mod.rs changed"
    now: &str,
) -> Result<()> {
    insert_note(db, &Note {
        id: uuid::Uuid::new_v4().to_string(),
        kind: "transition".to_string(),
        text: format!(
            "{} → {} (sync: {})",
            if old_status.is_empty() { "?" } else { old_status },
            new_status,
            cause
        ),
        author: "loom".to_string(),
        target_kind: target_kind.to_string(),
        target_id: target_id.to_string(),
        audience: String::new(),
        created_at: now.to_string(),
    })
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
         RETURN n.{id}, n.{kind}, n.{text}, n.{author}, n.{tkind}, n.{tid}, n.{aud}, n.{created} \
         ORDER BY n.{created}",
        lbl = label::NOTE,
        id = prop::ID,
        kind = prop::KIND,
        text = prop::TEXT,
        author = prop::AUTHOR,
        tkind = prop::TARGET_KIND,
        tid = prop::TARGET_ID,
        aud = prop::AUDIENCE,
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
        // Optional — "" (everyone) on notes from older graphs.
        audience:    str_val(get(row, cols, "n.audience")),
        created_at:  str_val(get(row, cols, "n.created_at")),
    }
}
