//! Append-only local evidence journal (INV-9).
//!
//! Plane: durable audit trail. Entries are JSONL under `.loom/journal`; this
//! module exposes append and read only — there is deliberately no mutation or
//! deletion API. Journal references are stable entry ids used as
//! `journal:<id>` evidence citations.

use crate::{Result, LOOM_DIR};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const JOURNAL_DIR: &str = "journal";
const EVENTS_FILE: &str = "events.jsonl";
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub ts: String,
    pub actor: String,
    pub event: String,
    pub target_id: String,
    pub payload: Value,
}

pub fn path(root: &Path) -> PathBuf {
    root.join(LOOM_DIR).join(JOURNAL_DIR).join(EVENTS_FILE)
}

/// Append exactly one immutable event and return its evidence reference.
pub fn append(root: &Path, event: &str, target_id: &str, payload: Value) -> Result<Entry> {
    let ts = timestamp();
    let id = format!(
        "{}-{}",
        ts.replace([':', '-', 'T', 'Z', '.'], ""),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let entry = Entry {
        id,
        ts,
        actor: std::env::var("LOOM_AGENT").unwrap_or_else(|_| "solo".into()),
        event: event.into(),
        target_id: target_id.into(),
        payload,
    };
    let file = path(root);
    fs::create_dir_all(file.parent().expect("journal file has a parent"))?;
    let mut out = OpenOptions::new().create(true).append(true).open(&file)?;
    serde_json::to_writer(&mut out, &entry)?;
    out.write_all(b"\n")?;
    out.sync_data()?;
    Ok(entry)
}

pub fn read(root: &Path) -> Result<Vec<Entry>> {
    let file = path(root);
    let Ok(file) = OpenOptions::new().read(true).open(&file) else {
        return Ok(Vec::new());
    };
    BufReader::new(file)
        .lines()
        .filter(|line| match line {
            Ok(line) => !line.trim().is_empty(),
            Err(_) => true,
        })
        .map(|line| {
            let line = line?;
            serde_json::from_str(&line).with_context(|| "parsing append-only journal entry")
        })
        .collect()
}

pub fn exists(root: &Path, id: &str) -> Result<bool> {
    Ok(read(root)?.iter().any(|entry| entry.id == id))
}

pub fn reference(entry: &Entry) -> String {
    format!("journal:{}", entry.id)
}

/// The journal's clock, shared so every append-only record and every observed
/// run agree on what "now" means.
pub fn now_iso() -> String {
    timestamp()
}

fn timestamp() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    // UTC epoch milliseconds are lossless and unambiguous while avoiding a
    // new time-formatting dependency.
    format!("{millis}")
}
