//! Proof-runner seam — validation types resolve to registered runners.
//!
//! Plane: execution behind a seam. The engine records a proof's outcome
//! uniformly through [`ProofOutcome`] without knowing how a shell test command
//! differs from a manual check or a journey. Each [`ValidationType`] resolves to
//! a [`ProofRunner`] via [`runner_for`]; adding a new proof mechanism is adding a
//! runner, not editing the recorder in `commands::proof_cmd`.

use crate::commands::truncate;
use crate::model::{Node, ValidationType};
use crate::subprocess::Captured;
use std::path::Path;
use std::time::Duration;

const DEFAULT_VALIDATION_TIMEOUT_SECS: u64 = 300;
const VALIDATION_OUTPUT_EXCERPT_BYTES: usize = 8192;

/// The uniform result the engine records, whatever ran the proof. The recorder
/// maps each variant to a validation status + evidence identically across
/// runners — that uniformity is the point of the seam.
#[derive(Debug, Clone)]
pub enum ProofOutcome {
    /// The proof passed. Carries the RECORD of the run loom performed — not a
    /// sentence about it. This is the difference between "54 of 59 proofs
    /// passed" and "an agent wrote that 54 of 59 proofs passed".
    Passed {
        evidence: String,
        run: Box<crate::evidence::RunRecord>,
    },
    /// The proof failed; carries the row detail the recorder surfaces.
    Failed {
        evidence: String,
        exit_code: i64,
        output: serde_json::Value,
        run: Box<crate::evidence::RunRecord>,
    },
    /// The proof could not complete (timeout, spawn error, or a missing
    /// prerequisite the runner names). Honest, and never forgotten.
    Blocked { reason: String },
    /// No automated runner applies here — a human records the verdict.
    Manual { reason: String },
}

/// A proof runner executes one validation and reports a uniform outcome.
pub trait ProofRunner {
    fn run(&self, root: &Path, validation: &Node) -> ProofOutcome;
}

/// Resolve the runner for a validation type. Total by construction: every
/// [`ValidationType`] maps to exactly one runner, and command-shaped proofs
/// (test/assertion/benchmark/contract/scenario) share the command runner.
pub fn runner_for(ty: ValidationType) -> Box<dyn ProofRunner> {
    match ty {
        ValidationType::ManualCheck => Box::new(ManualProofRunner),
        ValidationType::Journey => Box::new(JourneyProofRunner),
        ValidationType::Test
        | ValidationType::Assertion
        | ValidationType::Benchmark
        | ValidationType::Scenario
        | ValidationType::Contract => Box::new(CommandProofRunner),
    }
}

/// Runs a validation's shell `command` as a subprocess and maps its exit status.
pub struct CommandProofRunner;

impl ProofRunner for CommandProofRunner {
    fn run(&self, root: &Path, v: &Node) -> ProofOutcome {
        let command = v
            .body
            .get("command")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        if command.is_empty() {
            // A command-shaped proof with no command is unrunnable here; leave
            // it for a human verdict rather than fabricating a pass/fail.
            return ProofOutcome::Manual {
                reason: "manual_check".into(),
            };
        }
        if v.body.get("command_trusted").and_then(|v| v.as_bool()) == Some(false) {
            return ProofOutcome::Blocked {
                reason: format!(
                    "imported command is untrusted; review it, then run `loom validation update '{}' --command <reviewed-command>` to approve the exact text locally",
                    v.name
                ),
            };
        }
        let timeout_secs = validation_timeout_secs(v);
        let started = std::time::Instant::now();
        match run_validation_command(root, &command, timeout_secs) {
            Ok(Some(o)) if o.status.success() => ProofOutcome::Passed {
                evidence: format!("`{command}` exit 0"),
                run: Box::new(observation(root, &command, &o, started)),
            },
            Ok(Some(o)) => {
                let code = o.status.code().unwrap_or(-1);
                let output = validation_output_json(&o);
                let run = Box::new(observation(root, &command, &o, started));
                let stderr_excerpt = output["stderr"].as_str().unwrap_or("").trim();
                let stdout_excerpt = output["stdout"].as_str().unwrap_or("").trim();
                let excerpt = if stderr_excerpt.is_empty() {
                    stdout_excerpt
                } else {
                    stderr_excerpt
                };
                let evidence = if excerpt.is_empty() {
                    format!("`{command}` exit {code}")
                } else {
                    format!(
                        "`{command}` exit {code}; output: {}",
                        truncate(excerpt, 300)
                    )
                };
                ProofOutcome::Failed {
                    evidence,
                    exit_code: code,
                    output,
                    run,
                }
            }
            Ok(None) => ProofOutcome::Blocked {
                reason: format!("`{command}` timed out after {timeout_secs}s"),
            },
            Err(e) => ProofOutcome::Blocked {
                reason: format!("could not run: {e}"),
            },
        }
    }
}

/// A manual check has no automated proof — it always resolves to a human verdict.
pub struct ManualProofRunner;

impl ProofRunner for ManualProofRunner {
    fn run(&self, _root: &Path, _v: &Node) -> ProofOutcome {
        ProofOutcome::Manual {
            reason: "manual_check".into(),
        }
    }
}

/// A journey proof is executed by the journey subsystem (`loom journey run`),
/// which needs a base URL and env this recorder cannot supply — so within
/// `loom validation run` a journey resolves to its dedicated runner rather than
/// being run inline.
pub struct JourneyProofRunner;

impl ProofRunner for JourneyProofRunner {
    /// A journey proof IS runnable — it names a command, and refusing to run it
    /// meant every journey proof in the graph got its status from somewhere
    /// other than loom watching it. That is the gap this whole spine exists to
    /// close, so the stub is gone: the command runs, and what loom observes is
    /// what gets recorded.
    ///
    /// A journey with no command still falls through to `Manual` — the honest
    /// answer when there is nothing to execute.
    fn run(&self, root: &Path, v: &Node) -> ProofOutcome {
        CommandProofRunner.run(root, v)
    }
}

/// Turn an observed subprocess into a [`crate::evidence::RunRecord`].
///
/// `covered` is deliberately left for the caller to fill from the intents this
/// proof validates: a run anchors the code it exercised, and the caller is the
/// only layer that knows which files those are.
fn observation(
    root: &Path,
    command: &str,
    o: &Captured,
    started: std::time::Instant,
) -> crate::evidence::RunRecord {
    crate::runner::record(
        root,
        crate::model::RunProducer::Command,
        command,
        &[],
        0,
        o.status.code().unwrap_or(-1),
        &o.stdout,
        &o.stderr,
        started.elapsed().as_millis() as u64,
    )
}

/// The configured (or default) subprocess timeout for a validation.
fn validation_timeout_secs(v: &Node) -> u64 {
    v.body
        .get("timeout_seconds")
        .and_then(|value| value.as_u64())
        .filter(|secs| *secs > 0)
        .unwrap_or(DEFAULT_VALIDATION_TIMEOUT_SECS)
}

/// Excerpt bounded output for the JSON record. `total` is the TRUE byte count
/// the stream emitted (which may exceed the retained buffer), so truncation is
/// reported honestly even though only a window is kept.
fn output_excerpt(bytes: &[u8], total: usize) -> (String, usize, bool) {
    let take = bytes.len().min(VALIDATION_OUTPUT_EXCERPT_BYTES);
    let excerpt = String::from_utf8_lossy(&bytes[..take]).to_string();
    (excerpt, total, total > take)
}

fn validation_output_json(o: &Captured) -> serde_json::Value {
    let (stdout, stdout_bytes, stdout_truncated) = output_excerpt(&o.stdout, o.stdout_total);
    let (stderr, stderr_bytes, stderr_truncated) = output_excerpt(&o.stderr, o.stderr_total);
    serde_json::json!({
        "stdout": stdout,
        "stdout_bytes": stdout_bytes,
        "stdout_truncated": stdout_truncated,
        "stderr": stderr,
        "stderr_bytes": stderr_bytes,
        "stderr_truncated": stderr_truncated,
    })
}

fn run_validation_command(
    root: &Path,
    command: &str,
    timeout_secs: u64,
) -> std::io::Result<Option<Captured>> {
    crate::subprocess::run(command, root, Duration::from_secs(timeout_secs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NodeType, TruthClass};

    fn val(ty: &str, command: &str) -> Node {
        Node {
            id: "v1".into(),
            node_type: NodeType::Validation,
            name: "proof".into(),
            description: String::new(),
            status: "not_run".into(),
            truth_class: TruthClass::Asserted,
            body: serde_json::json!({ "type": ty, "command": command }),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn manual_and_journey_types_resolve_to_non_command_runners() {
        let root = std::env::temp_dir();
        match runner_for(ValidationType::ManualCheck).run(&root, &val("manual_check", "")) {
            ProofOutcome::Manual { reason } => assert_eq!(reason, "manual_check"),
            o => panic!("manual_check should be Manual, got {o:?}"),
        }
        // A journey proof with NO command has nothing to execute, so it is
        // honestly Manual. One WITH a command is run like any other — the stub
        // that refused to run journeys is why every journey proof in loom's own
        // graph got its status from somewhere other than loom watching it.
        match runner_for(ValidationType::Journey).run(&root, &val("journey", "")) {
            ProofOutcome::Manual { reason } => assert_eq!(reason, "manual_check"),
            o => panic!("a commandless journey should be Manual, got {o:?}"),
        }
    }

    #[test]
    fn command_runner_maps_exit_status() {
        let root = std::env::temp_dir();
        let runner = runner_for(ValidationType::Test);
        match runner.run(&root, &val("test", "true")) {
            ProofOutcome::Passed { evidence, run } => {
                assert!(evidence.contains("exit 0"));
                assert_eq!(run.exit_code, 0, "loom observed the run itself");
            }
            o => panic!("`true` should pass, got {o:?}"),
        }
        match runner.run(&root, &val("test", "exit 7")) {
            ProofOutcome::Failed { exit_code, .. } => assert_eq!(exit_code, 7),
            o => panic!("`exit 7` should fail with code 7, got {o:?}"),
        }
        match runner.run(&root, &val("test", "")) {
            ProofOutcome::Manual { .. } => {}
            o => panic!("empty command has no runnable proof, got {o:?}"),
        }
    }

    #[test]
    fn imported_untrusted_command_is_blocked_before_execution() {
        let root = std::env::temp_dir();
        let mut validation = val("test", "exit 0");
        validation.body["command_trusted"] = serde_json::Value::Bool(false);
        match runner_for(ValidationType::Test).run(&root, &validation) {
            ProofOutcome::Blocked { reason } => {
                assert!(reason.contains("imported command is untrusted"));
                assert!(reason.contains("validation update"));
            }
            outcome => panic!("untrusted command must be blocked, got {outcome:?}"),
        }
    }
}
