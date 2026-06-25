use serde_json::Value;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn loom_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_loom"))
}

fn run_json_as(cwd: &Path, args: &[&str], agent: &str) -> Value {
    let output = Command::new(loom_bin())
        .args(args)
        .current_dir(cwd)
        .env("LOOM_AGENT", agent)
        .env_remove("LOOM_GRAPH")
        .env_remove("LOOM_DIAGNOSE_MISSING_BASE")
        .output()
        .unwrap_or_else(|err| panic!("failed to run loom {args:?}: {err}"));

    if !output.status.success() {
        panic!(
            "loom {:?} failed with {}\nstdout:\n{}\nstderr:\n{}",
            args,
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "loom {:?} emitted invalid JSON: {err}\nstdout:\n{}",
            args,
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn run_json(cwd: &Path, args: &[&str]) -> Value {
    run_json_as(cwd, args, "llm:validator")
}

fn write_file(cwd: &Path, path: &str, content: &str) {
    let abs = cwd.join(path);
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent).expect("create parent dir");
    }
    fs::write(&abs, content).expect("write scratch file");
}

fn scratch_root(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_nanos();
    env::temp_dir().join(format!("loom-cold-{prefix}-{}-{nanos}", std::process::id()))
}

/// The cold-verifier boundary for saga trivial-endpoint detection:
///  - default segment list flags final-segment trivial URLs (/ping, /health)
///  - compound paths (/status/orders) are NOT flagged by default
///  - project-specific paths (/_/health, /service/ready) are flagged only when
///    declared in .loom/trivial-endpoints.yml
#[test]
fn cold_saga_endpoint_warning_boundary() {
    let root = scratch_root("endpoint-warning");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create scratch root");

    run_json(&root, &["init", ".", "--json"]);
    write_file(&root, "app.py", "def do_GET(self):\n    pass\n");
    run_json_as(
        &root,
        &["codefile", "add", "app.py", "--json"],
        "llm:builder",
    );

    let mk_intent = |name: &str, desc: &str| -> String {
        run_json_as(
            &root,
            &[
                "intent",
                "add",
                "--name",
                name,
                "--description",
                desc,
                "--level",
                "feature",
                "--lifecycle",
                "implemented",
                "--boundary",
                "inbound",
                "--json",
            ],
            "llm:builder",
        )["id"]
            .as_str()
            .unwrap()
            .to_string()
    };

    let orders = mk_intent(
        "list orders",
        "An authenticated user GETs their order history",
    );
    let profile = mk_intent("view profile", "An authenticated user GETs their profile");

    run_json_as(
        &root,
        &[
            "edge",
            "implement",
            &orders,
            "app.py",
            "--locator",
            "def do_GET",
            "--json",
        ],
        "llm:builder",
    );
    run_json(&root, &["sync", "--json"]);
    let add_step = |saga: &str, intent: &str, url: &str| -> Value {
        let f = format!("{saga}.yaml");
        write_file(
            &root,
            &f,
            &format!(
                "saga: {saga}\nsteps:\n  - name: step\n    intent: {intent}\n    request: {{ method: GET, url: {url} }}\n    expect: {{ status: 200 }}\n"
            ),
        );
        run_json_as(&root, &["saga", "add", &f, "--json"], "llm:validator")
    };
    let flagged = |v: &Value| -> usize {
        v["unmatched_steps"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0)
    };

    // DEFAULT exact-segment matches: final segment is trivial.
    assert_eq!(
        flagged(&add_step("ping", &profile, "/ping")),
        1,
        "/ping final segment is trivial by default"
    );
    assert_eq!(
        flagged(&add_step("health", &profile, "/api/v1/health")),
        1,
        "/api/v1/health final segment is trivial by default"
    );

    // DEFAULT segment discipline: compound path is treated as a real journey.
    assert_eq!(
        flagged(&add_step("status-orders", &orders, "/status/orders")),
        0,
        "/status/orders final segment is 'orders', not trivial"
    );

    // Without config, compound path /health/aggregate is NOT flagged because the
    // final segment ('aggregate') is not a default trivial segment.
    assert_eq!(
        flagged(&add_step("health-aggregate", &profile, "/health/aggregate")),
        0,
        "/health/aggregate final segment is 'aggregate', not trivial"
    );

    // Add project config that marks /health/aggregate and /service/ready-check as
    // trivial full paths, plus a custom substring token for legacy probes.
    write_file(
        &root,
        ".loom/trivial-endpoints.yml",
        "paths:\n  - /health/aggregate\n  - /service/ready-check\nsubstring_tokens:\n  - legacy-probe\n",
    );

    assert_eq!(
        flagged(&add_step(
            "health-aggregate-cfg",
            &profile,
            "/health/aggregate"
        )),
        1,
        "/health/aggregate must be flagged after adding to trivial-endpoints.yml"
    );
    assert_eq!(
        flagged(&add_step(
            "service-ready-check",
            &profile,
            "/service/ready-check"
        )),
        1,
        "/service/ready-check must be flagged after adding to trivial-endpoints.yml"
    );
    assert_eq!(
        flagged(&add_step(
            "legacy-probe",
            &profile,
            "/internal/legacy-probe"
        )),
        1,
        "substring token 'legacy-probe' must flag matching URLs"
    );

    // Config should still respect intent self-description: an aggregate-health
    // intent hitting /health/aggregate is NOT a forge.
    let aggregate_health = mk_intent(
        "aggregate health",
        "Fetch the aggregate health view at /health/aggregate",
    );
    assert_eq!(
        flagged(&add_step(
            "aggregate-health-ok",
            &aggregate_health,
            "/health/aggregate"
        )),
        0,
        "/health/aggregate bound to an aggregate-health intent must not false-warn"
    );

    let _ = fs::remove_dir_all(&root);
}
