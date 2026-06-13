//! Note annotation queries — append-only free-text memory.
//!
//! Notes are plain nodes filtered by target in Rust, never reached by
//! relationship traversal, so they only use grafeo's reliable query paths.

use anyhow::Result;
use grafeo::Value;
use std::collections::HashMap;

use crate::db::schema::{label, prop};
use crate::db::LoomDb;
use crate::types::Note;

use super::row::{col_map, get, str_val};

pub fn insert_note(db: &dyn LoomDb, note: &Note) -> Result<()> {
    // Param-bound: `text` is free prose from an agent — it never enters the
    // query string (see LoomDb::execute_with_params).
    let q = format!(
        "INSERT (:{lbl} {{{id}: $id, {kind}: $kind, {text}: $text, {author}: $author, \
         {tkind}: $tkind, {tid}: $tid, {aud}: $aud, {created}: $created}})",
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
    db.execute_with_params(
        &q,
        super::row::sparams(&[
            ("id", &note.id),
            ("kind", &note.kind),
            ("text", &note.text),
            ("author", &note.author),
            ("tkind", &note.target_kind),
            ("tid", &note.target_id),
            ("aud", &note.audience),
            ("created", &note.created_at),
        ]),
    )?;
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
    insert_note(
        db,
        &Note {
            id: uuid::Uuid::new_v4().to_string(),
            kind: "transition".to_string(),
            text: format!(
                "{} → {}",
                if old_status.is_empty() {
                    "?"
                } else {
                    old_status
                },
                new_status
            ),
            author: author.to_string(),
            target_kind: target_kind.to_string(),
            target_id: target_id.to_string(),
            audience: String::new(),
            created_at: now.to_string(),
        },
    )
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
    insert_note(
        db,
        &Note {
            id: uuid::Uuid::new_v4().to_string(),
            kind: "transition".to_string(),
            text: format!(
                "{} → {} (sync: {})",
                if old_status.is_empty() {
                    "?"
                } else {
                    old_status
                },
                new_status,
                cause
            ),
            author: "loom".to_string(),
            target_kind: target_kind.to_string(),
            target_id: target_id.to_string(),
            audience: String::new(),
            created_at: now.to_string(),
        },
    )
}

/// Extract the staling file from a sync-flip transition note ("… → <status>
/// (sync: <path> changed)") — the inverse of `record_sync_flip`'s text format.
/// `loom next --take` uses it to group a post-sync fix queue by hot file, so
/// an agent reads each changed file once instead of once per stale claim.
/// Returns None for verdict transitions and non-"<path> changed" causes.
pub fn parse_sync_cause(text: &str) -> Option<&str> {
    text.rsplit_once("(sync: ")?
        .1
        .strip_suffix(')')?
        .strip_suffix(" changed")
}

/// All notes, newest last, optionally filtered by target id and/or kind.
///
/// The `target_id` / `kind` predicates are pushed into grafeo's WHERE rather
/// than applied as a post-scan Rust `retain`. On a mature graph the Note label
/// holds thousands of append-only `transition` notes; a targeted or kinded
/// lookup (`notes_for_target`, the align churn scans, per-target show views)
/// would otherwise materialize the entire label into `Note` structs only to
/// throw nearly all of them away. Node-property equality in WHERE is
/// deterministic on grafeo 0.5.42 (`tests/grafeo_probe.rs`) and `$param`-bound
/// (the broken-`id` shadow is relationship-only); `ORDER BY n.created_at`
/// preserves the newest-last contract every caller relies on. The unfiltered
/// call (`None, None`) still does the full scan — by design, its callers want
/// every note.
pub fn list_notes(
    db: &dyn LoomDb,
    target_id: Option<&str>,
    kind: Option<&str>,
) -> Result<Vec<Note>> {
    let mut conds: Vec<String> = Vec::new();
    let mut params: HashMap<String, Value> = HashMap::new();
    if let Some(t) = target_id {
        conds.push(format!("n.{} = $tid", prop::TARGET_ID));
        params.insert("tid".to_string(), Value::from(t));
    }
    if let Some(k) = kind {
        conds.push(format!("n.{} = $kind", prop::KIND));
        params.insert("kind".to_string(), Value::from(k));
    }
    let where_clause = if conds.is_empty() {
        String::new()
    } else {
        format!("WHERE {} ", conds.join(" AND "))
    };
    let q = format!(
        "MATCH (n:{lbl}) {where_clause}\
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
    let result = if params.is_empty() {
        db.execute(&q)?
    } else {
        db.execute_with_params(&q, params)?
    };
    let cols = col_map(&result);
    let notes: Vec<Note> = result
        .rows()
        .iter()
        .map(|row| row_to_note(row, &cols))
        .collect();
    Ok(notes)
}

/// Notes attached to a specific target (an intent or edge id).
pub fn notes_for_target(db: &dyn LoomDb, target_id: &str) -> Result<Vec<Note>> {
    list_notes(db, Some(target_id), None)
}

fn row_to_note(row: &[Value], cols: &HashMap<&str, usize>) -> Note {
    Note {
        id: str_val(get(row, cols, "n.id")),
        kind: str_val(get(row, cols, "n.kind")),
        text: str_val(get(row, cols, "n.text")),
        author: str_val(get(row, cols, "n.author")),
        target_kind: str_val(get(row, cols, "n.target_kind")),
        target_id: str_val(get(row, cols, "n.target_id")),
        // Optional — "" (everyone) on notes from older graphs.
        audience: str_val(get(row, cols, "n.audience")),
        created_at: str_val(get(row, cols, "n.created_at")),
    }
}

/// Delete one note by id (node-keyed — reliable).
pub fn delete_note_by_id(db: &dyn LoomDb, note_id: &str) -> Result<()> {
    db.execute_with_params(
        "MATCH (n:Note {id: $id}) DETACH DELETE n",
        super::row::sparams(&[("id", note_id)]),
    )?;
    Ok(())
}

/// Notes whose target no longer exists — the same three kinds `loom doctor`
/// audits (intent / hypothesis / edge). Floating notes and file notes are
/// never dangling by this definition.
pub fn dangling_notes(db: &dyn LoomDb) -> Result<Vec<Note>> {
    let intent_ids: std::collections::HashSet<String> =
        super::intent::list_intents(db, None, None)?
            .into_iter()
            .map(|i| i.id)
            .collect();
    let hypothesis_ids: std::collections::HashSet<String> =
        super::hypothesis::list_hypotheses(db, None)?
            .into_iter()
            .map(|h| h.id)
            .collect();
    let edge_ids = super::integrity::collect_edge_ids(db)?;
    Ok(list_notes(db, None, None)?
        .into_iter()
        .filter(|n| match n.target_kind.as_str() {
            "intent" => !intent_ids.contains(&n.target_id),
            "hypothesis" => !hypothesis_ids.contains(&n.target_id),
            "edge" => !edge_ids.contains(&n.target_id),
            _ => false,
        })
        .collect())
}

/// Prune notes attached to edges that touched a just-deleted node. Derived
/// edge keys (v4) embed both endpoint ids — `rt:<from>:<to>` — so every edge
/// note whose key contains the deleted node's uuid is now unreachable.
/// Called by the hard-delete paths (intent delete, codefile remove,
/// validation delete) so DETACH DELETE can no longer orphan edge history.
pub fn prune_edge_notes_touching(db: &dyn LoomDb, node_id: &str) -> Result<usize> {
    if node_id.is_empty() {
        return Ok(0);
    }
    let mut pruned = 0usize;
    for n in list_notes(db, None, None)? {
        if n.target_kind == "edge" && n.target_id.contains(node_id) {
            delete_note_by_id(db, &n.id)?;
            pruned += 1;
        }
    }
    Ok(pruned)
}

/// Transition notes a retention prune would DROP: per target, everything beyond
/// the newest `keep_per_target` ROUTINE transitions, EXCEPT every regression
/// marker (`→ failing` / `→ needs_change`), which is kept regardless of age.
///
/// Why it's safe to drop the rest: the regression markers are the only
/// transition history `loom smells` reads (`recurrent_trouble` counts
/// transitions ending in failing/needs_change), so keeping them all leaves
/// every smell finding byte-identical; and the align queue keys off whether an
/// edge has ANY post-confirm sync churn (count ≥ 1), so keeping the newest per
/// target keeps the candidate set identical — only the churn *count* (a ranking
/// input) compresses. What goes is the bulk passing↔needs_reverification sync
/// flip-flop the driving loop appends. Authored notes (decision/commentary/…)
/// and `confirm` are not transitions, so they're never touched.
pub fn prunable_transition_notes(db: &dyn LoomDb, keep_per_target: usize) -> Result<Vec<Note>> {
    // list_notes orders ASC by created_at, so each target's slice is
    // oldest-first; walk it reversed to keep the newest survivors.
    let transitions = list_notes(db, None, Some("transition"))?;
    let mut by_target: HashMap<&str, Vec<&Note>> = HashMap::new();
    for n in &transitions {
        by_target.entry(n.target_id.as_str()).or_default().push(n);
    }
    let mut to_drop: Vec<Note> = Vec::new();
    for notes in by_target.values() {
        let mut kept_routine = 0usize;
        for n in notes.iter().rev() {
            if n.text.ends_with("→ failing") || n.text.ends_with("→ needs_change") {
                continue; // the recurrent_trouble signal — always kept
            }
            if kept_routine < keep_per_target {
                kept_routine += 1;
                continue;
            }
            to_drop.push((*n).clone());
        }
    }
    Ok(to_drop)
}
