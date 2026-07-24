//! Debt promotion integration tests — INV-3 feed/queue separation and the
//! `loom debt promote` write boundary.
//!
//! Fixture: four CodeFiles with loc facets 40/50/45/5000 produce exactly one
//! `size_outlier` cluster without git history. Every mutation path goes through
//! the real `loom` binary; Store is used only to seed and to inspect after CLI
//! writes (and is dropped before any CLI spawn so the graph lock is free).

use loom::model::{EdgeKind, NodeType, TargetKind, TruthClass};
use loom::store::Store;
use std::path::Path;
use std::process::Command;

mod common;
use common::*;

// ---- CLI helpers ------------------------------------------------------------

fn loom_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_loom"))
}

fn loom_ok(tmp: &Path, args: &[&str]) -> String {
    let mut cmd = Command::new(loom_bin());
    cmd.arg("--graph").arg(tmp).args(args);
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("spawn loom {:?}: {e}", args));
    assert!(
        out.status.success(),
        "loom {:?} failed:\nstderr: {}\nstdout: {}",
        args,
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn loom_json(tmp: &Path, args: &[&str]) -> serde_json::Value {
    let mut cmd = Command::new(loom_bin());
    cmd.arg("--graph").arg(tmp).args(args).arg("--json");
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("spawn loom {:?}: {e}", args));
    assert!(
        out.status.success(),
        "loom {:?} --json failed:\nstderr: {}\nstdout: {}",
        args,
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "loom {:?} --json did not emit JSON:\n{}\nparse error: {e}",
            args, stdout
        )
    })
}

/// Global `--json` before the subcommand (`loom --json debt`), distinct from
/// the trailing form exercised by `loom_json`.
fn loom_json_global(tmp: &Path, args: &[&str]) -> serde_json::Value {
    let mut cmd = Command::new(loom_bin());
    cmd.arg("--json").arg("--graph").arg(tmp).args(args);
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("spawn loom --json {:?}: {e}", args));
    assert!(
        out.status.success(),
        "loom --json {:?} failed:\nstderr: {}\nstdout: {}",
        args,
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "loom --json {:?} did not emit JSON:\n{}\nparse error: {e}",
            args, stdout
        )
    })
}

fn loom_err(tmp: &Path, args: &[&str]) -> String {
    let mut cmd = Command::new(loom_bin());
    cmd.arg("--graph").arg(tmp).args(args);
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("spawn loom {:?}: {e}", args));
    assert!(
        !out.status.success(),
        "loom {:?} should have failed, stdout:\n{}",
        args,
        String::from_utf8_lossy(&out.stdout)
    );
    // Prefer stderr; fall back to stdout when the binary prints the error there.
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !stderr.trim().is_empty() {
        stderr.into_owned()
    } else {
        stdout.into_owned()
    }
}

// ---- Fixture ----------------------------------------------------------------

/// Seed four CodeFiles with loc 40/50/45/5000 → one size_outlier on `big.rs`.
/// Returns the temp dir; the Store is dropped so CLI can take the write lock.
fn fixture_size_outlier() -> (Tmp, String) {
    let tmp = Tmp::new();
    let big_id = {
        let store = Store::init(tmp.path(), Some("debt-promo"), false).unwrap();
        let mut big = String::new();
        for (path, loc) in [("a.rs", 40), ("b.rs", 50), ("c.rs", 45), ("big.rs", 5000)] {
            let n = store
                .add_node(NodeType::CodeFile, path, "", "", serde_json::json!({}))
                .unwrap();
            store
                .set_facet(
                    &n.id,
                    TargetKind::Node,
                    "loc",
                    &loc.to_string(),
                    TruthClass::Derived,
                )
                .unwrap();
            if path == "big.rs" {
                big = n.id;
            }
        }
        let debt = loom::signal::debt(&store).unwrap();
        assert!(
            debt.iter().any(|d| d.kind == "size_outlier"),
            "fixture must produce a size_outlier: {debt:?}"
        );
        assert_eq!(
            debt.iter().filter(|d| d.kind == "size_outlier").count(),
            1,
            "fixture must produce exactly one size_outlier"
        );
        big
    };
    (tmp, big_id)
}

fn graph_counts(tmp: &Path) -> (usize, usize, usize) {
    let store = Store::open(tmp).unwrap();
    let snap = store.snapshot().unwrap();
    (snap.nodes.len(), snap.edges.len(), snap.facets.len())
}

fn live_size_outlier_cluster(tmp: &Path) -> (String, serde_json::Value) {
    let feed = loom_json(tmp, &["debt"]);
    let arr = feed
        .as_array()
        .unwrap_or_else(|| panic!("debt --json must be a top-level array, got: {feed}"));
    let cluster = arr
        .iter()
        .find(|c| c.get("kind").and_then(|v| v.as_str()) == Some("size_outlier"))
        .unwrap_or_else(|| panic!("live feed must contain size_outlier: {feed}"))
        .clone();
    let id = cluster
        .get("cluster_id")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("cluster must carry cluster_id: {cluster}"))
        .to_string();
    (id, cluster)
}

fn finding_count(tmp: &Path) -> usize {
    let store = Store::open(tmp).unwrap();
    store
        .list_nodes(Some(NodeType::Finding), usize::MAX)
        .unwrap()
        .len()
}

fn flags_or_assesses_edges(tmp: &Path) -> usize {
    let store = Store::open(tmp).unwrap();
    let edges = store.list_edges(None, usize::MAX).unwrap();
    edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Flags || e.kind == EdgeKind::Assesses)
        .count()
}

/// `statistical` is not a storable TruthClass. Guard the write surface by
/// ensuring no Flags/Assesses edges and that no Finding carries a non-asserted
/// truth class after promotion (INV-3).
fn inv3_surface_clean(tmp: &Path) -> bool {
    let store = Store::open(tmp).unwrap();
    let flags = store
        .list_edges(Some(EdgeKind::Flags), usize::MAX)
        .unwrap()
        .len();
    let assesses = store
        .list_edges(Some(EdgeKind::Assesses), usize::MAX)
        .unwrap()
        .len();
    let bad_finding = store
        .list_nodes(Some(NodeType::Finding), usize::MAX)
        .unwrap()
        .into_iter()
        .any(|n| {
            n.truth_class != TruthClass::Asserted
                && n.body.get("source").and_then(|v| v.as_str()) == Some("debt_promotion")
        });
    flags == 0 && assesses == 0 && !bad_finding
}

// ---- 1. Backward-compatible list / parse ------------------------------------

#[test]
fn debt_list_text_and_json_shapes_are_backward_compatible() {
    let (tmp, _) = fixture_size_outlier();

    // Text: human feed prints the stable id line.
    let text = loom_ok(tmp.path(), &["debt"]);
    assert!(
        text.contains("size_outlier"),
        "text debt feed must name the kind: {text}"
    );
    assert!(
        text.lines().any(|l| l.trim_start().starts_with("id: c")),
        "text debt feed must include `    id: c…` line, got:\n{text}"
    );

    // Trailing --json: top-level array with stable cluster_id.
    let trailing = loom_json(tmp.path(), &["debt"]);
    let arr = trailing
        .as_array()
        .unwrap_or_else(|| panic!("debt --json must be top-level array: {trailing}"));
    assert!(!arr.is_empty(), "debt feed must not be empty");
    for c in arr {
        let id = c
            .get("cluster_id")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| panic!("each cluster needs cluster_id: {c}"));
        assert!(
            id.starts_with('c') && id.len() == 17,
            "cluster_id must be c + 16 hex, got {id:?}"
        );
        assert!(
            c.get("subject_ids").is_none(),
            "subject_ids must not serialize into the feed: {c}"
        );
        for key in ["kind", "message", "impact", "confirm"] {
            assert!(c.get(key).is_some(), "cluster missing field {key}: {c}");
        }
    }

    // Global --json debt (flag before subcommand) must match the same contract.
    let global = loom_json_global(tmp.path(), &["debt"]);
    let garr = global
        .as_array()
        .unwrap_or_else(|| panic!("loom --json debt must be top-level array: {global}"));
    assert_eq!(
        garr.len(),
        arr.len(),
        "global and trailing --json debt must agree on count"
    );
    assert_eq!(
        garr[0]["cluster_id"], arr[0]["cluster_id"],
        "cluster_id must be stable across --json placements"
    );

    // Read-only: debt list must not mutate the graph.
    let (n0, e0, f0) = graph_counts(tmp.path());
    let _ = loom_ok(tmp.path(), &["debt"]);
    let (n1, e1, f1) = graph_counts(tmp.path());
    assert_eq!((n0, e0, f0), (n1, e1, f1), "loom debt must be read-only");
}

// ---- 2. Successful promotion ------------------------------------------------

#[test]
fn promote_by_unique_prefix_mints_asserted_finding_with_full_provenance() {
    let (tmp, big_id) = fixture_size_outlier();
    let (nodes_before, edges_before, facets_before) = graph_counts(tmp.path());
    let findings_before = finding_count(tmp.path());
    let flags_before = flags_or_assesses_edges(tmp.path());
    assert!(inv3_surface_clean(tmp.path()));

    let (cluster_id, live) = live_size_outlier_cluster(tmp.path());
    // Unique prefix: with a single cluster any proper prefix of the live id is
    // unambiguous; use enough hex to stay intentional without requiring exact.
    let prefix = &cluster_id[..8.min(cluster_id.len())];
    assert!(
        prefix.len() >= 2 && prefix.starts_with('c'),
        "prefix under test must look like a cluster id fragment: {prefix}"
    );

    let evidence = "big.rs is a genuine cohesion unit; split only if a second seam appears";
    let out = loom_json(
        tmp.path(),
        &[
            "debt",
            "promote",
            prefix,
            "--evidence",
            evidence,
            "--confidence",
            "0.85",
        ],
    );

    assert_eq!(
        out.get("cluster_id").and_then(|v| v.as_str()),
        Some(cluster_id.as_str()),
        "response cluster_id must be the live full id"
    );
    assert_eq!(
        out.get("destination").and_then(|v| v.as_str()),
        Some("finding")
    );
    assert_eq!(out.get("created").and_then(|v| v.as_bool()), Some(true));
    assert!(
        out.get("next_step").and_then(|v| v.as_str()).is_some(),
        "pulse must attach next_step: {out}"
    );
    assert!(
        out.get("graph_state").is_some(),
        "pulse must attach graph_state: {out}"
    );

    let finding = out
        .get("finding")
        .unwrap_or_else(|| panic!("promote response missing finding: {out}"));
    let expected_finding_id = format!("p{}", &cluster_id[1..]);
    assert_eq!(
        finding.get("id").and_then(|v| v.as_str()),
        Some(expected_finding_id.as_str()),
        "Finding id must be p + cluster digest"
    );
    assert_eq!(
        finding.get("type").and_then(|v| v.as_str()),
        Some("finding")
    );
    assert_eq!(
        finding.get("truth_class").and_then(|v| v.as_str()),
        Some("asserted")
    );
    assert_eq!(
        finding.get("status").and_then(|v| v.as_str()),
        Some("size_outlier")
    );

    let body = finding
        .get("body")
        .unwrap_or_else(|| panic!("finding body missing: {finding}"));
    assert_eq!(
        body.get("source").and_then(|v| v.as_str()),
        Some("debt_promotion")
    );
    assert_eq!(
        body.get("kind").and_then(|v| v.as_str()),
        Some("size_outlier")
    );
    assert_eq!(
        body.get("evidence").and_then(|v| v.as_str()),
        Some(evidence)
    );
    assert_eq!(body.get("confidence").and_then(|v| v.as_f64()), Some(0.85));
    assert_eq!(
        body.get("impact").and_then(|v| v.as_u64()),
        live.get("impact").and_then(|v| v.as_u64()),
        "numeric impact must match the live cluster"
    );
    assert_eq!(
        body.get("file").and_then(|v| v.as_str()),
        Some("big.rs"),
        "canonical subject name must land on body.file"
    );

    let snap = body
        .get("debt_cluster")
        .unwrap_or_else(|| panic!("body must nest immutable debt_cluster snapshot: {body}"));
    assert_eq!(
        snap.get("id").and_then(|v| v.as_str()),
        Some(cluster_id.as_str())
    );
    assert_eq!(
        snap.get("kind").and_then(|v| v.as_str()),
        Some("size_outlier")
    );
    assert_eq!(
        snap.get("message").and_then(|v| v.as_str()),
        live.get("message").and_then(|v| v.as_str())
    );
    assert_eq!(snap.get("impact"), live.get("impact"));
    assert_eq!(snap.get("confirm"), live.get("confirm"));
    let subjects = snap
        .get("subject_ids")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("debt_cluster.subject_ids required: {snap}"));
    assert_eq!(subjects.len(), 1);
    assert_eq!(
        subjects[0].as_str(),
        Some(big_id.as_str()),
        "subject_ids must preserve the live CodeFile id"
    );

    // Graph deltas: exactly one new Finding node; edges/facets unchanged.
    let (nodes_after, edges_after, facets_after) = graph_counts(tmp.path());
    assert_eq!(nodes_after, nodes_before + 1, "exactly one node added");
    assert_eq!(edges_after, edges_before, "promotion creates no edges");
    assert_eq!(facets_after, facets_before, "promotion creates no facets");
    assert_eq!(finding_count(tmp.path()), findings_before + 1);
    assert_eq!(
        flags_or_assesses_edges(tmp.path()),
        flags_before,
        "no Flags/Assesses edge may appear"
    );
    assert!(
        inv3_surface_clean(tmp.path()),
        "no Flags/Assesses edges; debt_promotion Findings stay asserted"
    );

    // Direct Store inspection of the persisted node.
    {
        let store = Store::open(tmp.path()).unwrap();
        let node = store
            .get_node(&expected_finding_id)
            .unwrap()
            .expect("promoted finding must exist in Store");
        assert_eq!(node.node_type, NodeType::Finding);
        assert_eq!(node.truth_class, TruthClass::Asserted);
        assert_eq!(node.status, "size_outlier");
        assert_eq!(
            node.body.get("source").and_then(|v| v.as_str()),
            Some("debt_promotion")
        );
        assert!(store
            .list_edges(Some(EdgeKind::Flags), usize::MAX)
            .unwrap()
            .is_empty());
        assert!(store
            .list_edges(Some(EdgeKind::Assesses), usize::MAX)
            .unwrap()
            .is_empty());
    }
}

// ---- 3. Idempotency / conflict ----------------------------------------------

#[test]
fn promote_replay_is_idempotent_and_conflicts_on_payload_change() {
    let (tmp, _) = fixture_size_outlier();
    let (cluster_id, _) = live_size_outlier_cluster(tmp.path());
    let evidence = "operator confirmed size_outlier after reading the module boundary";
    let confidence = "0.85";

    let first = loom_json(
        tmp.path(),
        &[
            "debt",
            "promote",
            &cluster_id,
            "--evidence",
            evidence,
            "--confidence",
            confidence,
        ],
    );
    assert_eq!(first.get("created").and_then(|v| v.as_bool()), Some(true));
    let finding_id = first["finding"]["id"].as_str().unwrap().to_string();
    let created_at = first["finding"]["created_at"].as_str().unwrap().to_string();
    let updated_at = first["finding"]["updated_at"].as_str().unwrap().to_string();
    let body_first = first["finding"]["body"].clone();

    // Small sleep only to detect timestamp churn on a false re-write.
    std::thread::sleep(std::time::Duration::from_millis(25));

    let replay = loom_json(
        tmp.path(),
        &[
            "debt",
            "promote",
            &cluster_id,
            "--evidence",
            evidence,
            "--confidence",
            confidence,
        ],
    );
    assert_eq!(
        replay.get("created").and_then(|v| v.as_bool()),
        Some(false),
        "identical evidence/confidence must be a no-op: {replay}"
    );
    assert_eq!(replay["finding"]["id"].as_str(), Some(finding_id.as_str()));
    assert_eq!(
        replay["finding"]["created_at"].as_str(),
        Some(created_at.as_str()),
        "created_at must not churn on idempotent replay"
    );
    assert_eq!(
        replay["finding"]["updated_at"].as_str(),
        Some(updated_at.as_str()),
        "updated_at must not churn on idempotent replay"
    );
    assert_eq!(
        replay["finding"]["body"], body_first,
        "finding body must be byte-identical on replay"
    );
    assert_eq!(finding_count(tmp.path()), 1, "still exactly one Finding");

    let (nodes_mid, edges_mid, facets_mid) = graph_counts(tmp.path());

    // Changed evidence → conflict, no write.
    let err_ev = loom_err(
        tmp.path(),
        &[
            "debt",
            "promote",
            &cluster_id,
            "--evidence",
            "different evidence that conflicts with the stored promotion",
            "--confidence",
            confidence,
        ],
    );
    assert!(
        err_ev.contains("already promoted") || err_ev.contains("different evidence"),
        "changed evidence must conflict with actionable phrase, got: {err_ev}"
    );

    // Changed confidence → conflict, no write.
    let err_cf = loom_err(
        tmp.path(),
        &[
            "debt",
            "promote",
            &cluster_id,
            "--evidence",
            evidence,
            "--confidence",
            "0.5",
        ],
    );
    assert!(
        err_cf.contains("already promoted")
            || err_cf.contains("different")
            || err_cf.contains("confidence"),
        "changed confidence must conflict with actionable phrase, got: {err_cf}"
    );

    let (nodes_end, edges_end, facets_end) = graph_counts(tmp.path());
    assert_eq!(
        (nodes_mid, edges_mid, facets_mid),
        (nodes_end, edges_end, facets_end),
        "conflicts must leave the graph untouched"
    );
    assert_eq!(finding_count(tmp.path()), 1);

    {
        let store = Store::open(tmp.path()).unwrap();
        let node = store.get_node(&finding_id).unwrap().unwrap();
        assert_eq!(
            node.body.get("evidence").and_then(|v| v.as_str()),
            Some(evidence),
            "original evidence must survive conflict attempts"
        );
        assert_eq!(
            node.body.get("confidence").and_then(|v| v.as_f64()),
            Some(0.85),
            "original confidence must survive conflict attempts"
        );
        assert_eq!(node.created_at, created_at);
        assert_eq!(node.updated_at, updated_at);
    }
}

// ---- 4. Evidence / confidence gates ----------------------------------------

#[test]
fn promote_rejects_empty_placeholder_and_out_of_range_confidence() {
    let (tmp, _) = fixture_size_outlier();
    let (cluster_id, _) = live_size_outlier_cluster(tmp.path());
    let (n0, e0, f0) = graph_counts(tmp.path());
    let findings0 = finding_count(tmp.path());

    // Evidence gates.
    for (label, evidence) in [
        ("empty", ""),
        ("whitespace", "   \t  "),
        ("ellipsis", "…"),
        ("angle-hole", "<evidence>"),
    ] {
        let err = loom_err(
            tmp.path(),
            &[
                "debt",
                "promote",
                &cluster_id,
                "--evidence",
                evidence,
                "--confidence",
                "0.7",
            ],
        );
        assert!(
            err.contains("substantive") || err.contains("evidence") || err.contains("placeholder"),
            "evidence gate ({label}) must mention actionable evidence phrase, got: {err}"
        );
        let (n, e, f) = graph_counts(tmp.path());
        assert_eq!(
            (n, e, f),
            (n0, e0, f0),
            "evidence gate ({label}) must not write"
        );
        assert_eq!(finding_count(tmp.path()), findings0);
    }

    // Confidence range / finiteness gates.
    for (label, conf) in [
        ("below", "-0.1"),
        ("above", "1.1"),
        ("nan", "NaN"),
        ("inf", "inf"),
        ("neginf", "-inf"),
    ] {
        let mut cmd = Command::new(loom_bin());
        cmd.arg("--graph").arg(tmp.path()).args([
            "debt",
            "promote",
            &cluster_id,
            "--evidence",
            "substantive operator evidence that is not a placeholder",
            "--confidence",
            conf,
        ]);
        let out = cmd.output().expect("spawn promote confidence gate");
        // Clap may reject some tokens before our validator; either path is a
        // failed promote with no graph mutation.
        if out.status.success() {
            panic!(
                "confidence gate ({label}={conf}) must fail, stdout: {}",
                String::from_utf8_lossy(&out.stdout)
            );
        }
        let err = {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stdout = String::from_utf8_lossy(&out.stdout);
            if !stderr.trim().is_empty() {
                stderr.into_owned()
            } else {
                stdout.into_owned()
            }
        };
        // Actionable app phrase when our validator runs; clap's own message is
        // acceptable for tokens it rejects first (do not pin clap wording).
        if !err.to_ascii_lowercase().contains("invalid")
            && !err.contains("confidence")
            && !err.contains("finite")
            && !err.contains("between")
            && !err.contains("parse")
        {
            // Still require *some* failure signal; graph check below is the
            // write-boundary honesty contract.
            assert!(
                !out.status.success(),
                "confidence gate ({label}) failed without message: {err}"
            );
        }
        let (n, e, f) = graph_counts(tmp.path());
        assert_eq!(
            (n, e, f),
            (n0, e0, f0),
            "confidence gate ({label}={conf}) must not write"
        );
        assert_eq!(finding_count(tmp.path()), findings0);
    }
}

#[test]
fn promote_accepts_confidence_boundary_zero() {
    // Independent fixture so success does not collide with other gate tests.
    let (tmp, _) = fixture_size_outlier();
    let (cluster_id, _) = live_size_outlier_cluster(tmp.path());
    let out = loom_json(
        tmp.path(),
        &[
            "debt",
            "promote",
            &cluster_id,
            "--evidence",
            "boundary confidence 0.0 is allowed for an honest low-certainty promotion",
            "--confidence",
            "0.0",
        ],
    );
    assert_eq!(out.get("created").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(out["finding"]["body"]["confidence"].as_f64(), Some(0.0));
    assert_eq!(finding_count(tmp.path()), 1);
}

#[test]
fn promote_accepts_confidence_boundary_one() {
    let (tmp, _) = fixture_size_outlier();
    let (cluster_id, _) = live_size_outlier_cluster(tmp.path());
    let out = loom_json(
        tmp.path(),
        &[
            "debt",
            "promote",
            &cluster_id,
            "--evidence",
            "boundary confidence 1.0 is allowed for a fully certain promotion",
            "--confidence",
            "1.0",
        ],
    );
    assert_eq!(out.get("created").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(out["finding"]["body"]["confidence"].as_f64(), Some(1.0));
    assert_eq!(finding_count(tmp.path()), 1);
}

// ---- 5. Resolution / staleness ----------------------------------------------

#[test]
fn promote_rejects_unknown_ambiguous_and_stale_cluster_ids() {
    let (tmp, big_id) = fixture_size_outlier();
    let (n0, e0, f0) = graph_counts(tmp.path());
    let findings0 = finding_count(tmp.path());

    // Unknown id — fail before write.
    let err_unknown = loom_err(
        tmp.path(),
        &[
            "debt",
            "promote",
            "cdeadbeefdeadbeef",
            "--evidence",
            "would-be promotion of a nonexistent cluster",
            "--confidence",
            "0.7",
        ],
    );
    assert!(
        err_unknown.contains("no debt cluster matches") || err_unknown.contains("live feed"),
        "unknown id must name the no-match phrase, got: {err_unknown}"
    );

    // Ambiguous / too-short prefix: keep the Tukey fence low by adding more
    // small loc values, then a second huge outlier. With
    // [40,42,44,45,46,48,50,5000,8000], Q3≈50 so both big files fire.
    {
        let store = Store::open(tmp.path()).unwrap();
        for (path, loc) in [
            ("d.rs", 42),
            ("e.rs", 44),
            ("f.rs", 46),
            ("g.rs", 48),
            ("huge.rs", 8000),
        ] {
            let n = store
                .add_node(NodeType::CodeFile, path, "", "", serde_json::json!({}))
                .unwrap();
            store
                .set_facet(
                    &n.id,
                    TargetKind::Node,
                    "loc",
                    &loc.to_string(),
                    TruthClass::Derived,
                )
                .unwrap();
        }
        let debt = loom::signal::debt(&store).unwrap();
        assert!(
            debt.iter().filter(|d| d.kind == "size_outlier").count() >= 2,
            "need ≥2 size_outlier clusters for ambiguous prefix: {debt:?}"
        );
    }
    let err_amb = loom_err(
        tmp.path(),
        &[
            "debt",
            "promote",
            "c",
            "--evidence",
            "ambiguous prefix must not write",
            "--confidence",
            "0.7",
        ],
    );
    assert!(
        err_amb.contains("ambiguous"),
        "too-short shared prefix must report ambiguous, got: {err_amb}"
    );

    let (n1, e1, f1) = graph_counts(tmp.path());
    // Extra CodeFiles + loc facets were intentional seed writes; findings still 0.
    assert!(n1 >= n0 + 5, "second-outlier fixture CodeFiles were seeded");
    assert!(f1 >= f0 + 5, "second-outlier loc facets were written");
    assert_eq!(e1, e0, "resolution failures must not add edges");
    assert_eq!(finding_count(tmp.path()), findings0);

    // Staleness: shrink the original outlier so its cluster leaves the feed,
    // then promote the previously live full id → no-match, no write.
    let (stale_id, _) = live_size_outlier_cluster(tmp.path());
    // Pick the big.rs cluster specifically via subject — re-open store and
    // drop loc of big.rs under the fence, and also drop the second huge so we
    // know which id went stale.
    let previously_live = {
        let store = Store::open(tmp.path()).unwrap();
        let feed = loom::signal::debt(&store).unwrap();
        let target = feed
            .iter()
            .find(|d| d.subject_ids.iter().any(|s| s == &big_id))
            .map(|d| d.cluster_id.clone())
            .unwrap_or(stale_id);
        // Pull both outliers under the fence.
        store
            .set_facet(&big_id, TargetKind::Node, "loc", "50", TruthClass::Derived)
            .unwrap();
        if let Some(huge) = store
            .list_nodes(Some(NodeType::CodeFile), usize::MAX)
            .unwrap()
            .into_iter()
            .find(|n| n.name == "huge.rs")
        {
            store
                .set_facet(&huge.id, TargetKind::Node, "loc", "50", TruthClass::Derived)
                .unwrap();
        }
        let after = loom::signal::debt(&store).unwrap();
        assert!(
            after.iter().all(|d| d.cluster_id != target),
            "stale cluster must leave the live feed: target={target}, after={after:?}"
        );
        target
    };

    let (n_pre, e_pre, f_pre) = graph_counts(tmp.path());
    let findings_pre = finding_count(tmp.path());
    let err_stale = loom_err(
        tmp.path(),
        &[
            "debt",
            "promote",
            &previously_live,
            "--evidence",
            "stale full id must not promote after the signal disappears",
            "--confidence",
            "0.7",
        ],
    );
    assert!(
        err_stale.contains("no debt cluster matches") || err_stale.contains("live feed"),
        "stale id must no-match, got: {err_stale}"
    );
    let (n_post, e_post, f_post) = graph_counts(tmp.path());
    assert_eq!(
        (n_pre, e_pre, f_pre),
        (n_post, e_post, f_post),
        "stale promote must write nothing"
    );
    assert_eq!(finding_count(tmp.path()), findings_pre);
}

// ---- 6. Feed / queue separation (INV-3) -------------------------------------

#[test]
fn promote_keeps_raw_debt_advisory_and_serves_finding_via_triage() {
    let (tmp, _) = fixture_size_outlier();

    // Capture pre-promotion maturity / queue pulse (debt present, no Finding).
    let ladder_before = {
        let store = Store::open(tmp.path()).unwrap();
        serde_json::to_value(loom::maturity::ladder(&store).unwrap()).unwrap()
    };
    let queues_before = {
        let store = Store::open(tmp.path()).unwrap();
        serde_json::to_value(loom::maturity::depths(&store).unwrap()).unwrap()
    };

    let (cluster_id, _) = live_size_outlier_cluster(tmp.path());
    let promote = loom_json(
        tmp.path(),
        &[
            "debt",
            "promote",
            &cluster_id,
            "--evidence",
            "promoted for triage separation contract coverage",
            "--confidence",
            "0.85",
        ],
    );
    assert_eq!(promote.get("created").and_then(|v| v.as_bool()), Some(true));
    let finding_id = promote["finding"]["id"].as_str().unwrap().to_string();

    // Raw debt feed still contains the statistical row (advisory, unstored).
    let debt = loom_json(tmp.path(), &["debt"]);
    let debt_arr = debt.as_array().expect("debt --json array");
    assert!(
        debt_arr.iter().any(|c| {
            c.get("kind").and_then(|v| v.as_str()) == Some("size_outlier")
                && c.get("cluster_id").and_then(|v| v.as_str()) == Some(cluster_id.as_str())
        }),
        "raw debt feed must still list the statistical cluster after promote: {debt}"
    );

    // finding list contains the asserted promotion.
    let findings = loom_json(tmp.path(), &["finding", "list"]);
    let flist = findings.as_array().expect("finding list --json array");
    let promoted = flist
        .iter()
        .find(|fv| {
            fv.get("node")
                .and_then(|n| n.get("id"))
                .and_then(|v| v.as_str())
                == Some(finding_id.as_str())
        })
        .unwrap_or_else(|| {
            panic!("finding list must include promoted id {finding_id}: {findings}")
        });
    assert_eq!(
        promoted
            .get("node")
            .and_then(|n| n.get("truth_class"))
            .and_then(|v| v.as_str()),
        Some("asserted")
    );
    assert_eq!(
        promoted
            .get("node")
            .and_then(|n| n.get("body"))
            .and_then(|b| b.get("source"))
            .and_then(|v| v.as_str()),
        Some("debt_promotion")
    );
    assert_eq!(
        promoted.get("state").and_then(|v| v.as_str()),
        Some("untriaged")
    );

    // loom next --mode triage serves the Finding, never a statistical row.
    let next = loom_json(tmp.path(), &["next", "--mode", "triage"]);
    let item = next
        .get("work_item")
        .unwrap_or_else(|| panic!("next triage must emit work_item: {next}"));
    assert!(
        !item.is_null(),
        "triage must have work after promotion: {next}"
    );
    let target = item
        .get("target")
        .unwrap_or_else(|| panic!("work_item missing target: {item}"));
    assert_eq!(
        target.get("kind").and_then(|v| v.as_str()),
        Some("finding"),
        "triage target must be a finding, not a debt cluster: {target}"
    );
    assert_eq!(
        target.get("id").and_then(|v| v.as_str()),
        Some(finding_id.as_str())
    );
    let rendered = item.to_string();
    assert!(
        !rendered.contains("\"kind\":\"size_outlier\"") || rendered.contains(&finding_id),
        "triage payload must not be a raw statistical debt row: {item}"
    );
    // No work_item may present as a debt cluster id (c…).
    assert!(
        !target
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .starts_with('c')
            || target.get("id").and_then(|v| v.as_str()) == Some(finding_id.as_str()),
        "triage must not serve a c… cluster id: {target}"
    );
    assert!(
        finding_id.starts_with('p'),
        "promoted finding id is p-prefixed"
    );

    // Promotion itself created no edge/facet (re-check INV-3 write surface).
    assert_eq!(flags_or_assesses_edges(tmp.path()), 0);
    assert!(
        inv3_surface_clean(tmp.path()),
        "INV-3: no Flags/Assesses; promoted Finding remains asserted"
    );

    // Raw debt never changes maturity/queue counts by itself; the asserted
    // Finding's ordinary triage effect is the only permitted queue delta.
    {
        let store = Store::open(tmp.path()).unwrap();
        let ladder_after = serde_json::to_value(loom::maturity::ladder(&store).unwrap()).unwrap();
        let queues_after = serde_json::to_value(loom::maturity::depths(&store).unwrap()).unwrap();

        // Growing the statistical signal alone would not move these; after
        // promote, triage count may rise by the new untriaged Finding only.
        let triage_before = queues_before
            .get("triage")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let triage_after = queues_after
            .get("triage")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        assert!(
            triage_after >= triage_before,
            "asserted Finding may increase triage; before={triage_before} after={triage_after}"
        );
        // The statistical feed size is not a queue field — ensure we still
        // have the size_outlier in debt while queues only reflect findings.
        let debt_now = loom::signal::debt(&store).unwrap();
        assert!(debt_now.iter().any(|d| d.kind == "size_outlier"));

        // Maturity ladder may move only via ordinary finding-open gates, never
        // via the raw statistical row count. We only assert the debt feed is
        // still non-empty and that no statistical truth was stored.
        let _ = ladder_before;
        let _ = ladder_after;
    }

    // Edges/facets: only the Finding node was added for the promotion write.
    let store = Store::open(tmp.path()).unwrap();
    let findings = store
        .list_nodes(Some(NodeType::Finding), usize::MAX)
        .unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].id, finding_id);
    assert!(store.list_edges(None, usize::MAX).unwrap().is_empty());
}

#[test]
fn promoted_finding_survives_sync_derived_rebuild() {
    let (tmp, _) = fixture_size_outlier();

    // The fixture seeds loc facets directly. Materialize matching source files
    // before sync so this invokes the real extractor on the isolated graph.
    for (path, lines) in [
        ("a.rs", 40_usize),
        ("b.rs", 50),
        ("c.rs", 45),
        ("big.rs", 5_000),
    ] {
        tmp.write(path, &"// debt-promotion sync fixture\n".repeat(lines));
    }

    let (cluster_id, _) = live_size_outlier_cluster(tmp.path());
    let evidence =
        "the large module has a confirmed boundary concern and warrants a human triage decision";
    let promoted = loom_json(
        tmp.path(),
        &[
            "debt",
            "promote",
            &cluster_id,
            "--evidence",
            evidence,
            "--confidence",
            "0.91",
        ],
    );
    let finding_id = promoted["finding"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("promotion must return finding id: {promoted}"))
        .to_string();
    assert!(
        finding_id.starts_with('p'),
        "promotion must mint a p-prefixed finding: {finding_id}"
    );

    let (before, before_body, before_created_at, before_updated_at) = {
        let store = Store::open(tmp.path()).unwrap();
        let node = store
            .get_node(&finding_id)
            .unwrap()
            .unwrap_or_else(|| panic!("promoted finding {finding_id} must be stored"));
        assert_eq!(node.node_type, NodeType::Finding);
        assert_eq!(node.truth_class, TruthClass::Asserted);
        assert_eq!(
            node.body.get("source").and_then(|value| value.as_str()),
            Some("debt_promotion")
        );
        assert_eq!(
            node.body.get("evidence").and_then(|value| value.as_str()),
            Some(evidence)
        );
        assert_eq!(
            node.body.get("confidence").and_then(|value| value.as_f64()),
            Some(0.91)
        );
        assert_eq!(
            node.body
                .get("debt_cluster")
                .and_then(|value| value.get("id"))
                .and_then(|value| value.as_str()),
            Some(cluster_id.as_str()),
            "promotion must retain cluster provenance"
        );
        assert!(
            store
                .facets_of(&finding_id, TargetKind::Node)
                .unwrap()
                .is_empty(),
            "promotion creates no facets"
        );
        assert!(
            store
                .list_edges(None, usize::MAX)
                .unwrap()
                .iter()
                .all(|edge| edge.from_id != finding_id && edge.to_id != finding_id),
            "promotion creates no incident edges"
        );
        (
            node.clone(),
            node.body.clone(),
            node.created_at.clone(),
            node.updated_at.clone(),
        )
    };

    let sync = loom_json(tmp.path(), &["sync"]);
    assert_eq!(
        sync.get("files_scanned").and_then(|value| value.as_u64()),
        Some(4),
        "sync must extract the physical isolated fixture files: {sync}"
    );

    let store = Store::open(tmp.path()).unwrap();
    let after = store
        .get_node(&finding_id)
        .unwrap()
        .unwrap_or_else(|| panic!("sync must preserve promoted finding {finding_id}"));
    assert_eq!(
        after, before,
        "derived rebuild must not rewrite the asserted promotion"
    );
    assert_eq!(after.id, finding_id);
    assert_eq!(after.truth_class, TruthClass::Asserted);
    assert_eq!(after.body, before_body);
    assert_eq!(after.created_at, before_created_at);
    assert_eq!(after.updated_at, before_updated_at);
    assert_eq!(
        after.body.get("evidence").and_then(|value| value.as_str()),
        Some(evidence)
    );
    assert_eq!(
        after
            .body
            .get("confidence")
            .and_then(|value| value.as_f64()),
        Some(0.91)
    );
    assert_eq!(
        after
            .body
            .get("debt_cluster")
            .and_then(|value| value.get("id"))
            .and_then(|value| value.as_str()),
        Some(cluster_id.as_str()),
        "sync must retain promotion provenance"
    );
    assert!(
        store
            .facets_of(&finding_id, TargetKind::Node)
            .unwrap()
            .is_empty(),
        "sync must not attach derived facets to the asserted promotion"
    );

    let edges = store.list_edges(None, usize::MAX).unwrap();
    assert!(
        edges
            .iter()
            .all(|edge| edge.from_id != finding_id && edge.to_id != finding_id),
        "sync must not attach derived edges to the asserted promotion"
    );
    let statistical_finding_ids: Vec<_> = store
        .list_nodes(Some(NodeType::Finding), usize::MAX)
        .unwrap()
        .into_iter()
        .filter(|node| {
            node.id != finding_id
                && (node.status == "size_outlier"
                    || node.body.get("kind").and_then(|value| value.as_str())
                        == Some("size_outlier"))
        })
        .map(|node| node.id)
        .collect();
    assert!(
        statistical_finding_ids.is_empty(),
        "the statistical debt feed must not become stored truth: {statistical_finding_ids:?}"
    );
    assert!(
        edges.iter().all(|edge| {
            !statistical_finding_ids
                .iter()
                .any(|id| edge.from_id == *id || edge.to_id == *id)
        }),
        "the statistical debt feed must not create stored edges"
    );
}
