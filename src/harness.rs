//! Proof-harness lock — serializes proof EXECUTION across loom processes.
//!
//! The graph lock serializes writes; nothing serialized two runs sharing
//! ports, databases, and processes — and the collisions minted FALSE failing
//! verdicts, the tool manufacturing the very smells it audits. Any command
//! that executes a proof (`validation run`, `journey run|freeze|diagnose`,
//! `observe`) takes this advisory lock first; a second executor refuses
//! immediately with the holder's identity instead of racing it.
//!
//! Re-entrancy: the holder exports [`HELD_ENV`] naming the lock path, so a
//! loom spawned as a child (a journey CLI step that runs `loom validation
//! run`, an observed `loom journey run`) proceeds without re-locking — the
//! outer run owns serialization. A different repo keys a different path and
//! still contends.

use crate::Result;
use anyhow::Context;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

/// Marker inside the contention error so `main` maps it to the contention
/// exit code (an infrastructure block, never a proof failure).
pub const HARNESS_CONTENTION_MARKER: &str = "loom-harness-contention";

/// Set by the holder; children naming the same path skip re-locking.
const HELD_ENV: &str = "LOOM_HARNESS_LOCK";

/// Holds the harness lock; dropping releases it.
pub struct HarnessGuard {
    _file: Option<File>,
}

impl std::fmt::Debug for HarnessGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HarnessGuard(..)")
    }
}

/// Take the proof-harness lock for `root`, or refuse with the holder's
/// identity. Immediate refusal: proof runs are long, so waiting would just
/// delay the same answer — rerun after the holder exits.
pub fn acquire(
    root: &Path,
    purpose: &str,
    identity: &crate::identity::ExecutionIdentity,
) -> Result<HarnessGuard> {
    let path = lock_path(root);
    acquire_at(path, root, purpose, identity)
}

/// Spec-scoped variant for graph-free `journey diagnose`: the shared resource
/// is the service the spec drives, so runs of the SAME spec contend while
/// independent specs proceed in parallel. Lives in the temp dir — a foreign
/// cwd must not gain a `.loom` for a diagnosis that records nothing.
pub fn acquire_for_artifact(
    spec: &Path,
    purpose: &str,
    identity: &crate::identity::ExecutionIdentity,
) -> Result<HarnessGuard> {
    let canonical = spec
        .canonicalize()
        .unwrap_or_else(|_| spec.to_path_buf())
        .to_string_lossy()
        .to_string();
    let path = std::env::temp_dir().join(format!(
        "loom-harness-spec-{}.lock",
        crate::artifact::fingerprint(&canonical)
    ));
    acquire_at(path, spec, purpose, identity)
}

fn acquire_at(
    path: PathBuf,
    root: &Path,
    purpose: &str,
    identity: &crate::identity::ExecutionIdentity,
) -> Result<HarnessGuard> {
    let key = path.to_string_lossy().to_string();
    if std::env::var(HELD_ENV).ok().as_deref() == Some(key.as_str()) {
        // Already serialized by an ancestor loom process.
        return Ok(HarnessGuard { _file: None });
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("opening harness lock {}", path.display()))?;
    if let Err(e) = file.try_lock() {
        match e {
            std::fs::TryLockError::WouldBlock => {
                let holder = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
                let holder_desc = holder
                    .map(|h| {
                        let profile = h
                            .get("profile")
                            .and_then(|v| v.as_str())
                            .map(|p| format!(" / profile {p}"))
                            .unwrap_or_default();
                        format!(
                            "agent {}{} (pid {}, since {}, {})\n  command: {}",
                            h.get("agent").and_then(|v| v.as_str()).unwrap_or("?"),
                            profile,
                            h.get("pid").and_then(|v| v.as_u64()).unwrap_or(0),
                            h.get("started_at").and_then(|v| v.as_str()).unwrap_or("?"),
                            h.get("purpose").and_then(|v| v.as_str()).unwrap_or("?"),
                            h.get("command").and_then(|v| v.as_str()).unwrap_or("?"),
                        )
                    })
                    .unwrap_or_else(|| "another loom process (identity unread)".to_string());
                anyhow::bail!(
                    "{HARNESS_CONTENTION_MARKER}: proof harness already in use by {holder_desc}\n\
                     two concurrent runs share ports, databases, and processes — rerun after it \
                     exits rather than racing it"
                );
            }
            std::fs::TryLockError::Error(e) => {
                return Err(e).with_context(|| format!("locking {}", path.display()));
            }
        }
    }
    // Record who holds it, so the next contender refuses against an identity,
    // not a mystery. Best-effort: the lock itself is the enforcement.
    let holder = serde_json::json!({
        "pid": std::process::id(),
        "agent": identity.actor(),
        "profile": identity.profile(),
        "purpose": purpose,
        "command": std::env::args().collect::<Vec<_>>().join(" "),
        "root": root.to_string_lossy(),
        "started_at": crate::journal::millis_to_iso(
            crate::journal::now_iso().parse::<i64>().unwrap_or(0),
        ),
    });
    let mut f = &file;
    use std::io::Write;
    let _ = f.set_len(0);
    let _ = f.write_all(holder.to_string().as_bytes());
    std::env::set_var(HELD_ENV, key);
    Ok(HarnessGuard { _file: Some(file) })
}

/// `<root>/.loom/harness.lock` when the graph directory exists; otherwise a
/// temp-dir path keyed by the repo root so a graph-less `journey diagnose`
/// still serializes without creating `.loom` in a foreign repo.
fn lock_path(root: &Path) -> PathBuf {
    let loom_dir = root.join(crate::LOOM_DIR);
    if loom_dir.is_dir() {
        return loom_dir.join("harness.lock");
    }
    let canonical = root
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .to_string();
    std::env::temp_dir().join(format!(
        "loom-harness-{}.lock",
        crate::artifact::fingerprint(&canonical)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Both tests mutate process env; serialize them against each other.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn temp_root(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("loom-harness-test-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(dir.join(crate::LOOM_DIR)).unwrap();
        dir
    }

    #[test]
    fn second_executor_is_refused_with_the_holders_identity() {
        let _serialize = ENV_LOCK.lock().unwrap();
        std::env::remove_var(HELD_ENV);
        let root = temp_root("contention");
        let identity = crate::identity::ExecutionIdentity::solo();
        let guard = acquire(&root, "test one", &identity).unwrap();
        std::env::remove_var(HELD_ENV); // act as a peer process, not a child

        let err = acquire(&root, "test two", &identity).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains(HARNESS_CONTENTION_MARKER), "unmarked: {msg}");
        assert!(msg.contains("test one"), "holder purpose missing: {msg}");

        drop(guard);
        acquire(&root, "test three", &identity).expect("released lock must be re-acquirable");
        std::env::remove_var(HELD_ENV);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_child_of_the_holder_proceeds_without_relocking() {
        let _serialize = ENV_LOCK.lock().unwrap();
        std::env::remove_var(HELD_ENV);
        let root = temp_root("nested");
        let identity = crate::identity::ExecutionIdentity::solo();
        let guard = acquire(&root, "outer", &identity).unwrap();
        // acquire exported HELD_ENV naming this path; a spawned loom inherits it.
        acquire(&root, "nested step", &identity).expect("nested executor must proceed");
        drop(guard);
        std::env::remove_var(HELD_ENV);
        let _ = std::fs::remove_dir_all(&root);
    }
}
