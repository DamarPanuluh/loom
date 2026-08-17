//! Proof-harness lock — serializes proof EXECUTION across loom processes.
//!
//! The graph lock serializes writes; nothing serialized two runs sharing
//! ports, databases, and processes — and the collisions minted FALSE failing
//! verdicts, the tool manufacturing the very smells it audits. Any command
//! that executes a proof (`validation run`, `journey run|freeze|diagnose`,
//! `observe`) takes this advisory lock first; a second executor refuses
//! immediately with the holder's identity instead of racing it.
//!
//! Re-entrancy: the holding thread records the lock path in the thread-local
//! [`HELD`] set, so a re-entering call on the same call stack (`journey run`
//! settling through its own `journey resume`) proceeds without re-locking —
//! the outer run owns serialization. A child *process* does not inherit the
//! claim: it is refused by the file lock and attests contention over
//! `LOOM_CONTENTION_FD` so the parent records Blocked, not a failing proof.
//! A different repo keys a different path and still contends.

use crate::Result;
use anyhow::Context;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

/// Marker inside the contention error so `main` maps it to the contention
/// exit code (an infrastructure block, never a proof failure).
pub const HARNESS_CONTENTION_MARKER: &str = "loom-harness-contention";

thread_local! {
    /// Lock paths this thread already holds.
    ///
    /// Re-entrancy is a property of one call stack — `journey run` taking the
    /// lock and its own `journey resume` re-entering it — so it is tracked per
    /// thread. It used to be a process-global environment variable, which is
    /// shared by every thread in the process: a sibling thread locking an
    /// unrelated root overwrote the marker, the first thread then failed its
    /// own re-entrancy check, tried to file-lock a path it was already holding,
    /// and was refused by itself. `cargo test` runs tests as threads in one
    /// process, so any suite driving loom in parallel could hit it; it took
    /// down a release code gate. The variable was also never cleared (there was
    /// no `Drop`), and nothing inherited it — children are spawned with
    /// `env_clear()` and a `PATH`/`TMPDIR` allowlist — so it bought nothing.
    ///
    /// A *different* thread wanting the same lock is genuine concurrent use and
    /// is still refused, because it holds no entry here and meets the file lock.
    static HELD: RefCell<BTreeSet<String>> = const { RefCell::new(BTreeSet::new()) };
}

fn held_by_this_thread(key: &str) -> bool {
    HELD.with(|held| held.borrow().contains(key))
}

/// Holds the harness lock; dropping releases it.
pub struct HarnessGuard {
    _file: Option<File>,
    /// Present only on the guard that actually took the lock, so a re-entrant
    /// guard dropping cannot release the outer one's claim.
    key: Option<String>,
}

impl Drop for HarnessGuard {
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            HELD.with(|held| held.borrow_mut().remove(&key));
        }
    }
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
    if held_by_this_thread(&key) {
        // Already serialized further up this call stack.
        return Ok(HarnessGuard {
            _file: None,
            key: None,
        });
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
    if let Err(error) = f.set_len(0) {
        eprintln!(
            "warning: could not clear harness holder identity at {}: {error}",
            path.display()
        );
    }
    if let Err(error) = f.write_all(holder.to_string().as_bytes()) {
        eprintln!(
            "warning: could not record harness holder identity at {}: {error}",
            path.display()
        );
    }
    HELD.with(|held| held.borrow_mut().insert(key.clone()));
    Ok(HarnessGuard {
        _file: Some(file),
        key: Some(key),
    })
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

    fn temp_root(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("loom-harness-test-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(dir.join(crate::LOOM_DIR)).unwrap();
        dir
    }

    #[test]
    fn second_executor_is_refused_with_the_holders_identity() {
        let root = temp_root("contention");
        let identity = crate::identity::ExecutionIdentity::solo();
        let guard = acquire(&root, "test one", &identity).unwrap();

        // A competing agent is another call stack, so contend from a real one
        // rather than by clearing a marker this thread owns.
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let err = acquire(&root, "test two", &identity).unwrap_err();
                let msg = format!("{err:#}");
                assert!(msg.contains(HARNESS_CONTENTION_MARKER), "unmarked: {msg}");
                assert!(msg.contains("test one"), "holder purpose missing: {msg}");
            });
        });

        drop(guard);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                acquire(&root, "test three", &identity)
                    .expect("released lock must be re-acquirable");
            });
        });
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_nested_step_of_the_holder_proceeds_without_relocking() {
        let root = temp_root("nested");
        let identity = crate::identity::ExecutionIdentity::solo();
        let guard = acquire(&root, "outer", &identity).unwrap();
        acquire(&root, "nested step", &identity).expect("nested executor must proceed");
        drop(guard);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unrelated_lock_on_another_thread_does_not_revoke_re_entrancy() {
        let identity = crate::identity::ExecutionIdentity::solo();
        let mine = temp_root("thread-self");
        let theirs = temp_root("thread-other");

        let outer = acquire(&mine, "journey run", &identity).unwrap();

        // A sibling thread locks an unrelated root. `cargo test` runs tests as
        // threads in ONE process, so anything process-global it touches is
        // shared with this thread's in-flight guard.
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let _theirs = acquire(&theirs, "unrelated run", &identity).unwrap();
            });
        });

        // The two-phase run -> resume pattern re-enters the lock this thread
        // already holds. Being refused here means a process was refused by its
        // own lock — which is what took down a release code gate.
        acquire(&mine, "journey resume", &identity)
            .expect("a thread must not be refused by the harness lock it already holds");

        drop(outer);
        let _ = std::fs::remove_dir_all(&mine);
        let _ = std::fs::remove_dir_all(&theirs);
    }
}
