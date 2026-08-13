//! Compile-fail proof that external crates cannot mint trusted Journey
//! assertion provenance through the public API.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn consumer_dir() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "loom-api-boundary-{}-{}",
        std::process::id(),
        nanos
    ))
}

fn compile_consumer(source: &str) -> (bool, String) {
    let root = consumer_dir();
    fs::create_dir_all(root.join("src")).unwrap();
    let loom = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    fs::write(
        root.join("Cargo.toml"),
        format!(
            r#"[package]
name = "loom-api-boundary-consumer"
version = "0.0.0"
edition = "2021"

[dependencies]
loom = {{ path = "{}" }}
"#,
            loom.display()
        ),
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), source).unwrap();
    let mut check = Command::new("cargo");
    check.args(["check", "--offline", "--quiet"]);
    if let Ok(dir) = std::env::var("CARGO_TARGET_DIR") {
        check.env("CARGO_TARGET_DIR", dir);
    } else {
        check.env(
            "CARGO_TARGET_DIR",
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target"),
        );
    }
    let output = check
        .current_dir(&root)
        .output()
        .expect("cargo check consumer");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let _ = fs::remove_dir_all(&root);
    (output.status.success(), stderr)
}

#[test]
fn external_crate_cannot_call_settle_with_caller_report() {
    let (ok, stderr) = compile_consumer(
        r#"
        pub fn attack(store: &loom::store::Store, report: &loom::journey_runtime::RuntimeReport) {
            let _ = loom::journey::settle_compiled_validation(
                store,
                "validation-id",
                report,
                &["src/cli.rs".into()],
            );
        }
        "#,
    );
    assert!(
        !ok,
        "forged RuntimeReport settlement must not compile:\n{stderr}"
    );
    assert!(
        stderr.contains("this function takes")
            || stderr.contains("arguments")
            || stderr.contains("E0061")
            || stderr.contains("E0308"),
        "expected a type/arity error, got:\n{stderr}"
    );
}

#[test]
fn external_crate_cannot_construct_journey_observation() {
    let (ok, stderr) = compile_consumer(
        r#"
        pub fn attack() -> loom::journey_runtime::JourneyObservation {
            loom::journey_runtime::JourneyObservation {
                report: unimplemented!(),
                proof: unimplemented!(),
            }
        }
        "#,
    );
    assert!(
        !ok,
        "JourneyObservation must not be constructible:\n{stderr}"
    );
    assert!(
        stderr.contains("private") || stderr.contains("E0451") || stderr.contains("E0423"),
        "expected a privacy error, got:\n{stderr}"
    );
}

#[test]
fn external_crate_cannot_attach_forged_run_through_assertion_observed() {
    let (ok, stderr) = compile_consumer(
        r#"
        pub fn attack(run: loom::evidence::RunRecord) -> loom::store::Assertion<'static> {
            loom::store::Assertion::new(
                loom::store::Subject::Node("n".into()),
                loom::model::Claim::Verdict,
                "passing",
                "attacker",
            )
            .observed(run)
        }
        "#,
    );
    assert!(
        !ok,
        "Assertion::observed(forged_run) must not compile:\n{stderr}"
    );
    assert!(
        stderr.contains("private")
            || stderr.contains("E0624")
            || stderr.contains("no method named `observed`"),
        "expected observed to be unusable, got:\n{stderr}"
    );
}
