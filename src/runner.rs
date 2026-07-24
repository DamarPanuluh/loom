//! Runner — the ONLY place a [`RunRecord`] comes into existence.
//!
//! Plane: observation. Everything here executes something and reports what it
//! saw; nothing here accepts an outcome from a caller.
//!
//! Contract: `Run` evidence means *loom did this and watched*. That is the whole
//! difference between "54 of 59 proofs passed" and "an agent wrote a sentence
//! saying 54 of 59 proofs passed". Because [`crate::evidence::CitedEvidence`]
//! has no `Run` variant, there is no path from caller input to this type — the
//! guarantee is structural, not procedural.
//!
//! Every run captures its `covered` set: the file → content-hash map in force
//! when it ran. That is the anchor. When any covered file changes, the run
//! expires and whatever it justified re-opens — a proof stops counting the
//! moment the code beneath it moves.

use crate::evidence::RunRecord;
use crate::model::RunProducer;
use crate::store::Store;
use crate::Result;
use process_control::{ChildExt, Control};
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

/// Bytes of each stream kept for humans. The FULL stream is fingerprinted.
const EXCERPT_BYTES: usize = 8192;

/// Default wall-clock limit for an observed command.
pub const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// What a run observed, before it is bound to a fact.
pub enum Observation {
    Ran(Box<RunRecord>),
    /// Could not run — a missing command, an untrusted import, a timeout. This
    /// is an honest outcome and is recorded as such; it is never a pass.
    Blocked {
        reason: String,
    },
}

/// Execute `command` at the graph root and record what happened.
///
/// `covered` names the files this run's result depends on; their current hashes
/// are captured so the run expires when any of them changes. `assertions` is how
/// many content checks the run actually made — zero means it only proved the
/// process exited, which is liveness, not behavior.
pub fn observe_command(
    root: &Path,
    producer: RunProducer,
    command: &str,
    covered: &[String],
    assertions: usize,
    timeout_secs: u64,
) -> Result<Observation> {
    if command.trim().is_empty() {
        return Ok(Observation::Blocked {
            reason: "no command to run — a manual check must be attested, not inferred".into(),
        });
    }
    let started = Instant::now();
    let child = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let child = match child {
        Ok(c) => c,
        Err(e) => {
            return Ok(Observation::Blocked {
                reason: format!("could not start `{command}`: {e}"),
            })
        }
    };
    let output = child
        .controlled_with_output()
        .time_limit(Duration::from_secs(timeout_secs))
        .terminate_for_timeout()
        .wait();
    let output = match output {
        Ok(Some(o)) => o,
        // A timeout is not a failure of the behavior — it is a failure to
        // observe, and loom refuses to guess which.
        Ok(None) => {
            return Ok(Observation::Blocked {
                reason: format!("`{command}` timed out after {timeout_secs}s"),
            })
        }
        Err(e) => {
            return Ok(Observation::Blocked {
                reason: format!("`{command}` could not be observed: {e}"),
            })
        }
    };
    Ok(Observation::Ran(Box::new(record(
        root,
        producer,
        command,
        covered,
        assertions,
        output.status.code().unwrap_or(-1) as i64,
        &output.stdout,
        &output.stderr,
        started.elapsed().as_millis() as u64,
    ))))
}

/// Build a record from an already-observed result. Used by probes that do their
/// own work in-process (locator resolution, pattern pre-screens) rather than
/// spawning — the point is the same: loom looked, loom reports.
#[allow(clippy::too_many_arguments)]
pub fn record(
    root: &Path,
    producer: RunProducer,
    command: &str,
    covered: &[String],
    assertions: usize,
    exit_code: i64,
    stdout: &[u8],
    stderr: &[u8],
    duration_ms: u64,
) -> RunRecord {
    RunRecord {
        producer,
        command: command.to_string(),
        cwd: String::new(),
        exit_code,
        stdout_hash: crate::artifact::fingerprint(&String::from_utf8_lossy(stdout)),
        stderr_hash: crate::artifact::fingerprint(&String::from_utf8_lossy(stderr)),
        stdout_excerpt: excerpt(stdout),
        stderr_excerpt: excerpt(stderr),
        covered: covered_hashes(root, covered),
        assertions,
        duration_ms,
        ran_at: crate::journal::now_iso(),
        loom_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Hash every covered file as it stands right now. A file that cannot be read
/// is recorded with an empty hash so its later appearance still counts as a
/// change — absence is a state, not a gap.
fn covered_hashes(root: &Path, files: &[String]) -> std::collections::BTreeMap<String, String> {
    files
        .iter()
        .map(|f| {
            let hash = std::fs::read_to_string(root.join(f))
                .map(|c| crate::artifact::fingerprint(&c))
                .unwrap_or_default();
            (f.clone(), hash)
        })
        .collect()
}

/// Whether every file a run covered still hashes to what it did at run time.
pub fn covered_intact(root: &Path, run: &RunRecord) -> Option<String> {
    for (file, hash) in &run.covered {
        let current = std::fs::read_to_string(root.join(file))
            .map(|c| crate::artifact::fingerprint(&c))
            .unwrap_or_default();
        if &current != hash {
            return Some(file.clone());
        }
    }
    None
}

fn excerpt(bytes: &[u8]) -> String {
    let take = bytes.len().min(EXCERPT_BYTES);
    String::from_utf8_lossy(&bytes[..take]).to_string()
}

/// The files a run over this intent's code depends on: every file grounded to
/// it. Used to build the `covered` set for a proof.
pub fn files_grounding(store: &Store, intent_id: &str) -> Result<Vec<String>> {
    let mut files = Vec::new();
    for e in store.edges_with(
        Some(crate::model::EdgeKind::Implements),
        Some(intent_id),
        None,
    )? {
        if store.edge_superseded(&e.id)? {
            continue;
        }
        if let Some(cf) = store.get_node(&e.to_id)? {
            files.push(cf.name);
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_command_is_blocked_not_passed() {
        let tmp = std::env::temp_dir();
        let obs =
            observe_command(&tmp, RunProducer::Command, "   ", &[], 0, 5).expect("no io failure");
        match obs {
            Observation::Blocked { reason } => assert!(reason.contains("manual check")),
            Observation::Ran(_) => panic!("an empty command must never produce a run record"),
        }
    }

    #[test]
    fn a_run_records_the_hashes_of_what_it_covered() {
        let dir = tempdir();
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        let obs = observe_command(&dir, RunProducer::Command, "true", &["a.txt".into()], 1, 30)
            .expect("no io failure");
        let Observation::Ran(run) = obs else {
            panic!("`true` runs")
        };
        assert_eq!(run.exit_code, 0);
        assert_eq!(run.covered.len(), 1);
        assert!(covered_intact(&dir, &run).is_none(), "nothing changed yet");

        // Edit the covered file: the run no longer describes this codebase.
        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        assert_eq!(covered_intact(&dir, &run).as_deref(), Some("a.txt"));
    }

    fn tempdir() -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!(
            "loom-runner-{}-{}",
            std::process::id(),
            crate::journal::now_iso().replace([':', '.', '-'], "")
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }
}
