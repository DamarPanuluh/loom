//! Ring 2 invariant tests — structural plane: extraction, sync ripple, derived
//! rebuildability (INV-2).

use loom::model::{EdgeKind, InspectionStatus, NodeType, TruthClass};
use loom::store::Store;
use loom::travel::Export;
mod common;
use common::*;

fn export_json(store: &Store) -> String {
    Export::from_snapshot(store.snapshot().unwrap())
        .to_json()
        .unwrap()
}

// ---- INV-2 : derived plane is rebuildable byte-identically -----------------

#[test]
fn inv2_derived_plane_rebuildable() {
    let tmp = Tmp::new();
    // A Rust source file that triggers a deterministic finding (panic marker).
    tmp.write(
        "src/demo.rs",
        "pub fn f(x: Option<i32>) -> i32 { x.unwrap() }\n",
    );
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    codefile(&store, "src/demo.rs");

    loom::sync::run(&store, tmp.path()).unwrap();
    let before = export_json(&store);
    // there must be at least one derived finding
    let findings = store
        .list_nodes(Some(NodeType::Finding), usize::MAX)
        .unwrap();
    assert!(!findings.is_empty(), "expected a derived finding");
    assert!(findings
        .iter()
        .all(|f| f.truth_class == TruthClass::Derived));

    // Wipe the ENTIRE derived plane and re-sync → byte-identical.
    store.wipe_derived().unwrap();
    loom::sync::run(&store, tmp.path()).unwrap();
    let after = export_json(&store);
    assert_eq!(
        before, after,
        "derived plane must rebuild byte-identically (INV-2)"
    );
}

// ---- content-hash drives ripple; no false flags ----------------------------

#[test]
fn sync_ripples_only_on_real_change() {
    let tmp = Tmp::new();
    tmp.write("src/payment.rs", "pub fn capture() {}\n");
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "payment can be captured",
            "",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = codefile(&store, "src/payment.rs");
    let edge = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &cf.id,
            TruthClass::Asserted,
        )
        .unwrap();
    // first sync establishes the content hash (no prior → no ripple)
    loom::sync::run(&store, tmp.path()).unwrap();
    // ground the implements edge passing
    store
        .record_verdict(
            &edge.id,
            InspectionStatus::Passing,
            "c",
            "src/payment.rs:1",
            0.9,
            "llm",
        )
        .unwrap();

    // sync with identical content → no ripple, stays passing
    loom::sync::run(&store, tmp.path()).unwrap();
    assert_eq!(
        store
            .get_edge(&edge.id)
            .expect("get_edge ok")
            .expect("edge exists")
            .status,
        InspectionStatus::Passing,
        "unchanged content must not stale the grounding"
    );

    // rewrite identical bytes (mtime churn, same hash) → still no ripple
    tmp.write("src/payment.rs", "pub fn capture() {}\n");
    loom::sync::run(&store, tmp.path()).unwrap();
    assert_eq!(
        store
            .get_edge(&edge.id)
            .expect("get_edge ok")
            .expect("edge exists")
            .status,
        InspectionStatus::Passing,
        "identical content (mtime churn) must not stale"
    );

    // real content change → ripple to needs_reverification
    tmp.write("src/payment.rs", "pub fn capture() { /* changed */ }\n");
    let report = loom::sync::run(&store, tmp.path()).unwrap();
    assert_eq!(report.files_changed, 1);
    assert_eq!(
        store.get_edge(&edge.id).unwrap().unwrap().status,
        InspectionStatus::NeedsReverification,
        "real change must stale the grounding"
    );
}

// ---- ripple reaches governs verdicts through the intent --------------------

#[test]
fn sync_ripples_governs_through_intent() {
    let tmp = Tmp::new();
    tmp.write("src/auth.rs", "pub fn login() {}\n");
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "user can log in",
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = codefile(&store, "src/auth.rs");
    let rule = store
        .add_node(
            NodeType::QualityRule,
            "service-auth-at-boundary",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &cf.id,
            TruthClass::Asserted,
        )
        .unwrap();
    let gov = store
        .add_edge(
            EdgeKind::Governs,
            &rule.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    loom::sync::run(&store, tmp.path()).unwrap();
    store
        .record_verdict(
            &gov.id,
            InspectionStatus::Passing,
            "auth checked",
            "src/auth.rs:1",
            0.9,
            "llm",
        )
        .unwrap();

    tmp.write("src/auth.rs", "pub fn login() { /* refactor */ }\n");
    loom::sync::run(&store, tmp.path()).unwrap();
    assert_eq!(
        store.get_edge(&gov.id).unwrap().unwrap().status,
        InspectionStatus::NeedsReverification,
        "governs verdict must stale when the grounding file changes"
    );
}

// ---- derived edges never enter the asserted residue queue ------------------

#[test]
fn derived_findings_are_not_asserted_work() {
    let tmp = Tmp::new();
    tmp.write(
        "src/x.rs",
        "pub fn g(o: Option<i32>) -> i32 { o.unwrap() }\n",
    );
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    codefile(&store, "src/x.rs");
    loom::sync::run(&store, tmp.path()).unwrap();

    let all_edges = store.list_edges(None, usize::MAX).unwrap();
    let derived: Vec<_> = all_edges
        .iter()
        .filter(|e| e.truth_class == TruthClass::Derived)
        .collect();
    assert!(!derived.is_empty(), "expected derived flags/assesses edges");
    // INV-5 corollary: every derived edge rests at `current`, never an
    // asserted verdict status that would put it in the `loom next` residue.
    for e in &derived {
        assert_eq!(
            e.status,
            InspectionStatus::Current,
            "derived edge {} must rest at 'current', not an asserted verdict",
            e.kind
        );
    }
}

// ---- glob expansion registers matching files -------------------------------

#[test]
fn glob_expansion_finds_rust_files() {
    let tmp = Tmp::new();
    tmp.write("src/a.rs", "fn a() {}\n");
    tmp.write("src/inner/b.rs", "fn b() {}\n");
    tmp.write("src/notes.txt", "ignore me\n");
    let matched = loom::fsglob::expand(tmp.path(), "src/**/*.rs").unwrap();
    assert!(matched.contains(&"src/a.rs".to_string()));
    assert!(matched.contains(&"src/inner/b.rs".to_string()));
    assert!(!matched.iter().any(|m| m.ends_with(".txt")));
}

// ---- integration monitoring: an upstream change resets the contracts -------
// that exercise the surfaces backed by the changed file (surface-plane ripple).

#[test]
fn sync_ripples_upstream_change_to_integration_contract() {
    let tmp = Tmp::new();
    // A vendored upstream file we consume but do not own.
    tmp.write("vendor/up/lib.rs", "pub fn auth() -> bool { true }\n");
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();

    // My consuming intent, the upstream file, and the integration surface.
    let intent = store
        .add_node(
            NodeType::Intent,
            "consume upstream auth",
            "",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = codefile(&store, "vendor/up/lib.rs");
    let surface = store
        .add_node(
            NodeType::InterfaceSurface,
            "UpstreamAuth",
            "",
            "",
            serde_json::json!({ "kind": "sdk_method" }),
        )
        .unwrap();
    // surface exposes the upstream codefile.
    store
        .add_edge(EdgeKind::Exposes, &surface.id, &cf.id, TruthClass::Asserted)
        .unwrap();

    // A contract validation that validates my intent and calls the surface.
    let val = store
        .add_node(
            NodeType::Validation,
            "auth-contract",
            "",
            "not_run",
            serde_json::json!({ "type": "contract" }),
        )
        .unwrap();
    let validates = store
        .add_edge(
            EdgeKind::Validates,
            &val.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .add_edge(EdgeKind::Calls, &val.id, &surface.id, TruthClass::Asserted)
        .unwrap();

    // Baseline sync (establishes the upstream hash; no prior → no ripple).
    let base = loom::sync::run(&store, tmp.path()).unwrap();
    assert_eq!(base.contracts_reset, 0, "baseline must not ripple");
    assert_eq!(base.surfaces_affected, 0);

    // We verified the contract once: it passes and its proof grounds the intent.
    store.set_node_status(&val.id, "passed").unwrap();
    store
        .record_verdict(
            &validates.id,
            InspectionStatus::Passing,
            "contract holds",
            "probed upstream — see vendor/up/lib.rs:1",
            0.9,
            "llm",
        )
        .unwrap();

    // Identical content again → still no ripple, contract stays passed.
    let noop = loom::sync::run(&store, tmp.path()).unwrap();
    assert_eq!(noop.contracts_reset, 0, "unchanged upstream must not reset");
    assert_eq!(store.get_node(&val.id).unwrap().unwrap().status, "passed");

    // Upstream changes → the surface's backing changed → the contract resets.
    tmp.write(
        "vendor/up/lib.rs",
        "pub fn auth() -> Result<bool, String> { Ok(true) }\n",
    );
    let report = loom::sync::run(&store, tmp.path()).unwrap();
    assert_eq!(report.files_changed, 1);
    assert_eq!(
        report.surfaces_affected, 1,
        "the touched surface is counted"
    );
    assert_eq!(report.contracts_reset, 1, "the calling contract is reset");
    assert_eq!(
        store.get_node(&val.id).unwrap().unwrap().status,
        "not_run",
        "an upstream change must reset the contract to not_run"
    );
    assert_eq!(
        store.get_edge(&validates.id).unwrap().unwrap().status,
        InspectionStatus::NeedsReverification,
        "the intent's proof must read as unproven after the upstream change"
    );
}

// A JourneyProof validation's `body.artifact` (a contract JSON / journey spec file) is
// not necessarily a registered CodeFile, so the structural pass cannot see it.
// Sync must track it directly: when the artifact changes, the validation is
// reset and its Validates edge stales — a stale proof cannot keep an intent
// "proven" and silence the journey smell.
#[test]
fn sync_stales_journey_proof_when_artifact_drifts() {
    let tmp = Tmp::new();
    // A contract artifact that is NOT registered as a CodeFile node.
    tmp.write("contracts/checkout.v1.json", r#"{"routes":[]}"#);
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();

    let intent = store
        .add_node(
            NodeType::Intent,
            "checkout completes",
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let val = store
        .add_node(
            NodeType::Validation,
            "checkout journey",
            "",
            "not_run",
            serde_json::json!({
                "type": "journey",
                "proof_kind": "journey",
                "proof_level": "L5",
                "artifact": "contracts/checkout.v1.json",
            }),
        )
        .unwrap();
    let validates = store
        .add_edge(
            EdgeKind::Validates,
            &val.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();

    // Baseline sync establishes the artifact hash; no prior → no ripple.
    let base = loom::sync::run(&store, tmp.path()).unwrap();
    assert_eq!(base.validations_reset, 0, "baseline must not ripple");

    // The journey proof passes.
    store.set_node_status(&val.id, "passed").unwrap();
    store
        .record_verdict(
            &validates.id,
            InspectionStatus::Passing,
            "journey passes end-to-end",
            "journey run passed — see contracts/checkout.v1.json:1",
            0.9,
            "llm",
        )
        .unwrap();

    // Identical artifact again → no ripple, proof stays passed.
    let noop = loom::sync::run(&store, tmp.path()).unwrap();
    assert_eq!(
        noop.validations_reset, 0,
        "unchanged artifact must not reset"
    );
    assert_eq!(store.get_node(&val.id).unwrap().unwrap().status, "passed");

    // Artifact drifts → proof resets and Validates edge stales.
    tmp.write(
        "contracts/checkout.v1.json",
        r#"{"routes":[{"path":"/x"}]}"#,
    );
    let report = loom::sync::run(&store, tmp.path()).unwrap();
    assert_eq!(
        report.validations_reset, 1,
        "a drifted journey artifact must reset its validation"
    );
    assert_eq!(
        store.get_node(&val.id).unwrap().unwrap().status,
        "not_run",
        "a drifted artifact must reset the proof to not_run"
    );
    assert_eq!(
        store.get_edge(&validates.id).unwrap().unwrap().status,
        InspectionStatus::NeedsReverification,
        "the Validates edge must read as unproven after artifact drift"
    );
}

// An artifact that disappears is the same as one that drifts: a proof against a
// vanished artifact is no longer proven.
#[test]
fn sync_stales_journey_proof_when_artifact_disappears() {
    let tmp = Tmp::new();
    tmp.write("contracts/checkout.v1.json", r#"{"routes":[]}"#);
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "checkout completes",
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let val = store
        .add_node(
            NodeType::Validation,
            "checkout journey",
            "",
            "not_run",
            serde_json::json!({
                "type": "journey",
                "proof_kind": "journey",
                "proof_level": "L5",
                "artifact": "contracts/checkout.v1.json",
            }),
        )
        .unwrap();
    let validates = store
        .add_edge(
            EdgeKind::Validates,
            &val.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    // Baseline + pass.
    loom::sync::run(&store, tmp.path()).unwrap();
    store.set_node_status(&val.id, "passed").unwrap();
    store
        .record_verdict(
            &validates.id,
            InspectionStatus::Passing,
            "journey passes",
            "journey run passed — see contracts/checkout.v1.json:1",
            0.9,
            "llm",
        )
        .unwrap();
    // Artifact deleted → proof resets and edge stales.
    std::fs::remove_file(tmp.path().join("contracts/checkout.v1.json")).unwrap();
    let report = loom::sync::run(&store, tmp.path()).unwrap();
    assert_eq!(
        report.validations_reset, 1,
        "vanished artifact resets proof"
    );
    assert_eq!(store.get_node(&val.id).unwrap().unwrap().status, "not_run");
}

// INV-2 convergence: a wipe of derived facets then sync must not re-ripple a
// drifted-and-reset artifact (no prior hash → no second reset).
#[test]
fn sync_artifact_drift_is_deterministic_on_rebuild() {
    let tmp = Tmp::new();
    tmp.write("contracts/c.json", r#"{"v":1}"#);
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "checkout completes",
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let val = store
        .add_node(
            NodeType::Validation,
            "checkout journey",
            "",
            "not_run",
            serde_json::json!({
                "type": "journey",
                "proof_kind": "journey",
                "proof_level": "L5",
                "artifact": "contracts/c.json",
            }),
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Validates,
            &val.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    loom::sync::run(&store, tmp.path()).unwrap();
    store.set_node_status(&val.id, "passed").unwrap();
    // Drift + reset.
    tmp.write("contracts/c.json", r#"{"v":2}"#);
    let r1 = loom::sync::run(&store, tmp.path()).unwrap();
    assert_eq!(r1.validations_reset, 1);
    // Wipe derived + rebuild must NOT re-ripple (prior hash gone).
    store.wipe_derived().unwrap();
    let r2 = loom::sync::run(&store, tmp.path()).unwrap();
    assert_eq!(
        r2.validations_reset, 0,
        "a wipe+rebuild must converge: no prior artifact_hash → no re-ripple"
    );
}

// A never-verified contract on a changed surface is not "now unproven" — it was
// never proven. It must not be counted, and the headline tally must match the
// integration line (no `0 validations reset` next to a reset contract).
#[test]
fn sync_does_not_count_never_verified_contracts() {
    let tmp = Tmp::new();
    tmp.write("vendor/up/lib.rs", "pub fn f() -> bool { true }\n");
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "consume f",
            "",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = codefile(&store, "vendor/up/lib.rs");
    let surface = store
        .add_node(
            NodeType::InterfaceSurface,
            "FSurface",
            "",
            "",
            serde_json::json!({ "kind": "sdk_method" }),
        )
        .unwrap();
    store
        .add_edge(EdgeKind::Exposes, &surface.id, &cf.id, TruthClass::Asserted)
        .unwrap();

    // Two contracts on the same surface: one verified, one never verified.
    let proven = store
        .add_node(
            NodeType::Validation,
            "proven-contract",
            "",
            "not_run",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Validates,
            &proven.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Calls,
            &proven.id,
            &surface.id,
            TruthClass::Asserted,
        )
        .unwrap();
    let never = store
        .add_node(
            NodeType::Validation,
            "never-verified",
            "",
            "not_run",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Validates,
            &never.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Calls,
            &never.id,
            &surface.id,
            TruthClass::Asserted,
        )
        .unwrap();

    loom::sync::run(&store, tmp.path()).unwrap(); // baseline
    store.set_node_status(&proven.id, "passed").unwrap();

    tmp.write("vendor/up/lib.rs", "pub fn f() -> u8 { 1 }\n");
    let report = loom::sync::run(&store, tmp.path()).unwrap();
    assert_eq!(
        report.contracts_reset, 1,
        "only the proven contract is counted"
    );
    assert_eq!(
        report.validations_reset, report.contracts_reset,
        "headline tally must equal the integration line (no contradiction)"
    );
    assert_eq!(
        store.get_node(&proven.id).unwrap().unwrap().status,
        "not_run",
        "the proven contract resets"
    );
    assert_eq!(
        store.get_node(&never.id).unwrap().unwrap().status,
        "not_run",
        "the never-verified contract stays not_run and is uncounted"
    );
}

// INV-2 for the integration path: a no-change sync and a wipe+rebuild must NOT
// fire the surface ripple — a proven contract stays proven across both.
#[test]
fn integration_ripple_is_deterministic_on_rebuild() {
    let tmp = Tmp::new();
    tmp.write("vendor/up/lib.rs", "pub fn f() -> bool { true }\n");
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "consume f",
            "",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = codefile(&store, "vendor/up/lib.rs");
    let surface = store
        .add_node(
            NodeType::InterfaceSurface,
            "FSurface",
            "",
            "",
            serde_json::json!({ "kind": "sdk_method" }),
        )
        .unwrap();
    store
        .add_edge(EdgeKind::Exposes, &surface.id, &cf.id, TruthClass::Asserted)
        .unwrap();
    let val = store
        .add_node(
            NodeType::Validation,
            "contract",
            "",
            "not_run",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Validates,
            &val.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .add_edge(EdgeKind::Calls, &val.id, &surface.id, TruthClass::Asserted)
        .unwrap();

    loom::sync::run(&store, tmp.path()).unwrap(); // baseline establishes the hash
    store.set_node_status(&val.id, "passed").unwrap();

    // No-change sync → identical content → no ripple, contract stays proven.
    let r1 = loom::sync::run(&store, tmp.path()).unwrap();
    assert_eq!(r1.contracts_reset, 0);
    assert_eq!(
        store.get_node(&val.id).unwrap().unwrap().status,
        "passed",
        "a no-change sync must not reset the contract"
    );

    // Wiping the derived plane drops the content_hash facet; the rebuild has no
    // prior hash, so it must NOT fire the ripple (rebuild, not a change).
    store.wipe_derived().unwrap();
    let r2 = loom::sync::run(&store, tmp.path()).unwrap();
    assert_eq!(
        r2.contracts_reset, 0,
        "rebuild must not fire the integration ripple"
    );
    assert_eq!(
        store.get_node(&val.id).unwrap().unwrap().status,
        "passed",
        "wipe + rebuild must leave the contract proven (INV-2)"
    );
}

// A disappearing upstream file is a change too: its dependent contracts reset.
// And it must ripple exactly ONCE — a still-missing file on later syncs must not
// keep resetting a re-verified contract (the content_hash is cleared to ensure
// the incremental path converges with a clean rebuild).
#[test]
fn sync_ripples_upstream_file_deletion_once() {
    let tmp = Tmp::new();
    tmp.write("vendor/up/lib.rs", "pub fn f() -> bool { true }\n");
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "consume f",
            "",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = codefile(&store, "vendor/up/lib.rs");
    let surface = store
        .add_node(
            NodeType::InterfaceSurface,
            "FSurface",
            "",
            "",
            serde_json::json!({ "kind": "sdk_method" }),
        )
        .unwrap();
    store
        .add_edge(EdgeKind::Exposes, &surface.id, &cf.id, TruthClass::Asserted)
        .unwrap();
    let val = store
        .add_node(
            NodeType::Validation,
            "contract",
            "",
            "not_run",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Validates,
            &val.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .add_edge(EdgeKind::Calls, &val.id, &surface.id, TruthClass::Asserted)
        .unwrap();

    loom::sync::run(&store, tmp.path()).unwrap(); // baseline
    store.set_node_status(&val.id, "passed").unwrap();

    // The upstream file vanishes → the contract that depends on it resets.
    std::fs::remove_file(tmp.path().join("vendor/up/lib.rs")).unwrap();
    let report = loom::sync::run(&store, tmp.path()).unwrap();
    assert_eq!(report.files_deleted, 1, "the disappearance is counted");
    assert_eq!(report.contracts_reset, 1, "the dependent contract resets");
    assert_eq!(store.get_node(&val.id).unwrap().unwrap().status, "not_run");

    // Pretend we re-verified against the now-absent upstream; a later sync with
    // the file STILL missing must not reset it again (ripple-once).
    store.set_node_status(&val.id, "passed").unwrap();
    let again = loom::sync::run(&store, tmp.path()).unwrap();
    assert_eq!(
        again.files_deleted, 0,
        "an already-gone file is not re-counted"
    );
    assert_eq!(
        again.contracts_reset, 0,
        "deletion ripples once, not every sync"
    );
    assert_eq!(
        store.get_node(&val.id).unwrap().unwrap().status,
        "passed",
        "a still-missing file must not keep resetting a re-verified contract"
    );
}
