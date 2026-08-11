//! Bootstrap suggest — cold-start Proposal of semantic Journey clues.

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
fn bootstrap_suggest_creates_clues_then_an_authored_journey_becomes_the_root() {
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
    let n = v["candidates"].as_u64().unwrap();
    assert!(n >= 1, "expected at least one candidate: {out}");

    let items = v["proposal"]["body"]["items"].as_array().unwrap();
    assert!(!items.is_empty());
    assert_eq!(items[0]["status"], "open");
    assert_eq!(items[0]["kind"], "journey_clue");

    tmp.write(
        "journeys/sign-in.yaml",
        concat!(
            "schema: loom.journey/v1\n",
            "id: sign-in\n",
            "name: Users sign in\n",
            "actor: user\n",
            "goal: A user authenticates before protected actions.\n",
            "inputs: {}\n",
            "preconditions: []\n",
            "steps:\n",
            "  - id: authenticate\n",
            "    name: Authenticate\n",
            "    action: Authenticate with valid credentials\n",
            "    expects:\n",
            "      - Protected actions are available\n",
            "    produces: {}\n",
            "profiles:\n",
            "  proof:\n",
            "    inputs: {}\n",
            "    workspace: {}\n",
        ),
    );
    let spec_path = tmp.path().join("journeys/sign-in.yaml");
    let spec_arg = spec_path.to_string_lossy().into_owned();
    loom_ok(tmp.path(), &["journey", "add", &spec_arg]);

    let store = Store::open(tmp.path()).unwrap();
    let journeys = store
        .list_nodes(Some(NodeType::Journey), usize::MAX)
        .unwrap();
    assert_eq!(journeys.len(), 1);
    assert_eq!(journeys[0].status, "authored");
    assert_eq!(journeys[0].name, "sign-in");
    assert!(store
        .list_nodes(Some(NodeType::Intent), usize::MAX)
        .unwrap()
        .is_empty());
    drop(store);

    let err = loom_err(tmp.path(), &["bootstrap", "suggest"]);
    assert!(
        err.contains("authored Journeys") || err.contains("Journey root"),
        "expected refuse once a Journey root exists: {err}"
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
