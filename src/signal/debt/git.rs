//! Bounded Git history adapter for co-change debt.
//!
//! Spawns system git with hard timeout/output caps; any failure, empty sample,
//! or malformed framing becomes `HistoryAvailability::Unavailable`. The NUL
//! parser is pure after the bytes are in hand.

use process_control::{ChildExt, Control};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

pub(crate) const CO_CHANGE_MAX_COMMITS: usize = 1000;
pub(crate) const CO_CHANGE_GIT_TIMEOUT_SECS: u64 = 10;
const CO_CHANGE_GIT_OUTPUT_CAP: usize = 16 * 1024 * 1024;

/// Whether git history could be sampled for co-change.
pub(super) enum HistoryAvailability {
    Available(GitHistory),
    Unavailable,
}

/// Parsed git sample: raw commits newest-first, rename lineage applied later.
pub(super) struct GitHistory {
    pub(super) commits: Vec<GitCommit>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GitStatus {
    Modify, // M, A, T, or unknown letter treated as touch
    Delete, // D — bulk noise only, never a member touch
    Rename, // R* — maps old path → new path's current node
    Copy,   // C* — both endpoints counted independently
}

#[derive(Clone, Debug)]
pub(super) struct GitChange {
    pub(super) status: GitStatus,
    pub(super) path: String,
    /// Present for R/C: the other path (source for rename/copy).
    pub(super) other: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct GitCommit {
    pub(super) changes: Vec<GitChange>,
}

/// Spawn system git; any failure/timeout/cap/non-git → Unavailable.
pub(super) fn read_git_history(root: &Path) -> HistoryAvailability {
    let mut cmd = std::process::Command::new("git");
    cmd.args(["-c", "core.quotepath=false", "-C"])
        .arg(root)
        .args(["log", "HEAD", "--no-merges", "--topo-order"])
        .arg(format!("--max-count={CO_CHANGE_MAX_COMMITS}"))
        .args([
            "--find-renames=50%",
            "--find-copies=50%",
            "--no-ext-diff",
            "--no-textconv",
            "--name-status",
            "-z",
            "--format=%x1e%H%x00",
            "--relative",
            "--",
            ".",
        ])
        .env("LC_ALL", "C")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return HistoryAvailability::Unavailable,
    };

    // Byte-cap via stdout filter: once past the cap, discard further chunks and
    // mark the sample unusable (never partial-parse a truncated log).
    let capped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let capped_flag = std::sync::Arc::clone(&capped);
    let mut accepted = 0usize;
    let filter = move |chunk: &[u8]| -> std::io::Result<bool> {
        accepted = accepted.saturating_add(chunk.len());
        if accepted > CO_CHANGE_GIT_OUTPUT_CAP {
            capped_flag.store(true, std::sync::atomic::Ordering::Relaxed);
            return Ok(false);
        }
        Ok(true)
    };

    let waited = child
        .controlled_with_output()
        .time_limit(Duration::from_secs(CO_CHANGE_GIT_TIMEOUT_SECS))
        .stdout_filter(filter)
        .terminate_for_timeout()
        .wait();

    let output = match waited {
        Ok(Some(o)) if o.status.success() => o,
        _ => return HistoryAvailability::Unavailable,
    };
    if capped.load(std::sync::atomic::Ordering::Relaxed)
        || output.stdout.len() > CO_CHANGE_GIT_OUTPUT_CAP
    {
        return HistoryAvailability::Unavailable;
    }
    if output.stdout.is_empty() {
        // Empty success is either unborn HEAD or a repo with no commits.
        return HistoryAvailability::Unavailable;
    }

    match parse_git_name_status_z(&output.stdout) {
        Some(commits) if !commits.is_empty() => {
            HistoryAvailability::Available(GitHistory { commits })
        }
        _ => HistoryAvailability::Unavailable,
    }
}

/// NUL-safe parser for `git log -z --name-status --format=%x1e%H%x00`.
/// Commit marker `\x1e` is recognized only at record boundaries (after a NUL
/// or at the start). Paths may contain newlines; non-UTF-8 paths are skipped.
pub(super) fn parse_git_name_status_z(bytes: &[u8]) -> Option<Vec<GitCommit>> {
    // Records are NUL-terminated fields. A commit starts with 0x1e + hash,
    // then a NUL, then zero or more name-status records each ending in NUL.
    // Status forms:
    //   M\0path\0 | A\0path\0 | D\0path\0 | T\0path\0
    //   R###\0old\0new\0 | C###\0old\0new\0
    let mut commits: Vec<GitCommit> = Vec::new();
    let mut i = skip_leading_nuls(bytes, 0);
    let n = bytes.len();

    while i < n {
        match advance_to_commit_marker(bytes, i) {
            Some(at_marker) => i = at_marker,
            None => break,
        }
        let after_hash = match parse_commit_hash(bytes, i) {
            Some(parsed) => parsed,
            None => {
                // One malformed hash record must not discard the whole sample:
                // step past this marker and resync on the next commit boundary.
                i += 1;
                continue;
            }
        };
        i = after_hash;
        let (changes, after_changes) = parse_commit_changes(bytes, i);
        i = after_changes;
        commits.push(GitCommit { changes });
        if commits.len() >= CO_CHANGE_MAX_COMMITS {
            break;
        }
    }

    Some(commits)
}

fn skip_leading_nuls(bytes: &[u8], mut i: usize) -> usize {
    let n = bytes.len();
    while i < n && bytes[i] == 0 {
        i += 1;
    }
    i
}

/// Find the next 0x1e commit marker at a record boundary; returns index of 0x1e.
fn advance_to_commit_marker(bytes: &[u8], mut i: usize) -> Option<usize> {
    let n = bytes.len();
    while i < n {
        if bytes[i] == 0x1e {
            return Some(i);
        }
        // Not a commit start — skip to next NUL boundary and retry.
        while i < n && bytes[i] != 0 {
            i += 1;
        }
        if i < n {
            i += 1; // consume NUL
        }
    }
    None
}

/// Validate and skip the hash after a commit marker at `i`.
/// Empty/non-UTF-8 hash → None (malformed).
fn parse_commit_hash(bytes: &[u8], mut i: usize) -> Option<usize> {
    let n = bytes.len();
    i += 1; // skip 0x1e
    let hash_start = i;
    while i < n && bytes[i] != 0 {
        i += 1;
    }
    if i == hash_start || std::str::from_utf8(&bytes[hash_start..i]).is_err() {
        return None;
    }
    if i < n {
        i += 1; // consume hash-trailing NUL
    }
    Some(i)
}

/// Consume name-status records until next 0x1e (at boundary) or EOF.
fn parse_commit_changes(bytes: &[u8], mut i: usize) -> (Vec<GitChange>, usize) {
    let n = bytes.len();
    let mut changes: Vec<GitChange> = Vec::new();
    while i < n {
        if bytes[i] == 0x1e {
            break; // next commit
        }
        if bytes[i] == 0 {
            i += 1;
            continue;
        }
        match parse_one_change(bytes, &mut i) {
            Some(ch) => changes.push(ch),
            None => {
                // skipped non-UTF-8 / incomplete record; cursor already advanced
            }
        }
    }
    (changes, i)
}

/// Parse one status + path record at `i`. Advances `i`. Returns None when the
/// record is skipped (non-UTF-8 / incomplete) without aborting the commit.
fn parse_one_change(bytes: &[u8], i: &mut usize) -> Option<GitChange> {
    let n = bytes.len();
    let st_start = *i;
    while *i < n && bytes[*i] != 0 {
        *i += 1;
    }
    let status_bytes = &bytes[st_start..*i];
    if *i < n {
        *i += 1;
    } else {
        return None;
    }
    if status_bytes.is_empty() {
        return None;
    }
    let status_str = match std::str::from_utf8(status_bytes) {
        Ok(s) => s,
        Err(_) => {
            // Skip this record's path fields best-effort: consume one
            // path. Non-UTF-8 status → skip one path.
            skip_nul_field(bytes, i);
            return None;
        }
    };
    let (status, needs_two) = classify_status(status_str)?;
    if needs_two {
        parse_rename_or_copy(bytes, i, status)
    } else {
        let path = read_utf8_nul_field(bytes, i)?;
        Some(GitChange {
            status,
            path,
            other: None,
        })
    }
}

fn classify_status(status_str: &str) -> Option<(GitStatus, bool)> {
    match status_str.as_bytes().first().copied() {
        Some(b'R') => Some((GitStatus::Rename, true)),
        Some(b'C') => Some((GitStatus::Copy, true)),
        Some(b'D') => Some((GitStatus::Delete, false)),
        Some(b'M') | Some(b'A') | Some(b'T') | Some(_) => Some((GitStatus::Modify, false)),
        None => None,
    }
}

fn parse_rename_or_copy(bytes: &[u8], i: &mut usize, status: GitStatus) -> Option<GitChange> {
    let old = match read_utf8_nul_field(bytes, i) {
        Some(p) => p,
        None => {
            // non-UTF-8 or missing: skip the partner too if present
            skip_nul_field(bytes, i);
            return None;
        }
    };
    let newp = read_utf8_nul_field(bytes, i)?;
    Some(GitChange {
        status,
        path: newp,
        other: Some(old),
    })
}

fn read_utf8_nul_field(bytes: &[u8], i: &mut usize) -> Option<String> {
    let n = bytes.len();
    if *i >= n {
        return None;
    }
    // A 0x1e at boundary means we've hit the next commit — no field.
    if bytes[*i] == 0x1e {
        return None;
    }
    let start = *i;
    while *i < n && bytes[*i] != 0 {
        *i += 1;
    }
    let slice = &bytes[start..*i];
    if *i < n && bytes[*i] == 0 {
        *i += 1;
    }
    if slice.is_empty() {
        return None;
    }
    std::str::from_utf8(slice).ok().map(|s| s.to_string())
}

fn skip_nul_field(bytes: &[u8], i: &mut usize) {
    let n = bytes.len();
    while *i < n && bytes[*i] != 0 && bytes[*i] != 0x1e {
        *i += 1;
    }
    if *i < n && bytes[*i] == 0 {
        *i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_handles_nul_framing_newline_path_and_skips_non_utf8() {
        let mut bytes = Vec::new();
        // commit h1: M a.rs, M "x\ny.rs"
        bytes.push(0x1e);
        bytes.extend(b"h1");
        bytes.push(0);
        bytes.extend(b"M");
        bytes.push(0);
        bytes.extend(b"a.rs");
        bytes.push(0);
        bytes.extend(b"M");
        bytes.push(0);
        bytes.extend(
            b"x
y.rs",
        );
        bytes.push(0);
        // commit h2: M non-utf8 (skip), M b.rs
        bytes.push(0x1e);
        bytes.extend(b"h2");
        bytes.push(0);
        bytes.extend(b"M");
        bytes.push(0);
        bytes.extend([0xff, 0xfe, b'z']);
        bytes.push(0);
        bytes.extend(b"M");
        bytes.push(0);
        bytes.extend(b"b.rs");
        bytes.push(0);
        // commit h3: R100 old.rs new.rs
        bytes.push(0x1e);
        bytes.extend(b"h3");
        bytes.push(0);
        bytes.extend(b"R100");
        bytes.push(0);
        bytes.extend(b"old.rs");
        bytes.push(0);
        bytes.extend(b"new.rs");
        bytes.push(0);

        let parsed = parse_git_name_status_z(&bytes).expect("parse");
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].changes.len(), 2);
        assert_eq!(parsed[0].changes[0].path, "a.rs");
        assert_eq!(
            parsed[0].changes[1].path,
            "x
y.rs"
        );
        assert_eq!(parsed[1].changes.len(), 1);
        assert_eq!(parsed[1].changes[0].path, "b.rs");
        assert_eq!(parsed[2].changes.len(), 1);
        assert_eq!(parsed[2].changes[0].status, GitStatus::Rename);
        assert_eq!(parsed[2].changes[0].path, "new.rs");
        assert_eq!(parsed[2].changes[0].other.as_deref(), Some("old.rs"));
    }

    #[test]
    fn a_malformed_commit_record_is_skipped_not_fatal_to_the_sample() {
        let mut bytes = Vec::new();
        // commit h1: M a.rs
        bytes.push(0x1e);
        bytes.extend(b"h1");
        bytes.push(0);
        bytes.extend(b"M");
        bytes.push(0);
        bytes.extend(b"a.rs");
        bytes.push(0);
        // malformed: a commit marker with an empty hash (0x1e then NUL).
        bytes.push(0x1e);
        bytes.push(0);
        // commit h2: M b.rs
        bytes.push(0x1e);
        bytes.extend(b"h2");
        bytes.push(0);
        bytes.extend(b"M");
        bytes.push(0);
        bytes.extend(b"b.rs");
        bytes.push(0);

        let parsed = parse_git_name_status_z(&bytes).expect("parse");
        assert_eq!(
            parsed.len(),
            2,
            "the malformed record must be skipped, keeping both good commits: {parsed:?}"
        );
        assert_eq!(parsed[0].changes[0].path, "a.rs");
        assert_eq!(parsed[1].changes[0].path, "b.rs");
    }
}
