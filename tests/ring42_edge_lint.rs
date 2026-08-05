//! Ring 42 — proof-strength dead ends are named at edge-write time.
//!
//! Certain grounding choices make the top strength grade permanently
//! unreachable: a locator naming a symbol the call graph cannot treat as
//! callable (a struct, a type, a binding), or a witness file exposing zero
//! indexable symbols. The tool used to report only the symptom later
//! ("nothing reaches the grounded symbol"); now the write itself warns, and
//! says what WOULD be indexable.

use loom::model::NodeType;
use loom::store::Store;
mod common;
use common::*;

fn loom_json(tmp: &std::path::Path, args: &[&str]) -> serde_json::Value {
    let out = std::process::Command::new(std::path::PathBuf::from(env!("CARGO_BIN_EXE_loom")))
        .arg("--graph")
        .arg(tmp)
        .args(args)
        .arg("--json")
        .output()
        .unwrap_or_else(|e| panic!("spawn loom {args:?}: {e}"));
    assert!(
        out.status.success(),
        "loom {args:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .unwrap_or_else(|e| panic!("no json: {e}"))
}

fn intent_and_file(tmp: &Tmp, file_content: &str) -> (Store, String) {
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(tmp.path().join("src/thing.rs"), file_content).unwrap();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "a behavior",
            "d",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    codefile(&store, "src/thing.rs");
    (store, intent.id)
}

/// A locator naming a struct: resolvable, honest — and permanently below S3.
/// The write succeeds but must say so, naming what WOULD be callable.
#[test]
fn a_non_callable_locator_lints_at_write_time_and_names_alternatives() {
    let tmp = Tmp::new();
    let (store, intent_id) = intent_and_file(
        &tmp,
        "pub struct Config {\n    pub retries: u8,\n}\n\npub fn load_config() -> u8 {\n    3\n}\n",
    );
    drop(store);

    let out = loom_json(
        tmp.path(),
        &[
            "edge",
            "implement",
            &intent_id,
            "src/thing.rs",
            "--locator",
            "struct Config",
        ],
    );
    let lints = out["lints"].as_array().expect("lints array");
    assert_eq!(lints.len(), 1, "one lint: {out}");
    let lint = lints[0].as_str().unwrap();
    assert!(lint.contains("Config"), "names the symbol: {lint}");
    assert!(lint.contains("struct"), "names the kind: {lint}");
    assert!(lint.contains("S3"), "names the unreachable grade: {lint}");
    assert!(
        lint.contains("load_config"),
        "says what WOULD be indexable: {lint}"
    );
}

/// A callable locator earns no lint.
#[test]
fn a_callable_locator_lints_nothing() {
    let tmp = Tmp::new();
    let (store, intent_id) = intent_and_file(&tmp, "pub fn load_config() -> u8 {\n    3\n}\n");
    drop(store);

    let out = loom_json(
        tmp.path(),
        &[
            "edge",
            "implement",
            &intent_id,
            "src/thing.rs",
            "--locator",
            "fn load_config",
        ],
    );
    assert_eq!(
        out["lints"].as_array().unwrap().len(),
        0,
        "no lint for a callable locator: {out}"
    );
}

/// A witness file whose language has no extractor: zero indexable symbols,
/// named at write time with what would be indexable.
#[test]
fn a_witness_file_with_no_indexable_symbols_lints_at_write_time() {
    let tmp = Tmp::new();
    std::fs::create_dir_all(tmp.path().join("checks")).unwrap();
    std::fs::write(
        tmp.path().join("checks/manual.md"),
        "# manual check\n\nlook at the output\n",
    )
    .unwrap();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "a behavior",
            "d",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    codefile(&store, "checks/manual.md");
    drop(store);

    let out = loom_json(
        tmp.path(),
        &[
            "edge",
            "implement",
            &intent.id,
            "checks/manual.md",
            "--role",
            "verifies",
        ],
    );
    let lints = out["lints"].as_array().expect("lints array");
    assert_eq!(lints.len(), 1, "one lint: {out}");
    let lint = lints[0].as_str().unwrap();
    assert!(
        lint.contains("no indexable symbols"),
        "names the dead end: {lint}"
    );
    assert!(
        lint.contains("rust"),
        "says what WOULD be indexable: {lint}"
    );
}

/// The same lint fires when the locator is re-pointed with set-locator.
#[test]
fn set_locator_lints_a_non_callable_target() {
    let tmp = Tmp::new();
    let (store, intent_id) = intent_and_file(
        &tmp,
        "pub struct Config {\n    pub retries: u8,\n}\n\npub fn load_config() -> u8 {\n    3\n}\n",
    );
    drop(store);

    let out = loom_json(
        tmp.path(),
        &[
            "edge",
            "implement",
            &intent_id,
            "src/thing.rs",
            "--locator",
            "fn load_config",
        ],
    );
    let edge_id = out["edge"]["id"].as_str().unwrap().to_string();

    let out = loom_json(
        tmp.path(),
        &["edge", "set-locator", &edge_id, "struct Config"],
    );
    let lints = out["lints"].as_array().expect("lints array");
    assert_eq!(lints.len(), 1, "one lint on re-point: {out}");
    assert!(
        lints[0].as_str().unwrap().contains("S3"),
        "the re-point names the unreachable grade: {out}"
    );
}
