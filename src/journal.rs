//! Append-only local evidence journal (INV-9).
//!
//! Plane: durable audit trail. Entries are JSONL under `.loom/journal`; this
//! module exposes append and read only — there is deliberately no mutation or
//! deletion API. Journal references are stable entry ids used as
//! `journal:<id>` evidence citations.

use crate::{Result, LOOM_DIR};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(test)]
use std::cell::Cell;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const JOURNAL_DIR: &str = "journal";
const EVENTS_FILE: &str = "events.jsonl";
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
// Per-thread count of full journal parses (`read` / `read_counting`). This is a
// permanent regression probe for the N×reread bug in hot paths, compiled only
// into test builds. Thread locality keeps parallel tests from contaminating the
// measurement while preserving exact call-path counts.
#[cfg(test)]
thread_local! {
    static FULL_READS: Cell<usize> = const { Cell::new(0) };
}

/// How many times this process has fully parsed the journal (test builds only).
#[cfg(test)]
pub fn full_read_count() -> usize {
    FULL_READS.with(Cell::get)
}

/// Reset the full-read counter before a regression assertion (test builds only).
#[cfg(test)]
pub fn reset_full_read_count() {
    FULL_READS.with(|count| count.set(0));
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    #[default]
    Local,
    Imported,
}

impl Origin {
    fn is_local(&self) -> bool {
        *self == Self::Local
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub ts: String,
    /// Authorization identity (`solo` or `llm:<lane>`).
    pub actor: String,
    /// Executor profile (for example `loom-auditor`), independent from actor
    /// authority. Legacy entries deserialize with no profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    pub event: String,
    pub target_id: String,
    pub payload: Value,
    /// Trust provenance is local runtime state. Old journal rows deserialize as
    /// local; imports are forcibly marked imported by `restore_entries`.
    #[serde(default, skip_serializing_if = "Origin::is_local")]
    pub origin: Origin,
}

pub fn path(root: &Path) -> PathBuf {
    root.join(LOOM_DIR).join(JOURNAL_DIR).join(EVENTS_FILE)
}

pub fn append_once(
    root: &Path,
    identity: &crate::identity::ExecutionIdentity,
    event: &str,
    target_id: &str,
    payload: Value,
    same_transition: impl Fn(&Entry) -> bool,
) -> Result<Option<Entry>> {
    let _lock = journal_lock(root)?;
    let entries = read_untracked(root)?;
    // Only local rows count as evidence of this transition: imported history
    // is another graph's record and must never suppress a fresh local entry.
    if entries
        .iter()
        .filter(|entry| entry.origin == Origin::Local)
        .any(same_transition)
    {
        return Ok(None);
    }
    let entry = new_entry(identity, event, target_id, payload);
    let mut line = serde_json::to_vec(&entry)?;
    line.push(b'\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path(root))?;
    file.write_all(&line)?;
    file.sync_data()?;
    Ok(Some(entry))
}

fn new_entry(
    identity: &crate::identity::ExecutionIdentity,
    event: &str,
    target_id: &str,
    payload: Value,
) -> Entry {
    let ts = timestamp();
    let id = format!(
        "{}-{}-{}",
        ts.replace([':', '-', 'T', 'Z', '.'], ""),
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    Entry {
        id,
        ts,
        actor: identity.actor(),
        profile: identity.profile().map(str::to_owned),
        event: event.into(),
        target_id: target_id.into(),
        payload,
        origin: Origin::Local,
    }
}

/// Append exactly one immutable event and return its evidence reference.
pub fn append(
    root: &Path,
    identity: &crate::identity::ExecutionIdentity,
    event: &str,
    target_id: &str,
    payload: Value,
) -> Result<Entry> {
    let entry = new_entry(identity, event, target_id, payload);
    // Serialize the full JSONL record first, then append under a dedicated
    // journal lock. Shared graph readers may append from many processes at
    // once; split `to_writer` + newline writes interleave into corrupt lines.
    let mut line = serde_json::to_vec(&entry)?;
    line.push(b'\n');
    write_record(root, &line)?;
    Ok(entry)
}

/// Exclusive lock for the journal directory — not the graph lock.
///
/// Readers share the graph lock and still append here; serializing only the
/// complete JSONL record under this file keeps concurrent appends well-formed.
fn journal_lock(root: &Path) -> Result<std::fs::File> {
    let dir = path(root)
        .parent()
        .expect("journal file has a parent")
        .to_path_buf();
    fs::create_dir_all(&dir)?;
    let lock_path = dir.join("lock");
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    file.lock()?;
    Ok(file)
}

/// Lock an already-materialized journal without creating any preflight
/// artifacts. A missing lock means there is no lockable state yet; callers
/// still inspect the current journal read-only and the mutating restore repeats
/// the plan under the creating lock before writing.
fn existing_journal_lock(root: &Path) -> Result<Option<std::fs::File>> {
    let lock_path = path(root)
        .parent()
        .expect("journal file has a parent")
        .join("lock");
    let file = match OpenOptions::new().read(true).write(true).open(&lock_path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    file.lock()?;
    Ok(Some(file))
}

fn write_record(root: &Path, line: &[u8]) -> Result<()> {
    let file = path(root);
    let _guard = journal_lock(root)?;
    let mut out = OpenOptions::new().create(true).append(true).open(&file)?;
    out.write_all(line)?;
    out.sync_data()?;
    Ok(())
}

fn read_untracked(root: &Path) -> Result<Vec<Entry>> {
    let file = path(root);
    let Ok(file) = OpenOptions::new().read(true).open(&file) else {
        return Ok(Vec::new());
    };
    let mut entries = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if let Ok(entry) = serde_json::from_str::<Entry>(&line) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

/// Well-formed entries plus a count of lines that could not be parsed.
///
/// A truncated final record from an interrupted append is corruption, not a
/// fatal read: it is counted and skipped so `status` and `next` stay live on a
/// journal whose tail was damaged. Only genuine IO errors propagate.
pub fn read_counting(root: &Path) -> Result<(Vec<Entry>, usize)> {
    #[cfg(test)]
    FULL_READS.with(|count| count.set(count.get() + 1));
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
    Ok(refs
        .iter()
        .filter_map(|r| by_id.get(*r).cloned())
        // Provenance is assigned by the receiving import boundary, not claimed
        // by an export. Normalizing here also preserves export→import→export
        // byte determinism for journal-cited evidence.
        .map(|mut entry| {
            entry.origin = Origin::Local;
            entry
        })
        .collect())
}

/// Side-effect-free collision/duplicate validation for a journal restore.
///
/// If a journal lock already exists it is acquired before reading. A fresh
/// destination is inspected without creating `.loom/journal` or a lock file;
/// [`restore_entries`] always repeats the same plan under the creating lock, so
/// a concurrent append between this preflight and restore is still rejected.
pub fn preflight_restore_entries(root: &Path, entries: &[Entry]) -> Result<()> {
    let _guard = existing_journal_lock(root)?;
    let existing = read(root)?;
    let pending = plan_restore_entries(&existing, entries)?;
    serialize_restore_entries(&pending)?;
    Ok(())
}

/// Apply all restore collision and duplicate rules and return the normalized,
/// first-seen imported records which still need to be appended.
fn plan_restore_entries(existing: &[Entry], entries: &[Entry]) -> Result<Vec<Entry>> {
    let mut existing_by_id: std::collections::BTreeMap<String, Vec<&Entry>> =
        std::collections::BTreeMap::new();
    for entry in existing {
        existing_by_id
            .entry(entry.id.clone())
            .or_default()
            .push(entry);
    }

    // Normalize and validate duplicate ids in the incoming slice before
    // considering any writes. Keep first-seen order for deterministic JSONL.
    let mut normalized = Vec::with_capacity(entries.len());
    let mut incoming_by_id: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();
    for entry in entries {
        let mut imported = entry.clone();
        imported.origin = Origin::Imported;
        if let Some(index) = incoming_by_id.get(&imported.id) {
            if normalized[*index] != imported {
                anyhow::bail!(
                    "journal restore ID collision for '{}': incoming provenance is imported but duplicate content differs",
                    imported.id
                );
            }
            continue;
        }
        incoming_by_id.insert(imported.id.clone(), normalized.len());
        normalized.push(imported);
    }

    let mut pending = Vec::new();
    for incoming in normalized {
        if let Some(existing_rows) = existing_by_id.get(&incoming.id) {
            for existing in existing_rows {
                match existing.origin {
                    Origin::Local => anyhow::bail!(
                        "journal restore ID collision for '{}': existing provenance is local; imported references cannot alias local authority",
                        incoming.id
                    ),
                    Origin::Imported if *existing != &incoming => anyhow::bail!(
                        "journal restore ID collision for '{}': existing provenance is imported but content differs",
                        incoming.id
                    ),
                    Origin::Imported => {}
                }
            }
            continue;
        }
        pending.push(incoming);
    }
    Ok(pending)
}

fn serialize_restore_entries(entries: &[Entry]) -> Result<Vec<u8>> {
    let mut records = Vec::new();
    for entry in entries {
        serde_json::to_writer(&mut records, entry)?;
        records.push(b'\n');
    }
    Ok(records)
}

/// Restore exported journal entries into the local journal. Original ids are
/// preserved verbatim because exported evidence cites them. Every supplied row
/// is normalized to [`Origin::Imported`]. Under the journal lock, the complete
/// batch is preflighted before any append:
///
/// - an id matching local authority is always rejected;
/// - an id matching an imported row is an idempotent skip only when every field
///   is identical after provenance normalization; and
/// - duplicate incoming ids follow the same exact-match rule.
pub fn restore_entries(root: &Path, entries: &[Entry]) -> Result<usize> {
    let file = path(root);
    let _guard = journal_lock(root)?;

    // Re-read and re-plan after acquiring the write lock. This closes the race
    // between an earlier import preflight and graph restore.
    let existing = read(root)?;
    let pending = plan_restore_entries(&existing, entries)?;

    // Serialization is also part of preflight: no journal bytes are written
    // until every collision and every record encoding has succeeded.
    let restored = pending.len();
    let records = serialize_restore_entries(&pending)?;
    if records.is_empty() {
        return Ok(0);
    }

    let mut out = OpenOptions::new().create(true).append(true).open(&file)?;
    out.write_all(&records)?;
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

/// Render epoch milliseconds as `YYYY-MM-DDTHH:MM:SSZ` (UTC). Civil-from-days
/// arithmetic by hand — the crate deliberately carries no time-formatting
/// dependency (see `stamp_millis`). For human-facing surfaces (lock-holder
/// identities); storage keeps the native epoch-millis/ISO forms.
pub fn millis_to_iso(millis: i64) -> String {
    let secs = millis.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        sod / 3600,
        (sod / 60) % 60,
        sod % 60
    )
}

/// Parse a journal or fact timestamp to UTC epoch milliseconds.
///
/// Accepts either epoch-millis decimal strings (the journal's native format)
/// or canonical UTC ISO stamps: `YYYY-MM-DDTHH:MM:SSZ` and
/// `YYYY-MM-DDTHH:MM:SS.sssZ`, where the fraction contains one to three digits.
pub fn stamp_millis(stamp: &str) -> Option<i64> {
    if let Ok(millis) = stamp.parse::<i64>() {
        return Some(millis);
    }

    let bytes = stamp.as_bytes();
    let fraction_len = match bytes.len() {
        20 => 0,
        22..=24 => bytes.len() - 21,
        _ => return None,
    };
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[bytes.len() - 1] != b'Z'
        || (fraction_len == 0 && bytes[19] != b'Z')
        || (fraction_len > 0 && bytes[19] != b'.')
    {
        return None;
    }

    fn decimal(bytes: &[u8]) -> Option<i64> {
        if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
            return None;
        }
        bytes.iter().try_fold(0i64, |value, digit| {
            value.checked_mul(10)?.checked_add(i64::from(*digit - b'0'))
        })
    }

    let year = decimal(&bytes[0..4])?;
    let month = decimal(&bytes[5..7])?;
    let day = decimal(&bytes[8..10])?;
    let hour = decimal(&bytes[11..13])?;
    let minute = decimal(&bytes[14..16])?;
    let second = decimal(&bytes[17..19])?;
    let fraction = if fraction_len == 0 {
        0
    } else {
        let value = decimal(&bytes[20..20 + fraction_len])?;
        value.checked_mul(match fraction_len {
            1 => 100,
            2 => 10,
            3 => 1,
            _ => return None,
        })?
    };

    if !(1..=12).contains(&month) || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_month = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if day == 0 || day > days_in_month {
        return None;
    }

    // Howard Hinnant's days_from_civil, with every arithmetic step checked so
    // malformed or future-expanded inputs can never wrap into a valid instant.
    let adjusted_year = if month <= 2 {
        year.checked_sub(1)?
    } else {
        year
    };
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year.checked_sub(era.checked_mul(400)?)?;
    let month_prime = if month > 2 {
        month.checked_sub(3)?
    } else {
        month.checked_add(9)?
    };
    let day_of_year = 153i64
        .checked_mul(month_prime)?
        .checked_add(2)?
        .checked_div(5)?
        .checked_add(day)?
        .checked_sub(1)?;
    let day_of_era = year_of_era
        .checked_mul(365)?
        .checked_add(year_of_era / 4)?
        .checked_sub(year_of_era / 100)?
        .checked_add(day_of_year)?;
    let days = era
        .checked_mul(146_097)?
        .checked_add(day_of_era)?
        .checked_sub(719_468)?;
    let seconds = days
        .checked_mul(86_400)?
        .checked_add(hour.checked_mul(3_600)?)?
        .checked_add(minute.checked_mul(60)?)?
        .checked_add(second)?;
    seconds.checked_mul(1_000)?.checked_add(fraction)
}

/// Normalize a journal or fact timestamp to its UTC minute key.
///
/// Accepts the same ISO and epoch-millisecond representations as
/// [`stamp_millis`]. Invalid stamps return `None` so time-sensitive callers can
/// fail closed instead of grouping or authorizing malformed records.
pub fn minute_key(stamp: &str) -> Option<String> {
    let millis = stamp_millis(stamp)?;
    Some(millis_to_iso(millis).chars().take(16).collect())
}

/// [`minute_key`] that also accepts an already-normalized `YYYY-MM-DDTHH:MM`
/// minute (as printed in audit findings), by re-expanding it to a full stamp.
/// One definition: the audit and the batch-authorization seal must normalize
/// a human-quoted minute identically or an attestation could miss its burst.
pub fn normalized_minute(stamp_or_minute: &str) -> Option<String> {
    minute_key(stamp_or_minute).or_else(|| minute_key(&format!("{stamp_or_minute}:00.000Z")))
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

    #[test]
    fn full_read_counter_is_isolated_from_parallel_test_threads() {
        let dir = std::env::temp_dir().join(format!(
            "loom-read-counter-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);

        reset_full_read_count();
        let child_dir = dir.clone();
        std::thread::spawn(move || {
            reset_full_read_count();
            read(&child_dir).unwrap();
            assert_eq!(full_read_count(), 1);
        })
        .join()
        .unwrap();

        assert_eq!(
            full_read_count(),
            0,
            "a journal read in another test thread must not contaminate this probe"
        );
        read(&dir).unwrap();
        assert_eq!(full_read_count(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_once_is_concurrent_idempotent() {
        let dir = std::env::temp_dir().join(format!(
            "loom-append-once-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        let mut threads = Vec::new();
        for _ in 0..8 {
            let dir = dir.clone();
            threads.push(std::thread::spawn(move || {
                append_once(
                    &dir,
                    &crate::identity::ExecutionIdentity::solo(),
                    "proof_strength_changed",
                    "validation-1",
                    serde_json::json!({"witness_model":"v2"}),
                    |entry| {
                        entry.event == "proof_strength_changed"
                            && entry.target_id == "validation-1"
                            && entry.payload["witness_model"] == "v2"
                    },
                )
                .unwrap();
            }));
        }
        for thread in threads {
            thread.join().unwrap();
        }
        let entries = read(&dir).unwrap();
        assert_eq!(
            entries.len(),
            1,
            "concurrent append_once with one transition must write exactly one record"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_once_ignores_imported_rows_when_deduping() {
        let dir = std::env::temp_dir().join(format!(
            "loom-append-once-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        // An imported row for the same transition must not suppress a fresh
        // local entry: imported history is another graph's record.
        let imported = super::Entry {
            id: "imported-1".into(),
            ts: "0".into(),
            actor: "solo".into(),
            profile: None,
            event: "proof_strength_changed".into(),
            target_id: "validation-1".into(),
            payload: serde_json::json!({"witness_model":"v2"}),
            origin: Origin::Imported,
        };
        let mut line = serde_json::to_vec(&imported).unwrap();
        line.push(b'\n');
        fs::create_dir_all(path(&dir).parent().unwrap()).unwrap();
        fs::write(path(&dir), line).unwrap();

        let written = append_once(
            &dir,
            &crate::identity::ExecutionIdentity::solo(),
            "proof_strength_changed",
            "validation-1",
            serde_json::json!({"witness_model":"v2"}),
            |entry| {
                entry.event == "proof_strength_changed"
                    && entry.target_id == "validation-1"
                    && entry.payload["witness_model"] == "v2"
            },
        )
        .unwrap();
        assert!(
            written.is_some(),
            "an imported row must not suppress the local entry"
        );
        assert_eq!(read(&dir).unwrap().len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_once_distinct_transitions_both_land() {
        let dir = std::env::temp_dir().join(format!(
            "loom-append-once-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        append_once(
            &dir,
            &crate::identity::ExecutionIdentity::solo(),
            "proof_strength_changed",
            "validation-1",
            serde_json::json!({"witness_model":"v2"}),
            |_| false,
        )
        .unwrap();
        append_once(
            &dir,
            &crate::identity::ExecutionIdentity::solo(),
            "proof_strength_changed",
            "validation-2",
            serde_json::json!({"witness_model":"v2"}),
            |_| false,
        )
        .unwrap();
        assert_eq!(read(&dir).unwrap().len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn entry_origin_defaults_local_and_append_is_local() {
        let legacy: Entry = serde_json::from_value(serde_json::json!({
            "id": "legacy",
            "ts": "0",
            "actor": "solo",
            "event": "legacy",
            "target_id": "graph",
            "payload": {}
        }))
        .unwrap();
        assert_eq!(legacy.origin, Origin::Local);

        let dir = std::env::temp_dir().join(format!("loom-origin-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let appended = append(
            &dir,
            &crate::identity::ExecutionIdentity::solo(),
            "local",
            "graph",
            serde_json::json!({}),
        )
        .unwrap();
        assert_eq!(appended.origin, Origin::Local);
        assert_eq!(read(&dir).unwrap()[0].origin, Origin::Local);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_forcibly_marks_caller_supplied_local_as_imported() {
        let dir = std::env::temp_dir().join(format!("loom-import-origin-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let supplied = Entry {
            id: "crafted-local".into(),
            ts: "0".into(),
            actor: "caller".into(),
            profile: None,
            event: "batch_authorization".into(),
            target_id: "digest".into(),
            payload: serde_json::json!({}),
            origin: Origin::Local,
        };
        assert_eq!(restore_entries(&dir, &[supplied]).unwrap(), 1);
        assert_eq!(read(&dir).unwrap()[0].origin, Origin::Imported);
        let _ = fs::remove_dir_all(&dir);
    }

    fn restore_test_entry(id: &str) -> Entry {
        Entry {
            id: id.into(),
            ts: "1000".into(),
            actor: "exporter".into(),
            profile: None,
            event: "ratification".into(),
            target_id: "intent-1".into(),
            payload: serde_json::json!({"decision": "accepted"}),
            origin: Origin::Local,
        }
    }

    fn write_test_entry(root: &Path, entry: &Entry) {
        let mut line = serde_json::to_vec(entry).unwrap();
        line.push(b'\n');
        write_record(root, &line).unwrap();
    }

    #[test]
    fn preflight_on_fresh_destination_creates_no_journal_artifacts() {
        let dir = std::env::temp_dir().join(format!(
            "loom-journal-preflight-fresh-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        preflight_restore_entries(&dir, &[restore_test_entry("novel-id")]).unwrap();

        assert!(!dir.join(LOOM_DIR).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn preflight_and_restore_share_collision_rules() {
        let dir = std::env::temp_dir().join(format!(
            "loom-journal-preflight-collision-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let local = restore_test_entry("preflight-local-id");
        write_test_entry(&dir, &local);

        let preflight = preflight_restore_entries(&dir, std::slice::from_ref(&local))
            .unwrap_err()
            .to_string();
        let restore = restore_entries(&dir, &[local]).unwrap_err().to_string();
        assert_eq!(preflight, restore);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_rejects_collision_with_local_authority_even_when_payload_matches() {
        let dir = std::env::temp_dir().join(format!(
            "loom-restore-local-collision-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let local = restore_test_entry("local-authority-id");
        write_test_entry(&dir, &local);

        let error = restore_entries(&dir, &[local]).unwrap_err().to_string();
        assert!(error.contains("local-authority-id"));
        assert!(error.contains("existing provenance is local"));
        assert!(!error.contains("accepted"), "error must not expose payload");
        assert_eq!(read(&dir).unwrap().len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_identical_imported_collision_is_idempotent() {
        let dir = std::env::temp_dir().join(format!(
            "loom-restore-imported-idempotent-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let entry = restore_test_entry("imported-idempotent-id");

        assert_eq!(
            restore_entries(&dir, &[entry.clone(), entry.clone()]).unwrap(),
            1
        );
        // Caller-controlled provenance is normalized before equality, so the
        // original exported row is identical to the stored imported row.
        assert_eq!(
            restore_entries(&dir, std::slice::from_ref(&entry)).unwrap(),
            0
        );
        assert_eq!(restore_entries(&dir, &[entry.clone(), entry]).unwrap(), 0);
        assert_eq!(read(&dir).unwrap().len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_rejects_imported_collision_when_any_content_field_differs() {
        type EntryMutation = (&'static str, fn(&mut Entry));
        let cases: Vec<EntryMutation> = vec![
            ("ts", |entry| entry.ts = "1001".into()),
            ("actor", |entry| entry.actor = "other".into()),
            ("event", |entry| entry.event = "rejection".into()),
            ("target", |entry| entry.target_id = "intent-2".into()),
            ("payload", |entry| {
                entry.payload = serde_json::json!({"decision": "rejected"})
            }),
        ];

        for (field, mutate) in cases {
            let dir = std::env::temp_dir().join(format!(
                "loom-restore-imported-diff-{field}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            let entry = restore_test_entry("imported-conflict-id");
            assert_eq!(
                restore_entries(&dir, std::slice::from_ref(&entry)).unwrap(),
                1
            );
            let mut conflicting = entry;
            mutate(&mut conflicting);

            let error = restore_entries(&dir, &[conflicting])
                .unwrap_err()
                .to_string();
            assert!(error.contains("imported-conflict-id"));
            assert!(error.contains("existing provenance is imported"));
            assert!(error.contains("content differs"));
            assert!(!error.contains("rejected"), "error must not expose payload");
            assert_eq!(read(&dir).unwrap().len(), 1, "case {field}");
            let _ = fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn restore_rejects_conflicting_duplicate_ids_in_one_batch() {
        let dir = std::env::temp_dir().join(format!(
            "loom-restore-incoming-duplicate-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let first = restore_test_entry("incoming-duplicate-id");
        let mut second = first.clone();
        second.actor = "different-exporter".into();

        let error = restore_entries(&dir, &[first, second])
            .unwrap_err()
            .to_string();
        assert!(error.contains("incoming-duplicate-id"));
        assert!(error.contains("incoming provenance is imported"));
        assert!(error.contains("duplicate content differs"));
        assert!(read(&dir).unwrap().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_preflight_prevents_partial_write_when_later_id_collides() {
        let dir = std::env::temp_dir().join(format!(
            "loom-restore-atomic-collision-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let local = restore_test_entry("later-local-collision");
        write_test_entry(&dir, &local);
        let novel = restore_test_entry("must-not-be-appended");

        let error = restore_entries(&dir, &[novel, local])
            .unwrap_err()
            .to_string();
        assert!(error.contains("later-local-collision"));
        let after = read(&dir).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, "later-local-collision");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn millis_to_iso_matches_known_instants() {
        assert_eq!(millis_to_iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(millis_to_iso(1_785_845_466_782), "2026-08-04T12:11:06Z");
        // Negative instants (pre-epoch) must round toward the floor, not zero.
        assert_eq!(millis_to_iso(-1), "1969-12-31T23:59:59Z");
        assert_eq!(millis_to_iso(951_782_400_000), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn stamp_millis_accepts_epoch_and_canonical_iso() {
        assert_eq!(stamp_millis("0"), Some(0));
        assert_eq!(stamp_millis("-1"), Some(-1));
        assert_eq!(stamp_millis("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(stamp_millis("1970-01-01T00:00:00.1Z"), Some(100));
        assert_eq!(stamp_millis("1970-01-01T00:00:00.12Z"), Some(120));
        assert_eq!(stamp_millis("1970-01-01T00:00:00.123Z"), Some(123));
        assert_eq!(stamp_millis("2000-02-29T00:00:00Z"), Some(951_782_400_000));
    }

    #[test]
    fn stamp_millis_rejects_noncanonical_suffixes_and_fractions() {
        for stamp in [
            "1970-01-01T00:00:00",
            "1970-01-01T00:00:00ZZ",
            "1970-01-01T00:00:00Zjunk",
            "1970-01-01T00:00:00.1234Z",
            "1970-01-01T00:00:00.Z",
            "1970-01-01T00:00:00.aZ",
            "1970-01-01T00:00:00.1aZ",
            "1970-01-01T00:00:00.+Z",
            "1970-01-01T00:00:00.１２Z",
        ] {
            assert_eq!(stamp_millis(stamp), None, "accepted {stamp:?}");
        }
    }

    #[test]
    fn stamp_millis_rejects_invalid_calendar_and_clock_components() {
        for stamp in [
            "2024-00-01T00:00:00Z",
            "2024-13-01T00:00:00Z",
            "2024-04-31T00:00:00Z",
            "2023-02-29T00:00:00Z",
            "1900-02-29T00:00:00Z",
            "2024-02-30T00:00:00Z",
            "2024-01-00T00:00:00Z",
            "2024-01-01T24:00:00Z",
            "2024-01-01T00:60:00Z",
            "2024-01-01T00:00:60Z",
        ] {
            assert_eq!(stamp_millis(stamp), None, "accepted {stamp:?}");
        }
    }

    #[test]
    fn stamp_millis_rejects_malformed_components_and_overflow() {
        for stamp in [
            "2024-1-01T00:00:00Z",
            "2024-01-1T00:00:00Z",
            "2024-01-01T0:00:00Z",
            "2024-01-01T00:0:00Z",
            "2024-01-01T00:00:0Z",
            "2024/01/01T00:00:00Z",
            "2024-01-01 00:00:00Z",
            "2024-01-01T00-00-00Z",
            "2024-01-01T00:00:00:00Z",
            "2024-01-01T00:00:00.000Zextra",
            "9223372036854775808",
        ] {
            assert_eq!(stamp_millis(stamp), None, "accepted {stamp:?}");
        }
    }

    #[test]
    fn iso_and_epoch_stamps_share_a_minute_key() {
        assert_eq!(
            minute_key("2026-07-25T07:10:25.553Z"),
            minute_key("1784963425553")
        );
        assert_eq!(
            minute_key("1784963425553").as_deref(),
            Some("2026-07-25T07:10")
        );
        assert_eq!(minute_key("not-a-stamp"), None);
    }

    /// A truncated final record from an interrupted append is corruption, not a
    /// fatal read: the good entries above it still parse, the count is surfaced,
    /// and existence checks keep working past the damage.
    #[test]
    fn a_truncated_tail_line_is_skipped_and_counted_not_fatal() {
        let dir = std::env::temp_dir().join(format!("loom-journal-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let identity = crate::identity::ExecutionIdentity::solo();
        let a = append(
            &dir,
            &identity,
            "ratification",
            "intent-1",
            serde_json::json!({}),
        )
        .unwrap();
        let b = append(
            &dir,
            &identity,
            "rejection",
            "intent-2",
            serde_json::json!({}),
        )
        .unwrap();

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
