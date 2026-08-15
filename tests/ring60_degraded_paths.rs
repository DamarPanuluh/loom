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

fn json_cli(root: &std::path::Path, args: &[&str]) -> (bool, serde_json::Value, String) {
    let output = loom_command()
        .arg("--graph")
        .arg(root)
        .args(args)
        .output()
        .expect("spawn loom");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let value = serde_json::from_str(&stdout).unwrap_or(serde_json::Value::Null);
    (output.status.success(), value, format!("{stdout}{stderr}"))
}

/// Seed a graph and release the write lock, so the spawned CLI can take it.
fn intent_fixture(tmp: &Tmp, names: &[&str]) {
    let store = loom::store::Store::init(tmp.path(), Some("t"), false).unwrap();
    for name in names {
        store
            .add_node(
                loom::model::NodeType::Intent,
                name,
                "",
                "",
                serde_json::json!({ "level": "feature" }),
            )
            .unwrap();
    }
    drop(store);
}

#[test]
fn an_offset_past_the_end_returns_an_empty_page_with_no_continuation() {
    let tmp = Tmp::new();
    intent_fixture(&tmp, &["alpha", "beta", "gamma"]);

    let (ok, page, raw) = json_cli(tmp.path(), &["intent", "list", "--offset", "0", "--json"]);
    assert!(ok, "{raw}");
    let total = page["pagination"]["total"].as_u64().expect("total");
    assert_eq!(total, 3);

    let past = (total + 50).to_string();
    let (ok, page, raw) = json_cli(tmp.path(), &["intent", "list", "--offset", &past, "--json"]);
    assert!(ok, "an offset past the end is not an error: {raw}");
    assert_eq!(page["items"].as_array().unwrap().len(), 0);
    assert_eq!(page["pagination"]["returned"], 0);
    assert_eq!(page["pagination"]["has_more"], false);
    assert!(
        page["pagination"]["next_offset"].is_null(),
        "a page past the end must not offer a continuation: {page}"
    );
    assert_eq!(page["pagination"]["total"], total);
}

#[test]
fn an_ambiguous_behavior_name_is_refused_rather_than_resolved_to_a_guess() {
    let tmp = Tmp::new();
    // Two active nodes carrying the exact same name.
    intent_fixture(&tmp, &["twin", "twin", "unique"]);

    let (ok, _v, raw) = json_cli(tmp.path(), &["intent", "show", "twin", "--json"]);
    assert!(!ok, "an ambiguous exact name must not resolve: {raw}");
    assert!(
        raw.contains("ambiguous"),
        "the refusal must say the name is ambiguous: {raw}"
    );
    assert!(
        raw.contains('2'),
        "the refusal must name how many nodes collide: {raw}"
    );

    // The unambiguous sibling still resolves, so the refusal is about the
    // collision and not about lookup being broken.
    let (ok, value, raw) = json_cli(tmp.path(), &["intent", "show", "unique", "--json"]);
    assert!(ok, "{raw}");
    assert_eq!(value["name"], "unique");
}

#[test]
fn attesting_a_burst_that_does_not_exist_is_refused_before_any_human_seal() {
    let tmp = Tmp::new();
    intent_fixture(&tmp, &["alpha"]);

    // Every other argument is supplied and well formed, so the only thing that
    // can refuse this is the burst check itself.
    let (ok, _v, raw) = json_cli(
        tmp.path(),
        &[
            "audit",
            "attest-burst",
            "llm:analyzer@2026-08-15T00:00",
            "--claim",
            "adjudication",
            "--criterion",
            "host-mediated",
            "--evidence",
            "a table of judgments reviewed in the host conversation",
            "--authority",
            "human",
            "--executor",
            "llm:analyzer",
            "--json",
        ],
    );
    assert!(!ok, "attesting a nonexistent burst must fail: {raw}");
    assert!(
        raw.contains("no live judgment burst"),
        "the refusal must name the missing burst: {raw}"
    );
    assert!(
        raw.contains("found 0"),
        "the refusal must report the count it actually found: {raw}"
    );
    // The human sealing challenge is never reached, so no authorization
    // envelope may exist afterwards.
    assert!(
        !raw.contains("sealed"),
        "no seal may be recorded for a burst that does not exist: {raw}"
    );
}
