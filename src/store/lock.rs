use crate::Result;
use anyhow::{bail, Context};
use std::fs::{File, OpenOptions};
use std::path::Path;

/// Stamped into the lock-contention error so a RUNNER can recognise its own
/// infrastructure failing, rather than attributing it to the code under test.
/// A child blocked on a lock its parent holds exits non-zero exactly like a
/// failing test, and that ambiguity once made loom record a false failing
/// verdict against a behavior that passes.
pub const LOCK_CONTENTION_MARKER: &str = "loom-lock-contention";

/// Wall-clock budget for acquiring an exclusive graph lock before failing with
/// a named contention error. Writers stay fail-fast so competing mutations do
/// not silently queue behind one another. Registered in `loom limits`.
pub(crate) const LOCK_WAIT_BUDGET_MS: u64 = 2_000;

/// Read-only diagnostics are commonly issued immediately after a graph write.
/// Give those commands a longer, still-bounded grace period so a routine
/// `status`/`next` observation does not turn a healthy in-flight write into an
/// EX_TEMPFAIL. Registered in `loom limits`.
pub(crate) const READ_LOCK_WAIT_BUDGET_MS: u64 = 10_000;

/// Statement-level SQLite busy timeout, so brief lock overlap retries inside
/// the store instead of surfacing SQLITE_BUSY.
pub(crate) const SQLITE_BUSY_TIMEOUT_MS: u64 = 5_000;

pub(crate) fn acquire_lock(
    loom_dir: &Path,
    exclusive: bool,
    identity: &crate::identity::ExecutionIdentity,
) -> Result<File> {
    let lock_path = loom_dir.join("lock");
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("opening lock {}", lock_path.display()))?;
    // Retry briefly: a just-dropped lock from a prior open in this or another
    // process can lag a few ms before the OS releases it. WAL + busy_timeout
    // handle real query concurrency; this flock only guards the open boundary
    // (schema migration above all). Writers take it exclusive; a read-only open
    // takes it shared, so N readers proceed together and only wait while a writer
    // actually holds the boundary.
    let (limit_name, budget_ms) = if exclusive {
        ("lock_wait_ms", LOCK_WAIT_BUDGET_MS)
    } else {
        ("read_lock_wait_ms", READ_LOCK_WAIT_BUDGET_MS)
    };
    let budget = std::time::Duration::from_millis(budget_ms);
    let deadline = std::time::Instant::now() + budget;
    let mut wait = std::time::Duration::from_millis(5);
    loop {
        let acquired = if exclusive {
            file.try_lock()
        } else {
            file.try_lock_shared()
        };
        match acquired {
            Ok(()) => {
                record_lock_holder(&file, exclusive, identity);
                return Ok(file);
            }
            // A held lock may release any moment — retry with backoff, but
            // never past the budget: a hang is never an acceptable failure mode.
            Err(std::fs::TryLockError::WouldBlock) => {
                let now = std::time::Instant::now();
                if now + wait >= deadline {
                    break;
                }
                std::thread::sleep(wait);
                if wait < std::time::Duration::from_millis(50) {
                    wait *= 2;
                }
            }
            // A real I/O error will not heal by waiting.
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(e).with_context(|| format!("locking {}", lock_path.display()));
            }
        }
    }
    bail!(
        "{LOCK_CONTENTION_MARKER}: graph lock exceeded {limit_name}={budget_ms} — {} \
         (waiting for {} access); retry after it exits",
        describe_lock_holder(&lock_path),
        if exclusive { "write" } else { "read" }
    )
}

/// Stamp the holder's identity into the lock file so a contender refuses
/// against an identity, not a mystery — the graph-lock counterpart of the
/// harness lock's holder record. Best-effort: the flock itself is the
/// enforcement, and it releases with the holder's process even if this
/// write fails.
fn record_lock_holder(
    file: &File,
    exclusive: bool,
    execution: &crate::identity::ExecutionIdentity,
) {
    use std::io::Write;
    let identity = serde_json::json!({
        "pid": std::process::id(),
        "agent": execution.actor(),
        "profile": execution.profile(),
        "mode": if exclusive { "write" } else { "read" },
        "command": std::env::args().collect::<Vec<_>>().join(" "),
        "since": crate::journal::millis_to_iso(
            crate::journal::now_iso().parse::<i64>().unwrap_or(0),
        ),
    });
    let mut f = file;
    let _ = f.set_len(0);
    let _ = f.write_all(identity.to_string().as_bytes());
}

/// Render the recorded holder for the contention error. The record lags the
/// lock by microseconds (acquire → write) and outlives it (release does not
/// truncate), so name it as the RECORDED holder: with shared read locks it
/// is also just the most recent of possibly several concurrent readers.
fn describe_lock_holder(lock_path: &Path) -> String {
    let parsed = std::fs::read_to_string(lock_path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    match parsed {
        Some(h) => {
            let profile = h
                .get("profile")
                .and_then(|v| v.as_str())
                .map(|p| format!(" / profile {p}"))
                .unwrap_or_default();
            format!(
                "recorded holder is agent {}{} pid {} ({} access, since {})\n  command: {}",
                h.get("agent").and_then(|v| v.as_str()).unwrap_or("?"),
                profile,
                h.get("pid").and_then(|v| v.as_u64()).unwrap_or(0),
                h.get("mode").and_then(|v| v.as_str()).unwrap_or("?"),
                h.get("since").and_then(|v| v.as_str()).unwrap_or("?"),
                h.get("command").and_then(|v| v.as_str()).unwrap_or("?"),
            )
        }
        None => "held by another loom process (identity unread)".to_string(),
    }
}
