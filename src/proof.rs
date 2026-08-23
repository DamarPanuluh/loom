//! Proof-runner seam — validation types resolve to registered runners.
//!
//! Plane: execution behind a seam. The engine records a proof's outcome
//! uniformly through [`ProofOutcome`] without knowing how a shell test command
//! differs from a manual check or a journey. Each [`ValidationType`] resolves to
//! a [`ProofRunner`] via [`runner_for`]; adding a new proof mechanism is adding a
//! runner, not editing the recorder in `commands::proof_cmd`.

use crate::model::{Node, ValidationType};
use crate::text::ellipsize;
use crate::subprocess::Captured;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(crate) const VALIDATION_OUTPUT_EXCERPT_BYTES: usize = 8192;

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

/// The child authority policy is part of execution identity. Two otherwise
/// identical commands must not share an observation if one runs as a hermetic
/// Solo fixture and the other inherits the validator lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum CommandEnvironment {
    Inherit,
    SoloTestFixture,
}

/// One immutable command observation plan.
///
/// The plan doubles as the within-batch execution fingerprint: it contains
/// every input that can change what Loom observes. Command text is deliberately
/// exact rather than shell-normalized; an avoidable duplicate execution is
/// safer than sharing two observations that merely look equivalent.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommandExecutionPlan {
    root: PathBuf,
    command: String,
    timeout_secs: u64,
    environment: CommandEnvironment,
}

impl CommandExecutionPlan {
    fn run(self) -> ProofOutcome {
        let started = std::time::Instant::now();
        match run_validation_command(&self) {
            Ok(Some(o)) if o.status.success() => ProofOutcome::Passed {
                evidence: format!("`{}` exit 0", self.command),
                run: Box::new(observation(&self.root, &self.command, &o, started)),
            },
            Ok(Some(o)) => {
                let code = i64::from(o.status.code().unwrap_or(-1));
                let output = validation_output_json(&o);
                let run = Box::new(observation(&self.root, &self.command, &o, started));
                let stderr_excerpt = output["stderr"].as_str().unwrap_or("").trim();
                let stdout_excerpt = output["stdout"].as_str().unwrap_or("").trim();
                let excerpt = if stderr_excerpt.is_empty() {
                    stdout_excerpt
                } else {
                    stderr_excerpt
                };
                let evidence = if excerpt.is_empty() {
                    format!("`{}` exit {code}", self.command)
                } else {
                    format!(
                        "`{}` exit {code}; output: {}",
                        self.command,
                        ellipsize(excerpt.trim(), 300)
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
                reason: format!(
                    "killed: `{}` exceeded timeout_secs={}",
                    self.command, self.timeout_secs
                ),
            },
            Err(e) => ProofOutcome::Blocked {
                reason: format!("could not run: {e}"),
            },
        }
    }
}

/// A proof after all validation-specific inputs have been resolved. Immediate
/// outcomes have no subprocess to share; command plans may be cached for the
/// lifetime of one batch invocation.
#[derive(Debug, Clone)]
pub enum PreparedProof {
    Immediate(ProofOutcome),
    Command(CommandExecutionPlan),
}

impl PreparedProof {
    pub(crate) fn execution_plan(&self) -> Option<&CommandExecutionPlan> {
        match self {
            Self::Immediate(_) => None,
            Self::Command(plan) => Some(plan),
        }
    }

    pub fn run(self) -> ProofOutcome {
        match self {
            Self::Immediate(outcome) => outcome,
            Self::Command(plan) => plan.run(),
        }
    }
}

/// A proof runner executes one validation and reports a uniform outcome.
pub trait ProofRunner {
    fn run(&self, root: &Path, validation: &Node) -> ProofOutcome;

    /// Resolve an execution plan when this runner supports sharing one observed
    /// subprocess across a batch. The default preserves compatibility for
    /// runners that only implement `run`: they execute immediately and remain
    /// deliberately uncacheable.
    fn prepare(&self, root: &Path, validation: &Node) -> PreparedProof {
        PreparedProof::Immediate(self.run(root, validation))
    }
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
        self.prepare(root, v).run()
    }

    fn prepare(&self, root: &Path, v: &Node) -> PreparedProof {
        let command = v
            .body
            .get("command")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        if command.is_empty() {
            // A command-shaped proof with no command is unrunnable here; leave
            // it for a human verdict rather than fabricating a pass/fail.
            return PreparedProof::Immediate(ProofOutcome::Manual {
                reason: "manual_check".into(),
            });
        }
        if v.body.get("command_trusted").and_then(|v| v.as_bool()) == Some(false) {
            return PreparedProof::Immediate(ProofOutcome::Blocked {
                reason: format!(
                    "imported command is untrusted; review it, then run `loom validation update '{}' --command <reviewed-command>` to approve the exact text locally",
                    v.name
                ),
            });
        }
        // A repo-native cargo test runs with cwd = repository root and an
        // inherited environment. Cargo resolves a RELATIVE cache root against
        // that cwd, so an inherited `CARGO_HOME=relative/path` (or `.`)
        // materializes a full registry inside the tree the proof is about —
        // the release plane refuses exactly this (DependencyCacheGuard), and
        // the proof door must too. Blocked, not failed: the code under proof
        // was never observed.
        if is_repo_native_cargo_test(&command) {
            if let Some(name) = relative_cache_root(&ambient_cache_roots()) {
                return PreparedProof::Immediate(ProofOutcome::Blocked {
                    reason: format!(
                        "cache root environment '{name}' names a relative path; a test-spawned \
                         cargo would resolve it inside the repository — export an absolute \
                         {name} (or unset it for the toolchain default) and re-run"
                    ),
                });
            }
        }
        PreparedProof::Command(CommandExecutionPlan {
            root: root.to_path_buf(),
            timeout_secs: validation_timeout_secs(v),
            environment: validation_environment(&command),
            command,
        })
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
        self.prepare(root, v).run()
    }

    fn prepare(&self, root: &Path, v: &Node) -> PreparedProof {
        CommandProofRunner.prepare(root, v)
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
        i64::from(o.status.code().unwrap_or(-1)),
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
        .unwrap_or(crate::runner::DEFAULT_TIMEOUT_SECS)
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

fn validation_environment(command: &str) -> CommandEnvironment {
    if is_repo_native_cargo_test(command) {
        CommandEnvironment::SoloTestFixture
    } else {
        CommandEnvironment::Inherit
    }
}

/// The declared toolchain cache roots as the child would inherit them.
fn ambient_cache_roots() -> Vec<(&'static str, String)> {
    ["CARGO_HOME", "RUSTUP_HOME"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok().map(|value| (name, value)))
        .collect()
}

/// The first cache-root variable whose value the toolchain would resolve
/// against the child's cwd instead of using as-is. Present-but-relative
/// (including empty and `.`) is the leak vector; absent falls back to the
/// documented `$HOME`-anchored default and is fine.
fn relative_cache_root<'a>(roots: &'a [(&'static str, String)]) -> Option<&'a str> {
    roots
        .iter()
        .find(|(_, value)| !Path::new(value).is_absolute())
        .map(|(name, _)| *name)
}

fn run_validation_command(plan: &CommandExecutionPlan) -> std::io::Result<Option<Captured>> {
    use crate::subprocess::ChildEnvironment;

    // The validator owns the outer observation and verdict. Cargo's test
    // process owns isolated fixture graphs and must not mistake that parent
    // authority for its own. This is deliberately validation-only: generic
    // observe, journey, and scan commands retain their caller environment.
    let environment = match plan.environment {
        CommandEnvironment::Inherit => ChildEnvironment::Inherit,
        CommandEnvironment::SoloTestFixture => ChildEnvironment::SoloTestFixture,
    };
    crate::subprocess::run_with_environment(
        &plan.command,
        &plan.root,
        Duration::from_secs(plan.timeout_secs),
        environment,
    )
}

/// Accept one or more top-level `cargo test` commands joined only by `&&`.
/// Anything else inherits the validator identity and therefore fails closed if
/// it tries to exercise a stronger Loom lane. This keeps the Solo fixture seam
/// away from arbitrary shell tails such as `cargo test && loom ratify ...`.
fn is_repo_native_cargo_test(command: &str) -> bool {
    let mut saw_segment = false;
    for segment in command.split("&&") {
        let segment = segment.trim();
        if segment.is_empty()
            || segment.contains([';', '|', '&', '\n', '\r'])
            || !(segment == "cargo test"
                || segment
                    .strip_prefix("cargo test")
                    .is_some_and(|rest| rest.chars().next().is_some_and(char::is_whitespace)))
        {
            return false;
        }
        saw_segment = true;
    }
    saw_segment
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
    fn relative_cache_root_flags_only_values_cargo_would_resolve_against_cwd() {
        // Relative (including `.` and empty) is the repo-leak vector.
        for value in ["relative/path", ".", ""] {
            let roots = vec![("CARGO_HOME", value.to_string())];
            assert_eq!(
                relative_cache_root(&roots),
                Some("CARGO_HOME"),
                "{value:?} must be refused"
            );
        }
        // Absolute roots and an absent variable are fine.
        let roots = vec![
            ("CARGO_HOME", "/tmp/cargo-home".to_string()),
            ("RUSTUP_HOME", "/tmp/rustup-home".to_string()),
        ];
        assert_eq!(relative_cache_root(&roots), None);
        assert_eq!(relative_cache_root(&[]), None);
        // The first offender is named, in declaration order.
        let roots = vec![
            ("CARGO_HOME", "/tmp/cargo-home".to_string()),
            ("RUSTUP_HOME", "rustup-home".to_string()),
        ];
        assert_eq!(relative_cache_root(&roots), Some("RUSTUP_HOME"));
    }

    #[test]
    fn validation_timeout_uses_runner_default_and_accepts_positive_override() {
        let mut validation = val("test", "true");
        assert_eq!(
            validation_timeout_secs(&validation),
            crate::runner::DEFAULT_TIMEOUT_SECS
        );

        validation.body["timeout_seconds"] = serde_json::json!(42);
        assert_eq!(validation_timeout_secs(&validation), 42);

        validation.body["timeout_seconds"] = serde_json::json!(0);
        assert_eq!(
            validation_timeout_secs(&validation),
            crate::runner::DEFAULT_TIMEOUT_SECS
        );
    }

    #[test]
    fn prepared_command_plan_keys_every_execution_distinction() {
        let root = std::env::temp_dir();
        let base = val("test", "true");
        let base_plan = runner_for(ValidationType::Test)
            .prepare(&root, &base)
            .execution_plan()
            .expect("command proof has an execution plan")
            .clone();

        // Journey delegates to the same command mechanism today, so the exact
        // same effective plan is safe to share despite the superficial type.
        let journey_plan = runner_for(ValidationType::Journey)
            .prepare(&root, &val("journey", "true"))
            .execution_plan()
            .unwrap()
            .clone();
        assert_eq!(base_plan, journey_plan);

        let mut slower = base.clone();
        slower.body["timeout_seconds"] = serde_json::json!(42);
        let slower_plan = runner_for(ValidationType::Test)
            .prepare(&root, &slower)
            .execution_plan()
            .unwrap()
            .clone();
        assert_ne!(base_plan, slower_plan, "timeout changes the observation");

        let spaced_plan = runner_for(ValidationType::Test)
            .prepare(&root, &val("test", " true"))
            .execution_plan()
            .unwrap()
            .clone();
        assert_ne!(
            base_plan, spaced_plan,
            "exact command text is preserved rather than shell-normalized"
        );

        assert_eq!(
            validation_environment("cargo test --test ring5 -q"),
            CommandEnvironment::SoloTestFixture
        );
        assert_eq!(validation_environment("true"), CommandEnvironment::Inherit);
    }

    #[test]
    fn cargo_test_fixture_policy_rejects_shell_tails_and_lookalikes() {
        for accepted in [
            "cargo test",
            "cargo test -q",
            "  cargo test --test ring1",
            "cargo test --lib store::tests -q && cargo test --test ring8 -q",
        ] {
            assert!(is_repo_native_cargo_test(accepted), "{accepted}");
        }
        for rejected in [
            "cargo",
            "cargo build",
            "cargo testevil",
            "echo cargo test",
            "LOOM_AGENT=solo cargo test",
            "cargo test && loom intent ratify x",
            "cargo test; loom intent ratify x",
            "cargo test | tee result",
        ] {
            assert!(!is_repo_native_cargo_test(rejected), "{rejected}");
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
