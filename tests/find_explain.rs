//! Find --tag/--where and explain neighborhood brief.

use std::path::Path;
use std::process::Command;

mod common;
use common::*;

fn loom_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_loom"))
}

fn loom_init(tmp: &Path) {
    let out = Command::new(loom_bin())
        .arg("init")
        .arg(tmp)
        .arg("--name")
        .arg("t")
        .output()
        .unwrap();
    assert!(out.status.success());
}

fn loom_ok(tmp: &Path, args: &[&str]) -> String {
    let mut cmd = Command::new(loom_bin());
    cmd.arg("--graph").arg(tmp).args(args);
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "loom {:?} failed: {}\n{}",
        args,
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn find_tag_and_where_and_explain() {
    let tmp = Tmp::new();
    loom_init(tmp.path());

    loom_ok(tmp.path(), &["vocab", "add", "auth", "--why", "authentication concern"]);
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "users can sign in",
            "--description",
            "auth succeeds",
            "--lifecycle",
            "implemented",
            "--visibility",
            "user_visible",
            "--level",
            "feature",
        ],
    );
    loom_ok(
        tmp.path(),
        &["intent", "tag", "add", "users can sign in", "auth"],
    );
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "internal hash ripple",
            "--description",
            "sync stales",
            "--lifecycle",
            "implemented",
            "--visibility",
            "internal",
        ],
    );

    let out = loom_ok(
        tmp.path(),
        &[
            "find",
            "--tag",
            "auth",
            "--where",
            "visibility=user_visible",
            "--json",
        ],
    );
    let rows: serde_json::Value = serde_json::from_str(&out).unwrap();
    let arr = rows.as_array().unwrap();
    assert_eq!(
        arr.len(),
        1,
        "expected only the tagged user_visible intent: {out}"
    );
    assert_eq!(arr[0]["name"], "users can sign in");

    let brief = loom_ok(tmp.path(), &["explain", "users can sign in", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&brief).unwrap();
    assert_eq!(v["intent"]["name"], "users can sign in");
    assert_eq!(v["intent"]["visibility"], "user_visible");
    assert!(v["completeness"]["axes"].is_array());
}
