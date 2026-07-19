//! Ring 18 — local verbatim journey baselines and replay deviations.

use loom::journey::{self, Baseline, Expect, JourneySpec, Step, StepOutcome};
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
