//! Ring 62 — advisory role leases for multi-driver coordination.
//!
//! Real CLI processes, no mocks. Contracts defended: a lease demands the
//! matching lane authority and a worker profile; a fresh foreign lease
//! refuses with the contention exit code naming the holder; re-claim by the
//! same profile is an idempotent refresh; ordinary commands run under the
//! claimed identity heartbeat the lease; a stale lease is taken over only
//! with the deliberate `--take-stale`; release is holder-only; the announce
//! (`role list`, `status`, `session`) reports holders, freshness, and debt;
//! and claim/release/takeover land in the journal.

use std::path::Path;

mod common;
use common::*;

const TTL_MS: u64 = 900_000;

fn lease_path(root: &Path, role: &str) -> std::path::PathBuf {
    root.join(".loom")
        .join("leases")
        .join(format!("{role}.json"))
}

fn read_lease(root: &Path, role: &str) -> serde_json::Value {
    let raw = std::fs::read_to_string(lease_path(root, role)).expect("lease file exists");
    serde_json::from_str(&raw).expect("lease parses")
}

fn as_driver(root: &Path, lane: &str, profile: &str, args: &[&str]) -> std::process::Output {
    let mut cmd = loom_command();
    cmd.arg("--graph").arg(root);
    cmd.env("LOOM_AGENT", format!("llm:{lane}"));
    cmd.env("LOOM_AGENT_PROFILE", profile);
    cmd.args(args);
    cmd.output().expect("spawn loom")
}

#[test]
fn claim_without_a_graph_fails_closed_and_scaffolds_nothing() {
    let tmp = Tmp::new();
    // No `loom init`: a mistyped --graph path must not invent lease state.
    let out = as_driver(
        tmp.path(),
        "analyzer",
        "agent-a",
        &["role", "claim", "analyzer"],
    );
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("run `loom init` first"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !tmp.path().join(".loom").exists(),
        "a refused claim must leave no .loom residue"
    );
}

#[test]
fn claim_demands_matching_lane_authority_and_a_profile() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("leases"));

    // Solo claims nothing — it drives every lane already.
    let mut solo = loom_command();
    solo.arg("--graph").arg(tmp.path());
    solo.args(["role", "claim", "analyzer"]);
    let out = solo.output().unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("solo operator drives every lane"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // The wrong lane authority cannot claim on another role's behalf.
    let out = as_driver(
        tmp.path(),
        "builder",
        "agent-a",
        &["role", "claim", "analyzer"],
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("set LOOM_AGENT=llm:analyzer"));

    // The matching lane without a profile is an anonymous lease — refused.
    let mut anon = loom_command();
    anon.arg("--graph").arg(tmp.path());
    anon.env("LOOM_AGENT", "llm:analyzer");
    anon.args(["role", "claim", "analyzer"]);
    let out = anon.output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("LOOM_AGENT_PROFILE"));

    assert!(!lease_path(tmp.path(), "analyzer").exists());
}

#[test]
fn fresh_foreign_lease_refuses_with_contention_exit_naming_the_holder() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("leases"));

    let out = as_driver(
        tmp.path(),
        "analyzer",
        "agent-a",
        &["role", "claim", "analyzer"],
    );
    assert!(
        out.status.success(),
        "claim failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Second driver, same role: bounded refusal on the reserved exit code,
    // naming the recorded holder — the same convention as the graph lock.
    let out = as_driver(
        tmp.path(),
        "analyzer",
        "agent-b",
        &["role", "claim", "analyzer"],
    );
    assert_eq!(out.status.code(), Some(75), "contention is exit 75");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("loom-role-contention"), "stderr: {stderr}");
    assert!(stderr.contains("agent-a"), "names the holder: {stderr}");

    // Re-claim by the holder is an idempotent refresh, not an error.
    let out = as_driver(
        tmp.path(),
        "analyzer",
        "agent-a",
        &["role", "claim", "analyzer"],
    );
    assert!(out.status.success());

    // Release is holder-only.
    let out = as_driver(
        tmp.path(),
        "analyzer",
        "agent-b",
        &["role", "release", "analyzer"],
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("only the holder releases"));
    let out = as_driver(
        tmp.path(),
        "analyzer",
        "agent-a",
        &["role", "release", "analyzer"],
    );
    assert!(out.status.success());
    assert!(!lease_path(tmp.path(), "analyzer").exists());
}

#[test]
fn ordinary_commands_heartbeat_the_lease_and_stale_takeover_is_deliberate() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("leases"));

    let out = as_driver(
        tmp.path(),
        "quality",
        "agent-a",
        &["role", "claim", "quality"],
    );
    assert!(out.status.success());

    // Age the lease far past the TTL by editing operational state directly.
    let mut lease = read_lease(tmp.path(), "quality");
    let aged = lease["last_seen_ms"].as_u64().unwrap() - (TTL_MS * 2);
    lease["last_seen_ms"] = aged.into();
    std::fs::write(
        lease_path(tmp.path(), "quality"),
        serde_json::to_vec(&lease).unwrap(),
    )
    .unwrap();

    // Any command run under the claimed identity refreshes the heartbeat.
    let out = as_driver(tmp.path(), "quality", "agent-a", &["status"]);
    assert!(out.status.success());
    let refreshed = read_lease(tmp.path(), "quality");
    assert!(
        refreshed["last_seen_ms"].as_u64().unwrap() > aged + TTL_MS,
        "store open under the holder identity must stamp last_seen_ms"
    );

    // A foreign command does NOT refresh someone else's lease.
    lease["last_seen_ms"] = aged.into();
    std::fs::write(
        lease_path(tmp.path(), "quality"),
        serde_json::to_vec(&lease).unwrap(),
    )
    .unwrap();
    let out = as_driver(tmp.path(), "quality", "agent-b", &["status"]);
    assert!(out.status.success());
    assert_eq!(
        read_lease(tmp.path(), "quality")["last_seen_ms"]
            .as_u64()
            .unwrap(),
        aged,
        "a non-holder must not heartbeat a foreign lease"
    );

    // Stale takeover requires the explicit acknowledgement…
    let out = as_driver(
        tmp.path(),
        "quality",
        "agent-b",
        &["role", "claim", "quality"],
    );
    assert!(!out.status.success());
    assert_ne!(out.status.code(), Some(75), "stale is not contention");
    assert!(String::from_utf8_lossy(&out.stderr).contains("--take-stale"));

    // …and succeeds with it, recording the displaced holder in the journal.
    let out = as_driver(
        tmp.path(),
        "quality",
        "agent-b",
        &["role", "claim", "quality", "--take-stale"],
    );
    assert!(out.status.success());
    assert_eq!(read_lease(tmp.path(), "quality")["profile"], "agent-b");

    let journal = std::fs::read_to_string(
        tmp.path()
            .join(".loom")
            .join("journal")
            .join("events.jsonl"),
    )
    .unwrap();
    assert!(journal.contains("\"role_claimed\""));
    assert!(journal.contains("\"took_over_stale\":\"agent-a\""));
}

#[test]
fn next_warns_when_the_packets_role_is_leased_to_someone_else() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("leases"));
    {
        // Seed one planned intent so the build lane serves a packet, then
        // drop the store so the CLI spawns below can take the lock.
        let store = loom::store::Store::open(tmp.path()).unwrap();
        store
            .add_node(
                loom::model::NodeType::Intent,
                "users can export reports",
                "a report export exists",
                "planned",
                serde_json::json!({}),
            )
            .unwrap();
    }
    let out = as_driver(
        tmp.path(),
        "builder",
        "agent-a",
        &["role", "claim", "builder"],
    );
    assert!(out.status.success());

    // The holder drains its own lane: no warning.
    let out = as_driver(
        tmp.path(),
        "builder",
        "agent-a",
        &["--json", "next", "--mode", "build"],
    );
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["work_item"].is_object(), "build packet is served");
    assert!(v.get("lease_conflict").is_none(), "holder sees no warning");

    // A different profile draining the same lane is warned — and still served:
    // the lease is advisory, so the collision is chosen, never accidental.
    let out = as_driver(
        tmp.path(),
        "builder",
        "agent-b",
        &["--json", "next", "--mode", "build"],
    );
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["work_item"].is_object(), "the packet is still served");
    let warning = v["lease_conflict"].as_str().expect("conflict is named");
    assert!(
        warning.contains("agent-a"),
        "warning names the holder: {warning}"
    );
}

#[test]
fn announce_reports_holders_freshness_and_debt() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("leases"));

    // Before any claim: every role free, no roles line in status text.
    let mut status = loom_command();
    status.arg("--graph").arg(tmp.path()).arg("status");
    let out = status.output().unwrap();
    assert!(out.status.success());
    assert!(!String::from_utf8_lossy(&out.stdout).contains("roles:"));

    let out = as_driver(
        tmp.path(),
        "builder",
        "agent-a",
        &["role", "claim", "builder"],
    );
    assert!(out.status.success());

    // role list --json: six claimable roles, holder + freshness + lanes + debt.
    let mut list = loom_command();
    list.arg("--graph").arg(tmp.path()).arg("--json");
    list.args(["role", "list"]);
    let out = list.output().unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let roles = v["roles"].as_object().unwrap();
    assert_eq!(roles.len(), 6);
    assert_eq!(roles["builder"]["claimed_by"], "agent-a");
    assert_eq!(roles["builder"]["fresh"], true);
    assert!(roles["builder"]["lanes"].get("build").is_some());
    assert!(roles["builder"]["debt"].is_u64());
    assert!(roles["analyzer"]["claimed_by"].is_null());
    // The shared review lane shows under all three verdict-writing roles.
    for shared in ["analyzer", "validator", "quality"] {
        assert!(
            roles[shared]["lanes"].get("review").is_some(),
            "{shared} lists the shared review lane"
        );
    }

    // status --json carries the same block; status text names the holder.
    let mut status = loom_command();
    status
        .arg("--graph")
        .arg(tmp.path())
        .arg("--json")
        .arg("status");
    let out = status.output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["roles"]["builder"]["claimed_by"], "agent-a");

    let mut status = loom_command();
    status.arg("--graph").arg(tmp.path()).arg("status");
    let out = status.output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("roles: builder=agent-a(fresh)"),
        "status text: {text}"
    );

    // session --json also announces, so a joining driver sees it turn-zero.
    let mut session = loom_command();
    session
        .arg("--graph")
        .arg(tmp.path())
        .arg("--json")
        .arg("session");
    let out = session.output().unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["roles"]["builder"]["claimed_by"], "agent-a");
}
