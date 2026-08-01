//! Append-only local evidence journal (INV-9).
//!
//! Plane: durable audit trail. Entries are JSONL under `.loom/journal`; this
//! module exposes append and read only — there is deliberately no mutation or
//! deletion API. Journal references are stable entry ids used as
//! `journal:<id>` evidence citations.

use crate::{Result, LOOM_DIR};
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
    // Process id disambiguates concurrent writers: SEQUENCE is process-local and
    // starts at 0, so two processes appending in the same millisecond would mint
    // the same id without it. Sequence still disambiguates within a process.
    let id = format!(
        "{}-{}-{}",
        ts.replace([':', '-', 'T', 'Z', '.'], ""),
        std::process::id(),
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

/// Well-formed entries plus a count of lines that could not be parsed.
///
/// A truncated final record from an interrupted append is corruption, not a
/// fatal read: it is counted and skipped so `status` and `next` stay live on a
/// journal whose tail was damaged. Only genuine IO errors propagate.
pub fn read_counting(root: &Path) -> Result<(Vec<Entry>, usize)> {
    let file = path(root);
    let Ok(file) = OpenOptions::new().read(true).open(&file) else {
        return Ok((Vec::new(), 0));
    };
    let mut entries = Vec::new();
    let mut corrupt = 0usize;
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Entry>(&line) {
            Ok(entry) => entries.push(entry),
            Err(_) => corrupt += 1,
        }
    }
    Ok((entries, corrupt))
}

pub fn read(root: &Path) -> Result<Vec<Entry>> {
    Ok(read_counting(root)?.0)
}

pub fn exists(root: &Path, id: &str) -> Result<bool> {
    let file = path(root);
    let Ok(file) = OpenOptions::new().read(true).open(&file) else {
        return Ok(false);
    };
    // Stream and short-circuit: an existence check must not parse and allocate
    // the whole journal, and a malformed tail line must not fail the lookup.
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if serde_json::from_str::<Entry>(&line).is_ok_and(|entry| entry.id == id) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The journal entries the given evidence cites (`journal:<id>` refs),
/// deduplicated and sorted by id so they travel deterministically in the
/// export. Without them, an imported graph's journal-cited facts (e.g. the
/// ratification acts) have dangling refs and the self-audit flags them as
/// unanchored — the journal is local runtime state, so it must ride along for
/// the refs an export carries.
pub fn cited_entries(root: &Path, evidence: &[crate::evidence::EvidenceRow]) -> Result<Vec<Entry>> {
    let refs: std::collections::BTreeSet<&str> = evidence
        .iter()
        .filter_map(|row| match &row.payload {
            crate::evidence::Evidence::Journal { r#ref } => Some(r#ref.as_str()),
            _ => None,
        })
        .collect();
    if refs.is_empty() {
        return Ok(Vec::new());
    }
    let by_id: std::collections::BTreeMap<String, Entry> =
        read(root)?.into_iter().map(|e| (e.id.clone(), e)).collect();
    Ok(refs.iter().filter_map(|r| by_id.get(*r).cloned()).collect())
}

/// Restore exported journal entries into the local journal, appending any not
/// already present (by id). The original ids are preserved verbatim — the
/// export's evidence cites them, so minting fresh ids would leave the refs
/// dangling. Idempotent across repeated imports.
pub fn restore_entries(root: &Path, entries: &[Entry]) -> Result<usize> {
    let existing: std::collections::BTreeSet<String> =
        read(root)?.into_iter().map(|e| e.id).collect();
    let mut restored = 0;
    let file = path(root);
    fs::create_dir_all(file.parent().expect("journal file has a parent"))?;
    let mut out = OpenOptions::new().create(true).append(true).open(&file)?;
    for entry in entries {
        if existing.contains(&entry.id) {
            continue;
        }
        serde_json::to_writer(&mut out, entry)?;
        out.write_all(b"\n")?;
        restored += 1;
    }
    out.sync_data()?;
    Ok(restored)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A truncated final record from an interrupted append is corruption, not a
    /// fatal read: the good entries above it still parse, the count is surfaced,
    /// and existence checks keep working past the damage.
    #[test]
    fn a_truncated_tail_line_is_skipped_and_counted_not_fatal() {
        let dir = std::env::temp_dir().join(format!("loom-journal-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let a = append(&dir, "ratification", "intent-1", serde_json::json!({})).unwrap();
        let b = append(&dir, "rejection", "intent-2", serde_json::json!({})).unwrap();

        // Simulate a crash mid-append: a partial, unparseable JSON line.
        let mut f = OpenOptions::new().append(true).open(path(&dir)).unwrap();
        f.write_all(b"{\"id\":\"trunc\",\"ts\":\"1\"").unwrap();
        drop(f);

        let entries = read(&dir).unwrap();
        assert_eq!(entries, vec![a.clone(), b.clone()], "good entries survive");

        let (again, corrupt) = read_counting(&dir).unwrap();
        assert_eq!(again, vec![a.clone(), b.clone()]);
        assert_eq!(corrupt, 1, "the truncated tail is counted once");

        assert!(
            exists(&dir, &a.id).unwrap(),
            "a real id resolves past the damage"
        );
        assert!(!exists(&dir, "not-real").unwrap());

        let _ = fs::remove_dir_all(&dir);
    }
}
