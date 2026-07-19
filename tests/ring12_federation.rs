//! Ring 12 — cross-graph federation contracts.
//!
//! Real SQLite, two separate Tmp graphs, no mocks. Each test defends one
//! observable contract of the federation system: linking, sync-time
//! reconciliation, staleness propagation, upstream invisibility to local
//! queues, unlink orphaning, and wipe_derived convergence.

use loom::model::{EdgeKind, InspectionStatus, NodeType, TargetKind};
use loom::store::Store;

mod common;
use common::*;

/// Path to the compiled `loom` binary, provided by Cargo at build time.
fn loom_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_loom"))
}

fn loom_init(tmp: &std::path::Path, name: Option<&str>) {
    let mut cmd = std::process::Command::new(loom_bin());
    cmd.arg("init").arg(tmp);
    if let Some(n) = name {
        cmd.arg("--name").arg(n);
    }
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn loom_ok(tmp: &std::path::Path, args: &[&str]) {
    let mut cmd = std::process::Command::new(loom_bin());
    cmd.arg("--graph").arg(tmp).args(args);
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "loom {:?} failed: {}\n{}",
        args,
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout),
    );
}

fn loom_json(tmp: &std::path::Path, args: &[&str]) -> serde_json::Value {
    let mut cmd = std::process::Command::new(loom_bin());
    cmd.arg("--graph").arg(tmp).args(args).arg("--json");
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "loom {:?} failed: {}\n{}",
        args,
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout),
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "loom {:?} stdout not JSON: {e}\n{}",
            args,
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn write_file(root: &std::path::Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

#[test]
fn edge_implement_is_idempotent_and_updates_the_locator() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("grounding"));
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "federated behavior",
            "--description",
            "one behavior",
        ],
    );
    write_file(tmp.path(), "src/federation.rs", "fn run() {}\n");
    loom_ok(tmp.path(), &["codefile", "add", "src/federation.rs"]);

    let first = loom_json(
        tmp.path(),
        &[
            "edge",
            "implement",
            "federated behavior",
            "src/federation.rs",
            "--locator",
            "fn run",
        ],
    );
    let second = loom_json(
        tmp.path(),
        &[
            "edge",
            "implement",
            "federated behavior",
            "src/federation.rs",
            "--locator",
            "module federation",
        ],
    );

    assert_eq!(first["edge"]["id"], second["edge"]["id"]);
    assert_eq!(second["locator"], "module federation");
    let store = Store::open(tmp.path()).unwrap();
    let edges = store
        .edges_with(Some(EdgeKind::Implements), None, None)
        .unwrap();
    assert_eq!(edges.len(), 1, "re-grounding must not create a second edge");
    assert_eq!(
        store
            .get_facet(&edges[0].id, TargetKind::Edge, "locator")
            .unwrap()
            .as_deref(),
        Some("module federation")
    );
}

#[test]
fn federation_sync_fails_loudly_when_a_linked_export_disappears() {
    let upstream = Tmp::new();
    loom_init(upstream.path(), Some("upstream"));
    loom_ok(upstream.path(), &["export"]);
    let export = upstream.path().join("loom.graph.json");

    let downstream = Tmp::new();
    loom_init(downstream.path(), Some("downstream"));
    loom_json(
        downstream.path(),
        &["graph", "link", export.to_str().unwrap()],
    );
    std::fs::remove_file(&export).unwrap();

    let out = std::process::Command::new(loom_bin())
        .arg("--graph")
        .arg(downstream.path())
        .args(["sync", "--json"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "missing upstream export must fail sync"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("reading upstream export"),
        "failure must identify the unavailable federation input: {stderr}"
    );
}

// =========================================================================
// 1. Two-graph E2E: link + sync, upstream change stales DependsOn edges.
// =========================================================================

#[test]
fn federation_link_sync_and_upstream_change_stales_edges() {
    // --- upstream graph ---
    let upstream = Tmp::new();
    loom_init(upstream.path(), Some("platform"));
    loom_ok(
        upstream.path(),
        &[
            "intent",
            "add",
            "--name",
            "auth-flow",
            "--description",
            "user authentication",
        ],
    );
    write_file(upstream.path(), "src/auth.rs", "fn auth() {}");
    loom_ok(upstream.path(), &["codefile", "add", "src/auth.rs"]);
    loom_ok(
        upstream.path(),
        &["edge", "implement", "auth-flow", "src/auth.rs"],
    );
    loom_ok(upstream.path(), &["sync"]);
    loom_ok(upstream.path(), &["export"]);
    let upstream_export = upstream.path().join("loom.graph.json");

    // --- downstream graph ---
    let downstream = Tmp::new();
    loom_init(downstream.path(), Some("app"));

    // Link upstream.
    let link_j = loom_json(
        downstream.path(),
        &["graph", "link", upstream_export.to_str().unwrap()],
    );
    assert_eq!(link_j["linked"], true, "link succeeded: {link_j}");
    assert!(
        link_j["shadow_nodes"].as_i64().unwrap() >= 1,
        "shadows created: {link_j}"
    );

    // Create a local intent and a DependsOn edge.
    loom_ok(
        downstream.path(),
        &[
            "intent",
            "add",
            "--name",
            "login-page",
            "--description",
            "login UI",
        ],
    );
    loom_ok(
        downstream.path(),
        &[
            "edge",
            "depends-on",
            "login-page",
            "upstream/platform/auth-flow",
        ],
    );

    // First sync — edge stays clean (nothing changed upstream).
    loom_ok(downstream.path(), &["sync"]);
    {
        let store = Store::open(downstream.path()).unwrap();
        let edges = store
            .edges_with(Some(EdgeKind::DependsOn), None, None)
            .unwrap();
        assert_eq!(edges.len(), 1);
        // Edge should be uninspected (just created), not staled.
        assert_eq!(edges[0].status, InspectionStatus::Uninspected);
    }

    // Verdict the edge so staleness is observable.
    let edge_id = {
        let store = Store::open(downstream.path()).unwrap();
        let edges = store
            .edges_with(Some(EdgeKind::DependsOn), None, None)
            .unwrap();
        edges[0].id.clone()
    };
    loom_ok(
        downstream.path(),
        &[
            "edge",
            "verdict",
            &edge_id,
            "ground",
            "--criterion",
            "auth-flow exists",
            "--evidence",
            "linked upstream",
        ],
    );

    // --- upstream changes ---
    loom_ok(
        upstream.path(),
        &[
            "intent",
            "update",
            "auth-flow",
            "--description",
            "user authentication v2",
            "--reason",
            "testing federation staleness",
        ],
    );
    loom_ok(upstream.path(), &["sync"]);
    loom_ok(upstream.path(), &["export"]);

    // Sync downstream — the DependsOn edge must be staled.
    let sync_j = loom_json(downstream.path(), &["sync"]);
    let fed = &sync_j["federation"];
    assert!(
        fed["shadows_updated"].as_i64().unwrap() >= 1,
        "shadow updated: {sync_j}"
    );
    assert!(
        fed["edges_staled"].as_i64().unwrap() >= 1,
        "edge staled: {sync_j}"
    );

    // Verify edge status in the store.
    let store = Store::open(downstream.path()).unwrap();
    let edges = store
        .edges_with(Some(EdgeKind::DependsOn), None, None)
        .unwrap();
    assert_eq!(edges[0].status, InspectionStatus::NeedsReverification);

    // Verify the stale_cause facet was set.
    let cause = store
        .get_facet(&edges[0].id, TargetKind::Edge, "stale_cause")
        .unwrap();
    assert!(
        cause.is_some(),
        "stale_cause facet must be set on the staled edge"
    );
}

// =========================================================================
// 2. Upstream shadows are invisible to local intent queries/queues.
// =========================================================================

#[test]
fn upstream_shadows_invisible_to_local_queues() {
    let upstream = Tmp::new();
    loom_init(upstream.path(), Some("lib"));
    loom_ok(
        upstream.path(),
        &[
            "intent",
            "add",
            "--name",
            "core-fn",
            "--description",
            "core function",
        ],
    );
    loom_ok(upstream.path(), &["export"]);
    let upstream_export = upstream.path().join("loom.graph.json");

    let downstream = Tmp::new();
    loom_init(downstream.path(), Some("app"));
    loom_ok(
        downstream.path(),
        &["graph", "link", upstream_export.to_str().unwrap()],
    );
    loom_ok(
        downstream.path(),
        &[
            "intent",
            "add",
            "--name",
            "local-feat",
            "--description",
            "a local feature",
        ],
    );
    loom_ok(downstream.path(), &["sync"]);

    // Status should count only the local intent.
    let status_j = loom_json(downstream.path(), &["status"]);
    assert_eq!(
        status_j["counts"]["intents"].as_i64().unwrap(),
        1,
        "only local intent counted in status: {status_j}"
    );

    // Maturity ladder uses list_nodes(Intent) — shadows must not inflate it.
    let store = Store::open(downstream.path()).unwrap();
    let intents = store
        .list_nodes(Some(NodeType::Intent), usize::MAX)
        .unwrap();
    assert_eq!(intents.len(), 1, "only local intent in Intent list");
    let upstream_intents = store
        .list_nodes(Some(NodeType::UpstreamIntent), usize::MAX)
        .unwrap();
    assert!(
        !upstream_intents.is_empty(),
        "upstream shadow exists as UpstreamIntent"
    );
}

// =========================================================================
// 3. Unlink leaves shadow nodes orphaned (no deletion).
// =========================================================================

#[test]
fn unlink_orphans_shadows_for_doctor() {
    let upstream = Tmp::new();
    loom_init(upstream.path(), Some("svc"));
    loom_ok(
        upstream.path(),
        &[
            "intent",
            "add",
            "--name",
            "endpoint-a",
            "--description",
            "an endpoint",
        ],
    );
    loom_ok(upstream.path(), &["export"]);
    let upstream_export = upstream.path().join("loom.graph.json");

    let downstream = Tmp::new();
    loom_init(downstream.path(), Some("client"));
    loom_ok(
        downstream.path(),
        &["graph", "link", upstream_export.to_str().unwrap()],
    );

    // Shadow exists.
    {
        let store = Store::open(downstream.path()).unwrap();
        let shadows = store
            .list_nodes(Some(NodeType::UpstreamIntent), usize::MAX)
            .unwrap();
        assert_eq!(shadows.len(), 1, "one shadow after link");
    }

    // Unlink.
    loom_ok(downstream.path(), &["graph", "unlink", "svc"]);

    // Shadow is still there (orphaned, not deleted).
    let store = Store::open(downstream.path()).unwrap();
    let shadows = store
        .list_nodes(Some(NodeType::UpstreamIntent), usize::MAX)
        .unwrap();
    assert_eq!(
        shadows.len(),
        1,
        "shadow survives unlink (orphan, not deleted)"
    );

    // No upstream registrations remain.
    let entries = loom::federation::read_upstream_entries(&store).unwrap();
    assert!(entries.is_empty(), "no upstreams registered after unlink");
    drop(store);

    // Doctor flags the orphaned shadow (exits non-zero with issues).
    let out = {
        let mut cmd = std::process::Command::new(loom_bin());
        cmd.arg("--graph")
            .arg(downstream.path())
            .args(["doctor", "--json"]);
        cmd.output().unwrap()
    };
    // Doctor exits non-zero when issues exist — that's expected.
    assert!(
        !out.status.success(),
        "doctor should exit non-zero with orphaned shadow"
    );
    let issues: Vec<serde_json::Value> =
        serde_json::from_slice(&out.stdout).expect("doctor JSON array");
    assert!(
        issues
            .iter()
            .any(|i| i["kind"] == "orphaned_upstream_intent"),
        "doctor must flag orphaned upstream intent: {issues:?}"
    );
    // Doctor message must name the remediation — orphans without a cleanup path
    // left hardened maturity unreachable after intentional permanent unlink.
    let orphan_msg = issues
        .iter()
        .find(|i| i["kind"] == "orphaned_upstream_intent")
        .and_then(|i| i["message"].as_str())
        .unwrap_or("");
    assert!(
        orphan_msg.contains("prune-orphans"),
        "doctor orphan message must name prune-orphans remediation: {orphan_msg}"
    );
}

// =========================================================================
// 3b. Permanent dispose: prune-orphans after unlink clears doctor.
// =========================================================================

#[test]
fn prune_orphans_after_unlink_clears_doctor() {
    // graph list must surface orphan residue when the registry is empty —
    // the product-graph failure mode was "list []" while doctor blocked hardened.
    let upstream = Tmp::new();
    loom_init(upstream.path(), Some("svc"));
    loom_ok(
        upstream.path(),
        &[
            "intent",
            "add",
            "--name",
            "endpoint-a",
            "--description",
            "an endpoint",
        ],
    );
    loom_ok(upstream.path(), &["export"]);
    let upstream_export = upstream.path().join("loom.graph.json");

    let downstream = Tmp::new();
    loom_init(downstream.path(), Some("client"));
    loom_ok(
        downstream.path(),
        &["graph", "link", upstream_export.to_str().unwrap()],
    );
    loom_ok(downstream.path(), &["graph", "unlink", "svc"]);

    let listed = loom_json(downstream.path(), &["graph", "list"]);
    assert_eq!(
        listed["orphan_shadows"].as_u64().unwrap(),
        1,
        "list must report orphan residue, not a silent empty array: {listed}"
    );
    assert_eq!(listed["hint"], "loom graph prune-orphans");

    let pruned = loom_json(downstream.path(), &["graph", "prune-orphans"]);
    assert_eq!(pruned["pruned"]["count"].as_u64().unwrap(), 1);
    assert!(pruned["pruned"]["blocked"].as_array().unwrap().is_empty());

    let store = Store::open(downstream.path()).unwrap();
    let shadows = store
        .list_nodes(Some(NodeType::UpstreamIntent), usize::MAX)
        .unwrap();
    assert!(shadows.is_empty(), "all orphan shadows disposed");
    drop(store);

    // After prune, list is a plain empty array again (no false orphan envelope).
    let listed_clean = loom_json(downstream.path(), &["graph", "list"]);
    assert!(
        listed_clean
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(false),
        "clean list must be []: {listed_clean}"
    );

    loom_ok(downstream.path(), &["doctor"]);
}

// =========================================================================
// 3c. unlink --prune disposes shadows in the same step.
// =========================================================================

#[test]
fn unlink_prune_disposes_shadows_immediately() {
    let upstream = Tmp::new();
    loom_init(upstream.path(), Some("svc"));
    loom_ok(
        upstream.path(),
        &[
            "intent",
            "add",
            "--name",
            "endpoint-a",
            "--description",
            "an endpoint",
        ],
    );
    loom_ok(upstream.path(), &["export"]);
    let upstream_export = upstream.path().join("loom.graph.json");

    let downstream = Tmp::new();
    loom_init(downstream.path(), Some("client"));
    loom_ok(
        downstream.path(),
        &["graph", "link", upstream_export.to_str().unwrap()],
    );

    let out = loom_json(downstream.path(), &["graph", "unlink", "svc", "--prune"]);
    assert_eq!(out["pruned"]["count"].as_u64().unwrap(), 1);

    let store = Store::open(downstream.path()).unwrap();
    assert!(store
        .list_nodes(Some(NodeType::UpstreamIntent), usize::MAX)
        .unwrap()
        .is_empty());
    drop(store);
    loom_ok(downstream.path(), &["doctor"]);
}

// =========================================================================
// 3d. prune refuses orphans still targeted by DependsOn; --cascade forces.
// =========================================================================

#[test]
fn prune_orphans_refuses_depends_on_unless_cascade() {
    let upstream = Tmp::new();
    loom_init(upstream.path(), Some("svc"));
    loom_ok(
        upstream.path(),
        &[
            "intent",
            "add",
            "--name",
            "endpoint-a",
            "--description",
            "an endpoint",
        ],
    );
    loom_ok(
        upstream.path(),
        &[
            "intent",
            "add",
            "--name",
            "endpoint-b",
            "--description",
            "another endpoint",
        ],
    );
    loom_ok(upstream.path(), &["export"]);
    let upstream_export = upstream.path().join("loom.graph.json");

    let downstream = Tmp::new();
    loom_init(downstream.path(), Some("client"));
    loom_ok(
        downstream.path(),
        &["graph", "link", upstream_export.to_str().unwrap()],
    );
    loom_ok(
        downstream.path(),
        &[
            "intent",
            "add",
            "--name",
            "local-feature",
            "--description",
            "depends on upstream",
        ],
    );
    loom_ok(
        downstream.path(),
        &[
            "edge",
            "depends-on",
            "local-feature",
            "upstream/svc/endpoint-a",
        ],
    );
    loom_ok(downstream.path(), &["graph", "unlink", "svc"]);

    // Without --cascade: free orphan (endpoint-b) is pruned; claimed one blocks.
    let mut cmd = std::process::Command::new(loom_bin());
    cmd.arg("--graph")
        .arg(downstream.path())
        .args(["graph", "prune-orphans", "--json"]);
    let out = cmd.output().unwrap();
    // Partial success: one pruned, one blocked — should still exit 0.
    assert!(
        out.status.success(),
        "partial prune should succeed: {}\n{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let body: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(body["pruned"]["count"].as_u64().unwrap(), 1, "{body}");
    assert_eq!(
        body["pruned"]["blocked"].as_array().unwrap().len(),
        1,
        "{body}"
    );

    let store = Store::open(downstream.path()).unwrap();
    let remaining = store
        .list_nodes(Some(NodeType::UpstreamIntent), usize::MAX)
        .unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].name, "upstream/svc/endpoint-a");
    let deps = store
        .edges_with(Some(EdgeKind::DependsOn), None, None)
        .unwrap();
    assert_eq!(
        deps.len(),
        1,
        "DependsOn edge still present without cascade"
    );
    drop(store);

    // Pure-blocked re-run (only the claimed orphan left) must exit non-zero.
    let mut cmd = std::process::Command::new(loom_bin());
    cmd.arg("--graph")
        .arg(downstream.path())
        .args(["graph", "prune-orphans", "--json"]);
    let blocked_only = cmd.output().unwrap();
    assert!(
        !blocked_only.status.success(),
        "all-blocked prune must fail closed"
    );

    // --cascade removes the last shadow and its DependsOn edge.
    let cascaded = loom_json(downstream.path(), &["graph", "prune-orphans", "--cascade"]);
    assert_eq!(cascaded["pruned"]["count"].as_u64().unwrap(), 1);
    assert_eq!(cascaded["pruned"]["cascade_edges"].as_u64().unwrap(), 1);

    let store = Store::open(downstream.path()).unwrap();
    assert!(store
        .list_nodes(Some(NodeType::UpstreamIntent), usize::MAX)
        .unwrap()
        .is_empty());
    assert!(store
        .edges_with(Some(EdgeKind::DependsOn), None, None)
        .unwrap()
        .is_empty());
    drop(store);
    loom_ok(downstream.path(), &["doctor"]);
}

// =========================================================================
// 4. wipe_derived + sync converges: derived facets on shadows are rebuilt.
// =========================================================================

#[test]
fn wipe_derived_then_sync_restores_upstream_facets() {
    let upstream = Tmp::new();
    loom_init(upstream.path(), Some("plat"));
    loom_ok(
        upstream.path(),
        &[
            "intent",
            "add",
            "--name",
            "pay",
            "--description",
            "payments",
        ],
    );
    loom_ok(upstream.path(), &["export"]);
    let upstream_export = upstream.path().join("loom.graph.json");

    let downstream = Tmp::new();
    loom_init(downstream.path(), Some("shop"));
    loom_ok(
        downstream.path(),
        &["graph", "link", upstream_export.to_str().unwrap()],
    );
    loom_ok(downstream.path(), &["sync"]);

    // Verify facets exist.
    let store = Store::open(downstream.path()).unwrap();
    let shadows = store
        .list_nodes(Some(NodeType::UpstreamIntent), usize::MAX)
        .unwrap();
    assert_eq!(shadows.len(), 1);
    let shadow_id = &shadows[0].id;
    assert!(
        store
            .get_facet(shadow_id, TargetKind::Node, "upstream_content_hash")
            .unwrap()
            .is_some(),
        "facet exists before wipe"
    );

    // Wipe derived data.
    store.wipe_derived().unwrap();

    // Facets are gone.
    assert!(
        store
            .get_facet(shadow_id, TargetKind::Node, "upstream_content_hash")
            .unwrap()
            .is_none(),
        "facet gone after wipe"
    );

    // Shadow node itself survives (asserted).
    assert!(
        store.get_node(shadow_id).unwrap().is_some(),
        "shadow node survives wipe (asserted)"
    );
    drop(store);

    // Sync restores the facets.
    loom_ok(downstream.path(), &["sync"]);

    let store = Store::open(downstream.path()).unwrap();
    assert!(
        store
            .get_facet(shadow_id, TargetKind::Node, "upstream_content_hash")
            .unwrap()
            .is_some(),
        "facet restored after sync (INV-2 convergence)"
    );
    assert!(
        store
            .get_facet(shadow_id, TargetKind::Node, "upstream_description")
            .unwrap()
            .is_some(),
        "upstream_description facet restored"
    );
}
