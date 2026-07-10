//! Bootstrap suggest — cold-start Proposal of planned pillars.

use loom::model::NodeType;
use loom::store::Store;
use std::path::Path;
use std::process::Command;

mod common;
use common::*;

fn loom_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_loom"))
}

fn loom_init(tmp: &Path, name: &str) {
    let out = Command::new(loom_bin())
        .arg("init")
        .arg(tmp)
        .arg("--name")
        .arg(name)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn loom_ok(tmp: &Path, args: &[&str]) -> String {
    let mut cmd = Command::new(loom_bin());
    cmd.arg("--graph").arg(tmp).args(args);
    let out = cmd.output().expect("spawn loom");
    assert!(
        out.status.success(),
        "loom {:?} failed: {}\n{}",
        args,
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn loom_err(tmp: &Path, args: &[&str]) -> String {
    let mut cmd = Command::new(loom_bin());
    cmd.arg("--graph").arg(tmp).args(args);
    let out = cmd.output().expect("spawn loom");
    assert!(!out.status.success(), "loom {:?} should have failed", args);
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn bootstrap_suggest_creates_proposal_adopt_yields_planned_intent() {
    let tmp = Tmp::new();
    tmp.write("src/auth.rs", "pub fn login() {}\n");
    tmp.write("tests/auth_flow.rs", "#[test] fn login_works() {}\n");
    tmp.write(
        "README.md",
        "# Demo\n\n## Sign in\n\nUsers authenticate.\n\n## License\n\nMIT\n",
    );

    loom_init(tmp.path(), "boot");
    loom_ok(tmp.path(), &["codefile", "add", "src/auth.rs"]);

    let out = loom_ok(tmp.path(), &["bootstrap", "suggest", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let proposal_id = v["proposal"]["id"].as_str().unwrap().to_string();
    let n = v["candidates"].as_u64().unwrap();
    assert!(n >= 1, "expected at least one candidate: {out}");

    let items = v["proposal"]["body"]["items"].as_array().unwrap();
    assert!(!items.is_empty());
    assert_eq!(items[0]["status"], "open");
    assert_eq!(items[0]["kind"], "intent");

    loom_ok(
        tmp.path(),
        &[
            "proposal",
            "item",
            "adopt",
            &proposal_id,
            "1",
            "--as",
            "intent",
            "--name",
            "users can sign in",
            "--description",
            "authentication succeeds before protected actions",
        ],
    );

    let store = Store::open(tmp.path()).unwrap();
    let intents = store
        .list_nodes(Some(NodeType::Intent), usize::MAX)
        .unwrap();
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0].status, "planned");
    assert_eq!(intents[0].name, "users can sign in");
    drop(store);

    let err = loom_err(tmp.path(), &["bootstrap", "suggest"]);
    assert!(
        err.contains("non-empty intent graph") || err.contains("already exist"),
        "expected refuse on non-empty intents: {err}"
    );
}

#[test]
fn bootstrap_suggest_requires_codefiles() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), "empty");
    let err = loom_err(tmp.path(), &["bootstrap", "suggest"]);
    assert!(
        err.contains("codefile") || err.contains("registered"),
        "expected codefile prerequisite: {err}"
    );
}
