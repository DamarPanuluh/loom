//! Ring 18 — local verbatim journey baselines and replay deviations.

use loom::cli::{Cli, Command, JourneyCmd};
use loom::journey::{self, Baseline, Expect, JourneySpec, Step, StepOutcome};
use loom::store::Store;
mod common;
use common::*;

fn spec(command: &str) -> JourneySpec {
    JourneySpec {
        journey: "baseline-demo".into(),
        base: String::new(),
        steps: vec![Step {
            name: "print".into(),
            intent: "demo intent".into(),
            run: command.into(),
            request: Default::default(),
            expect: Expect {
                exit_code: Some(0),
                ..Default::default()
            },
            capture: Default::default(),
        }],
    }
}

#[test]
fn freeze_then_identical_replay_has_no_deviations() {
    let tmp = Tmp::new();
    let first = journey::execute_steps(&spec("printf stable"), Some(tmp.path()), false).unwrap();
    journey::write_baseline(tmp.path(), "baseline-demo", &first).unwrap();
    let replay = journey::execute_steps(&spec("printf stable"), Some(tmp.path()), false).unwrap();
    let baseline = journey::read_baseline(tmp.path(), "baseline-demo")
        .unwrap()
        .unwrap();
    assert!(journey::deviations(&baseline, &replay).is_empty());
}

#[test]
fn output_drift_is_reported_even_when_exit_expectation_passes() {
    let tmp = Tmp::new();
    let first = journey::execute_steps(&spec("printf before"), Some(tmp.path()), false).unwrap();
    journey::write_baseline(tmp.path(), "baseline-demo", &first).unwrap();
    let replay = journey::execute_steps(&spec("printf after"), Some(tmp.path()), false).unwrap();
    assert!(replay[0].passed, "exit-code expectation still passes");
    let baseline = journey::read_baseline(tmp.path(), "baseline-demo")
        .unwrap()
        .unwrap();
    assert_eq!(
        journey::deviations(&baseline, &replay),
        vec!["print: verbatim output changed"]
    );
}

#[test]
fn latency_cliff_is_reported_without_changing_the_expectation_result() {
    let baseline = Baseline {
        journey: "baseline-demo".into(),
        outcomes: vec![StepOutcome {
            name: "print".into(),
            intent: "demo intent".into(),
            passed: true,
            detail: "ok".into(),
            transcript: "stable".into(),
            latency_ms: 50,
        }],
    };
    let replay = vec![StepOutcome {
        latency_ms: 200,
        ..baseline.outcomes[0].clone()
    }];
    assert_eq!(
        journey::deviations(&baseline, &replay),
        vec!["print: latency cliff (50ms → 200ms)"]
    );
}

#[test]
fn legacy_failed_baseline_is_not_read_as_proof() {
    let tmp = Tmp::new();
    let failed = Baseline {
        journey: "legacy-failure".into(),
        outcomes: vec![StepOutcome {
            name: "broken".into(),
            intent: "demo intent".into(),
            passed: false,
            detail: "exit 1 (want 0)".into(),
            transcript: String::new(),
            latency_ms: 1,
        }],
    };
    journey::write_baseline(tmp.path(), &failed.journey, &failed.outcomes).unwrap();
    assert!(journey::read_baseline(tmp.path(), &failed.journey)
        .unwrap()
        .is_none());
    assert!(journey::read_baselines(tmp.path()).unwrap().is_empty());
}

fn freeze(tmp: &Tmp, spec: &std::path::Path) -> loom::Result<()> {
    loom::commands::run(Cli {
        graph: Some(tmp.path().to_path_buf()),
        json: true,
        command: Some(Command::Journey {
            cmd: JourneyCmd::Freeze {
                spec: spec.to_path_buf(),
            },
        }),
    })
}

fn freeze_events(tmp: &Tmp) -> usize {
    loom::journal::read(tmp.path())
        .unwrap()
        .iter()
        .filter(|entry| entry.event == "journey_freeze")
        .count()
}

#[test]
fn successful_cli_freeze_writes_baseline_and_event() {
    let tmp = Tmp::new();
    Store::init(tmp.path(), Some("t"), false).unwrap();
    let path = tmp.path().join("success.json");
    std::fs::write(
        &path,
        r#"{"journey":"success","steps":[{"name":"one","intent":"demo","run":"printf ok","expect":{"exit_code":0}}]}"#,
    )
    .unwrap();

    freeze(&tmp, &path).unwrap();

    let baseline = journey::read_baseline(tmp.path(), "success")
        .unwrap()
        .unwrap();
    assert_eq!(baseline.outcomes.len(), 1);
    assert!(baseline.outcomes[0].passed);
    assert_eq!(freeze_events(&tmp), 1);
}

#[test]
fn mixed_failure_does_not_write_a_baseline_or_success_event() {
    let tmp = Tmp::new();
    Store::init(tmp.path(), Some("t"), false).unwrap();
    let path = tmp.path().join("mixed.json");
    std::fs::write(
        &path,
        r#"{"journey":"mixed","steps":[{"name":"pass","intent":"demo","run":"true"},{"name":"fail","intent":"demo","run":"false"}]}"#,
    )
    .unwrap();

    let err = freeze(&tmp, &path).unwrap_err().to_string();

    assert!(err.contains("completed 2/2") || err.contains("step 'fail' failed"));
    assert!(!journey::baseline_path(tmp.path(), "mixed").exists());
    assert_eq!(freeze_events(&tmp), 0);
}

#[test]
fn failed_refreeze_preserves_prior_baseline_bytes_and_adds_no_event() {
    let tmp = Tmp::new();
    Store::init(tmp.path(), Some("t"), false).unwrap();
    let path = tmp.path().join("refreeze.json");
    std::fs::write(
        &path,
        r#"{"journey":"refreeze","steps":[{"name":"one","intent":"demo","run":"printf stable"}]}"#,
    )
    .unwrap();
    freeze(&tmp, &path).unwrap();
    let baseline_path = journey::baseline_path(tmp.path(), "refreeze");
    let before = std::fs::read(&baseline_path).unwrap();
    let events_before = freeze_events(&tmp);
    std::fs::write(
        &path,
        r#"{"journey":"refreeze","steps":[{"name":"one","intent":"demo","run":"false"}]}"#,
    )
    .unwrap();

    freeze(&tmp, &path).unwrap_err();

    assert_eq!(std::fs::read(baseline_path).unwrap(), before);
    assert_eq!(freeze_events(&tmp), events_before);
}

#[test]
fn empty_spec_cannot_be_frozen() {
    let tmp = Tmp::new();
    Store::init(tmp.path(), Some("t"), false).unwrap();
    let path = tmp.path().join("empty.json");
    std::fs::write(&path, r#"{"journey":"empty","steps":[]}"#).unwrap();

    let err = freeze(&tmp, &path).unwrap_err().to_string();

    assert!(err.contains("no authored steps"));
    assert!(!journey::baseline_path(tmp.path(), "empty").exists());
    assert_eq!(freeze_events(&tmp), 0);
}
