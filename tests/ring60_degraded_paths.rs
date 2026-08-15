//! Ring 60 — the degraded paths named as scenarios in the 2026-08-15 drain.
//!
//! Each test proves exactly one scenario behavior, so no proof command is
//! shared between two behaviors.

mod common;
use common::*;

#[test]
fn an_invalid_quality_pattern_fails_the_scan_with_the_pattern_named() {
    let tmp = Tmp::new();
    tmp.write("src/a.rs", "fn a() {}\n");
    let err = loom::prescan::prescreen(
        tmp.path(),
        &["src/a.rs".to_string()],
        &["fn a(".to_string()], // unbalanced parenthesis: not a valid regex
        10,
    )
    .expect_err("an uncompilable pattern must fail the scan");
    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("fn a("),
        "the error must name the offending pattern: {rendered}"
    );

    // A valid pattern over the same inputs still returns hits, so the failure
    // above is the pattern's, not the scan's.
    let hits = loom::prescan::prescreen(
        tmp.path(),
        &["src/a.rs".to_string()],
        &["fn a".to_string()],
        10,
    )
    .unwrap();
    assert_eq!(hits.len(), 1);
}

#[test]
fn a_zero_cap_or_empty_input_extracts_nothing_without_reading_files() {
    let tmp = Tmp::new();
    // No file is written: reading one would fail, so an empty short-circuit is
    // the only way these can succeed.
    for (files, patterns, cap) in [
        (vec!["missing.rs".to_string()], vec!["fn".to_string()], 0),
        (vec![], vec!["fn".to_string()], 10),
        (vec!["missing.rs".to_string()], vec![], 10),
    ] {
        let hits = loom::prescan::prescreen(tmp.path(), &files, &patterns, cap).unwrap();
        assert!(hits.is_empty());
    }
}

#[test]
fn on_ambiguity_the_driver_policy_defers_instead_of_widening_the_stage() {
    let policy = loom::checkpoint::driver_policy();
    let rendered = serde_json::to_value(&policy).unwrap();
    let local = &rendered["local_commit"];
    assert_eq!(local["stage_only_included_paths"], true);
    assert_eq!(local["forbidden_command"], "git add -A");
    assert_eq!(local["defer_on_ambiguity"], true);
    assert_eq!(local["publication"], "local_only");
}

#[test]
fn a_push_needs_a_current_decision_bound_to_the_exact_commit() {
    let policy = loom::checkpoint::push_policy();
    let rendered = serde_json::to_value(&policy).unwrap();
    assert_eq!(rendered["allowed_without_human_decision"], false);
    assert_eq!(rendered["drift_requires_new_decision"], true);
    assert_eq!(rendered["silence_or_refusal"], "keep_local");
    let binding = rendered["required_binding"].as_array().unwrap();
    for part in ["repository", "remote", "branch", "commit"] {
        assert!(
            binding.iter().any(|b| b == part),
            "the decision must bind {part}"
        );
    }
}
