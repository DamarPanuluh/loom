//! Ring 6 tests — smells (structural), debt (statistical, never stored),
//! doctor (integrity), and a live journey run against a mock HTTP server.

use loom::identity::Agent;
use loom::model::{EdgeKind, InspectionStatus, NodeType, TargetKind, TruthClass};
use loom::store::Store as LoomStore;
use std::ops::{Deref, DerefMut};
use std::path::Path;

mod common;
use common::*;

/// Ring 6 builds fixture graphs in-process. Keep those fixtures hermetic when
/// the suite itself is launched by a lane-scoped proof runner: the runner's
/// authority must not become the authority used to construct the fixture.
struct Store(LoomStore);

impl Store {
    fn init(root: &Path, name: Option<&str>, observed: bool) -> loom::Result<Self> {
        let store = LoomStore::init(root, name, observed)?;
        store.set_agent(Agent::Solo);
        Ok(Self(store))
    }

    fn open(root: &Path) -> loom::Result<Self> {
        let store = LoomStore::open(root)?;
        store.set_agent(Agent::Solo);
        Ok(Self(store))
    }

    fn derived_node_id(node_type: NodeType, det_key: &str) -> String {
        LoomStore::derived_node_id(node_type, det_key)
    }
}

impl Deref for Store {
    type Target = LoomStore;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Store {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Spawn the compiled CLI as an independent solo fixture operator. Executor
/// attribution belongs to the outer proof runner and must not leak into the
/// graph mutations performed by these subprocess fixtures.
fn loom_command() -> std::process::Command {
    let mut command =
        std::process::Command::new(std::path::PathBuf::from(env!("CARGO_BIN_EXE_loom")));
    command.env(loom::identity::AGENT_ENV, "solo");
    command.env_remove(loom::identity::PROFILE_ENV);
    command
}

fn run_cli(tmp: &Path, args: &[&str]) {
    let mut cmd = loom_command();
    cmd.arg("--graph").arg(tmp).args(args);
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("spawn loom {args:?}: {e}"));
    assert!(
        out.status.success(),
        "loom {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn run_cli_json(tmp: &Path, args: &[&str]) -> serde_json::Value {
    let mut cmd = loom_command();
    cmd.arg("--graph").arg(tmp).args(args).arg("--json");
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("spawn loom {args:?}: {e}"));
    assert!(
        out.status.success(),
        "loom {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "loom {args:?} emitted invalid JSON: {e}\n{}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn run_cli_raw(tmp: &Path, args: &[&str]) -> (std::process::ExitStatus, String, String) {
    let mut cmd = loom_command();
    cmd.arg("--graph").arg(tmp).args(args);
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("spawn loom {args:?}: {e}"));
    (
        out.status,
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn intent(store: &Store, name: &str, lifecycle: &str) -> String {
    store
        .add_node(NodeType::Intent, name, "", lifecycle, serde_json::json!({}))
        .unwrap()
        .id
}
/// This suite's helper returns the id; `common::codefile` backs the path with a
/// real file so a citation into it can anchor.
fn codefile(store: &Store, path: &str) -> String {
    common::codefile(store, path).id
}

// ---- smells ----------------------------------------------------------------

#[test]
fn smells_detect_undeclared_shared_ownership() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    // ≥3 disconnected owners → tangled_file
    let cf = codefile(&store, "src/god.rs");
    for i in 0..3 {
        let id = intent(&store, &format!("behavior {i}"), "implemented");
        store
            .add_edge(EdgeKind::Implements, &id, &cf, TruthClass::Asserted)
            .unwrap();
    }
    let smells = loom::signal::smells(&store).unwrap();
    assert!(
        smells
            .iter()
            .any(|s| s.kind == "tangled_file" && s.message.contains("src/god.rs")),
        "disconnected multi-owners must fire tangled_file: {smells:?}"
    );

    // Exactly 2 disconnected owners → same smell (former overlapping_ownership).
    let cf2 = codefile(&store, "src/pair.rs");
    let a = intent(&store, "alpha behavior", "implemented");
    let b = intent(&store, "beta behavior", "implemented");
    store
        .add_edge(EdgeKind::Implements, &a, &cf2, TruthClass::Asserted)
        .unwrap();
    store
        .add_edge(EdgeKind::Implements, &b, &cf2, TruthClass::Asserted)
        .unwrap();
    let smells = loom::signal::smells(&store).unwrap();
    assert!(
        smells
            .iter()
            .any(|s| s.kind == "tangled_file" && s.message.contains("src/pair.rs")),
        "disconnected pair must fire tangled_file: {smells:?}"
    );
    assert!(
        !smells.iter().any(|s| s.kind == "overlapping_ownership"),
        "overlapping_ownership is retired"
    );
}

#[test]
fn tangled_file_spared_when_co_owners_are_related() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let cf = codefile(&store, "src/shared.rs");
    let a = intent(&store, "alpha behavior", "implemented");
    let b = intent(&store, "beta behavior", "implemented");
    let c = intent(&store, "gamma behavior", "implemented");
    for id in [&a, &b, &c] {
        store
            .add_edge(EdgeKind::Implements, id, &cf, TruthClass::Asserted)
            .unwrap();
    }
    // Star (parent ↔ children), not a clique — still one connected neighborhood.
    store
        .add_edge(EdgeKind::ScenarioOf, &b, &a, TruthClass::Asserted)
        .unwrap();
    store
        .add_edge(EdgeKind::ScenarioOf, &c, &a, TruthClass::Asserted)
        .unwrap();
    let smells = loom::signal::smells(&store).unwrap();
    assert!(
        !smells
            .iter()
            .any(|s| s.kind == "tangled_file" && s.message.contains("src/shared.rs")),
        "connected co-owners must not fire tangled_file: {smells:?}"
    );
}

#[test]
fn tangled_file_ignores_owner_count_when_connected() {
    // Contract: connectedness is the gate — many related owners stay silent;
    // there is no max_file_owners count threshold.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let cf = codefile(&store, "src/mega.rs");
    let parent = intent(&store, "parent capability", "implemented");
    store
        .add_edge(EdgeKind::Implements, &parent, &cf, TruthClass::Asserted)
        .unwrap();
    for i in 0..4 {
        let id = intent(&store, &format!("scenario {i}"), "implemented");
        store
            .add_edge(EdgeKind::Implements, &id, &cf, TruthClass::Asserted)
            .unwrap();
        store
            .add_edge(EdgeKind::ScenarioOf, &id, &parent, TruthClass::Asserted)
            .unwrap();
    }
    let smells = loom::signal::smells(&store).unwrap();
    assert!(
        !smells
            .iter()
            .any(|s| s.kind == "tangled_file" && s.message.contains("src/mega.rs")),
        "five connected owners must stay silent: {smells:?}"
    );
}

#[test]
fn sync_materializes_smell_finding_adjudication_and_convergence() {
    let tmp = Tmp::new();
    tmp.write("src/pair.rs", "pub fn pair() {}\n");
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let cf = codefile(&store, "src/pair.rs");
    let a = intent(&store, "alpha behavior", "implemented");
    let b = intent(&store, "beta behavior", "implemented");
    store
        .add_edge(EdgeKind::Implements, &a, &cf, TruthClass::Asserted)
        .unwrap();
    store
        .add_edge(EdgeKind::Implements, &b, &cf, TruthClass::Asserted)
        .unwrap();

    let smell = loom::signal::smells(&store)
        .unwrap()
        .into_iter()
        .find(|s| s.kind == "tangled_file")
        .unwrap();
    let expected_id = Store::derived_node_id(
        NodeType::Finding,
        &loom::signal::smell_det_key(&smell.identity),
    );
    assert!(store.get_node(&expected_id).unwrap().is_none());

    loom::sync::run(&store, tmp.path()).unwrap();
    let finding = store.get_node(&expected_id).unwrap().unwrap();
    assert_eq!(finding.node_type, NodeType::Finding);
    assert_eq!(finding.status, smell.kind);
    assert_eq!(finding.name, smell.message);
    assert_eq!(finding.description, smell.remedy);
    assert_eq!(
        finding.body.get("category").and_then(|v| v.as_str()),
        Some("smell")
    );
    assert_eq!(
        finding.body.get("identity").and_then(|v| v.as_str()),
        Some(smell.identity.as_str())
    );

    store
        .record_finding_verdict(&expected_id, "justified", "shared transition seam", "")
        .unwrap();
    loom::sync::run(&store, tmp.path()).unwrap();
    assert!(store.get_node(&expected_id).unwrap().is_some());
    assert_eq!(
        loom::signal::adjudication_of(&store, &expected_id).unwrap(),
        Some(("justified".into(), "shared transition seam".into()))
    );

    store
        .add_edge(EdgeKind::Relates, &a, &b, TruthClass::Asserted)
        .unwrap();
    loom::sync::run(&store, tmp.path()).unwrap();
    assert!(store.get_node(&expected_id).unwrap().is_none());
}

#[test]
fn smells_duplicated_responsibility_via_tags() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    store.add_vocab_term("retry", "retry policy").unwrap();
    let a = intent(&store, "retry on http failure", "implemented");
    let b = intent(&store, "retry on queue failure", "implemented");
    let fa = codefile(&store, "src/http.rs");
    let fb = codefile(&store, "src/queue.rs");
    store
        .add_edge(EdgeKind::Implements, &a, &fa, TruthClass::Asserted)
        .unwrap();
    store
        .add_edge(EdgeKind::Implements, &b, &fb, TruthClass::Asserted)
        .unwrap();
    store.set_tag(&a, TargetKind::Node, "retry").unwrap();
    store.set_tag(&b, TargetKind::Node, "retry").unwrap();
    let smells = loom::signal::smells(&store).unwrap();
    assert!(smells.iter().any(|s| s.kind == "duplicated_responsibility"));
}

#[test]
fn shared_proof_command_smell_reports_collisions_only_and_recommends_narrowing() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("shared proof diagnostic"), false).unwrap();
    let first = intent(&store, "first shared-proof behavior", "implemented");
    let second = intent(&store, "second shared-proof behavior", "implemented");
    let unique = intent(&store, "unique-proof behavior", "implemented");
    let shared_command = "cargo test --test ring6 shared-proof-sentinel";
    let unique_command = "cargo test --test ring6 unique-proof-sentinel";
    let add_validation = |name: &str, command: &str, intent_id: &str| {
        let validation = store
            .add_node(
                NodeType::Validation,
                name,
                "",
                "not_run",
                serde_json::json!({"type":"test", "command":command}),
            )
            .unwrap();
        store
            .add_edge(
                EdgeKind::Validates,
                &validation.id,
                intent_id,
                TruthClass::Asserted,
            )
            .unwrap();
    };
    add_validation("first shared proof", shared_command, &first);
    add_validation("second shared proof", shared_command, &second);
    add_validation("unique proof", unique_command, &unique);

    let smells = loom::signal::smells(&store).unwrap();
    let shared = smells
        .iter()
        .filter(|smell| smell.kind == "shared_proof_command")
        .collect::<Vec<_>>();
    assert_eq!(
        shared.len(),
        1,
        "only the command shared by distinct behaviors is diagnosed: {smells:#?}"
    );
    let diagnostic = shared[0];
    assert!(
        diagnostic.message.contains("2 behaviors") && diagnostic.message.contains(shared_command),
        "diagnostic identifies the shared mechanism and affected behavior count: {diagnostic:#?}"
    );
    assert!(
        diagnostic.remedy.contains("narrow each proof"),
        "diagnostic recommends narrowing the shared proof: {diagnostic:#?}"
    );
    assert!(
        !diagnostic.message.contains(unique_command)
            && !diagnostic.identity.contains(unique_command),
        "a mechanism used by only one behavior is excluded: {diagnostic:#?}"
    );
}

// ---- journey proof smells --------------------------------------------------

/// helper: an implemented intent marked user_visible.
fn visible_intent(store: &Store, name: &str) -> String {
    let id = intent(store, name, "implemented");
    store
        .set_facet(
            &id,
            TargetKind::Node,
            "visibility",
            "user_visible",
            TruthClass::Asserted,
        )
        .unwrap();
    id
}

#[test]
fn journey_proof_smell_fires_when_user_visible_intent_has_no_validation() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let _ = visible_intent(&store, "checkout completes");
    let smells = loom::signal::smells(&store).unwrap();
    let smell = smells
        .iter()
        .find(|s| s.kind == "missing_journey_proof" && s.message.contains("checkout completes"))
        .expect("missing journey proof smell");
    assert!(
        smell.message.contains("S3-or-stronger")
            && !smell.message.contains("L5")
            && !smell.message.contains("L6"),
        "smell must use the derived strength scale, not retired L5/L6: {}",
        smell.message
    );
}

#[test]
fn journey_proof_smell_fires_when_validation_is_too_shallow() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent_id = visible_intent(&store, "checkout completes");
    // a non-journey, non-L5 validation linked via Validates
    let validation = store
        .add_node(
            NodeType::Validation,
            "unit checkout",
            "",
            "passed",
            serde_json::json!({"type":"test","command":"true"}),
        )
        .unwrap();
    // Earned, not asserted: loom runs the proof and records what it saw.
    store
        .ensure_edge(EdgeKind::Validates, &validation.id, &intent_id)
        .unwrap();
    {
        let mut body = validation.body.clone();
        body["command"] = serde_json::json!("true");
        body["type"] = serde_json::json!("test");
        store.set_node_body(&validation.id, &body).unwrap();
        let fresh = store.get_node(&validation.id).unwrap().unwrap();
        loom::commands::observe_validation(&store, &fresh).unwrap();
    }
    let smells = loom::signal::smells(&store).unwrap();
    let smell = smells
        .iter()
        .find(|s| {
            s.kind == "proof_too_shallow_for_intent" && s.message.contains("checkout completes")
        })
        .expect("shallow proof smell");
    assert!(
        smell.message.contains("S3-or-stronger")
            && !smell.message.contains("L5")
            && !smell.message.contains("L6"),
        "smell must use the derived strength scale, not retired L5/L6: {}",
        smell.message
    );
}

#[test]
fn journey_proof_smell_silent_when_passing_compiled_journey_exists() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent_id = visible_intent(&store, "checkout completes");
    // Earned, not asserted: the grade is derived from the proof's shape, so the
    // fixture has to build a proof that actually has that shape.
    s3_journey_proof(&store, tmp.path(), &intent_id, "checkout journey");
    let smells = loom::signal::smells(&store).unwrap();
    assert!(
        !smells
            .iter()
            .any(|s| s.kind == "missing_journey_proof" || s.kind == "proof_too_shallow_for_intent"),
        "no journey proof smell should fire: {smells:?}"
    );
}

// Drift gate ties sync staleness to the smell: a passing compiled Journey proof
// silences the smell, but once its artifact drifts and sync resets the proof,
// the smell MUST re-fire — a stale artifact cannot keep an intent "proven".
#[test]
fn journey_proof_smell_re_fires_after_artifact_drift_resets_proof() {
    let tmp = Tmp::new();
    tmp.write("contracts/checkout.v1.json", r#"{"routes":[]}"#);
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent_id = visible_intent(&store, "checkout completes");
    // A proof that EARNS its grade — every conjunct real — so what the drift
    // below takes away is a genuinely strong proof, not a declared one.
    let validation = s3_journey_proof(&store, tmp.path(), &intent_id, "checkout journey");
    loom::sync::run(&store, tmp.path()).unwrap();
    let silent = loom::signal::smells(&store).unwrap();
    assert!(
        !silent
            .iter()
            .any(|s| s.kind == "missing_journey_proof" || s.kind == "proof_too_shallow_for_intent"),
        "passing compiled Journey proof should silence the smell: {silent:?}"
    );

    // A semantic change is re-registered. That invalidates the accepted
    // Derives/Surfaces projections, and sync resets the compiled proof.
    let journey = store
        .resolve_node("checkout-journey", Some(NodeType::Journey))
        .unwrap();
    let artifact = journey.body["artifact"].as_str().unwrap().to_string();
    let mut spec = loom::journey::parse(&tmp.path().join(&artifact)).unwrap();
    spec.steps[0].action = "checks out under a changed semantic contract".into();
    std::fs::write(
        tmp.path().join(&artifact),
        serde_norway::to_string(&spec).unwrap(),
    )
    .unwrap();
    drop(store);
    let output = loom_command()
        .arg("--graph")
        .arg(tmp.path())
        .args(["journey", "add"])
        .arg(tmp.path().join(&artifact))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let store = Store::open(tmp.path()).unwrap();
    loom::sync::run(&store, tmp.path()).unwrap();
    assert_eq!(
        store.get_node(&validation.id).unwrap().unwrap().status,
        "not_run"
    );
    let smells = loom::signal::smells(&store).unwrap();
    assert!(
        smells
            .iter()
            .any(|s| s.kind == "proof_too_shallow_for_intent"
                && s.message.contains("checkout completes")),
        "a drifted artifact must re-fire the journey proof smell: {smells:?}"
    );
}
// ---- debt: statistical, never stored (INV-3) -------------------------------

#[test]
fn debt_size_outlier_is_not_stored() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    // loc facets: three small, one huge → outlier
    for (p, loc) in [("a.rs", 40), ("b.rs", 50), ("c.rs", 45), ("big.rs", 5000)] {
        let id = codefile(&store, p);
        store
            .set_facet(
                &id,
                TargetKind::Node,
                "loc",
                &loc.to_string(),
                TruthClass::Derived,
            )
            .unwrap();
    }
    let edges_before = store.list_edges(None, usize::MAX).unwrap().len();
    let debt = loom::signal::debt(&store).unwrap();
    assert!(debt
        .iter()
        .any(|d| d.kind == "size_outlier" && d.message.contains("big.rs")));
    // INV-3: computing debt stores no edges
    let edges_after = store.list_edges(None, usize::MAX).unwrap().len();
    assert_eq!(edges_before, edges_after, "debt must never store edges");
}

/// INV-3, remaining clauses: a statistical signal is never a *gate input*
/// (maturity ladder) and never a *required item* (`loom next` / queue counts).
/// The debt feed appearing or growing must leave routing and maturity
/// byte-identical.
#[test]
fn inv3_debt_never_gates_or_queues() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    for (p, loc) in [("a.rs", 40), ("b.rs", 50), ("c.rs", 45)] {
        let id = codefile(&store, p);
        store
            .set_facet(
                &id,
                TargetKind::Node,
                "loc",
                &loc.to_string(),
                TruthClass::Derived,
            )
            .unwrap();
    }
    assert!(loom::signal::debt(&store).unwrap().is_empty());

    // Introduce a statistical outlier → the debt feed becomes non-empty.
    let big = codefile(&store, "big.rs");
    store
        .set_facet(&big, TargetKind::Node, "loc", "5000", TruthClass::Derived)
        .unwrap();
    let debt = loom::signal::debt(&store).unwrap();
    assert!(
        debt.iter().any(|d| d.kind == "size_outlier"),
        "setup must produce a debt cluster: {debt:?}"
    );

    // The new codefile legitimately adds coverage work; neutralize that one
    // honest delta (the shape written by `loom ignore add`) so the only
    // remaining difference could be the debt signal.
    store
        .set_meta(
            "ignores",
            &serde_json::to_string(&serde_json::json!([
                {"glob": "*.rs", "reason": "test: exclude coverage work to isolate the debt signal"},
            ]))
            .unwrap(),
        )
        .unwrap();
    let ladder_isolated = serde_json::to_value(loom::maturity::ladder(&store).unwrap()).unwrap();
    let queues_isolated = serde_json::to_value(loom::maturity::depths(&store).unwrap()).unwrap();

    // Grow the outlier tenfold: a *pure* statistical change. Nothing routed,
    // nothing gated may move.
    store
        .set_facet(&big, TargetKind::Node, "loc", "50000", TruthClass::Derived)
        .unwrap();
    assert!(loom::signal::debt(&store)
        .unwrap()
        .iter()
        .any(|d| d.kind == "size_outlier"));
    let ladder_after = serde_json::to_value(loom::maturity::ladder(&store).unwrap()).unwrap();
    let queues_after = serde_json::to_value(loom::maturity::depths(&store).unwrap()).unwrap();
    assert_eq!(
        ladder_isolated, ladder_after,
        "a statistical signal must never be a maturity gate input"
    );
    assert_eq!(
        queues_isolated, queues_after,
        "a statistical signal must never change what loom next serves"
    );

    // And no served work item may carry the statistical signal.
    if let Some(item) = loom::workitem::next(&store, None).unwrap() {
        let rendered = serde_json::to_string(&item).unwrap();
        assert!(
            !rendered.contains("size_outlier"),
            "loom next must never serve a debt cluster: {rendered}"
        );
    }
}

/// Real temp-git path: 12 analyzable commits with a strong a+b co-change pair
/// surface exactly one advisory `co_change` cluster. INV-3: feed computation
/// never writes, never gates maturity, never enters the next queue.
#[test]
fn debt_detects_git_cochange_and_is_advisory() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a_id = codefile(&store, "a.rs");
    let b_id = codefile(&store, "b.rs");
    let _c_id = codefile(&store, "c.rs");

    git_ok(tmp.path(), &["init"]);
    git_ok(tmp.path(), &["config", "user.name", "loom-test"]);
    git_ok(
        tmp.path(),
        &["config", "user.email", "loom-test@example.com"],
    );
    git_ok(tmp.path(), &["config", "commit.gpgsign", "false"]);

    // 4 joint a+b, 1 solo a, 1 solo b, 6 solo c → 12 analyzable commits.
    // joint=4, sa=5, sb=5, N=12: Jaccard 4/6, dir 4/5, lift 4*12/(5*5)=1.92.
    let mut stamp = 0u32;
    for i in 1..=4 {
        stamp += 1;
        write_repo_file(tmp.path(), "a.rs", &format!("a-joint-{i}-{stamp}"));
        write_repo_file(tmp.path(), "b.rs", &format!("b-joint-{i}-{stamp}"));
        git_commit_paths(
            tmp.path(),
            &["a.rs", "b.rs"],
            &format!("joint-ab-{i}"),
            &format!("2001-01-{:02}T12:00:00 +0000", i),
        );
    }
    stamp += 1;
    write_repo_file(tmp.path(), "a.rs", &format!("a-solo-{stamp}"));
    git_commit_paths(
        tmp.path(),
        &["a.rs"],
        "solo-a-1",
        "2001-01-05T12:00:00 +0000",
    );
    stamp += 1;
    write_repo_file(tmp.path(), "b.rs", &format!("b-solo-{stamp}"));
    git_commit_paths(
        tmp.path(),
        &["b.rs"],
        "solo-b-1",
        "2001-01-06T12:00:00 +0000",
    );
    for i in 1..=6 {
        stamp += 1;
        write_repo_file(tmp.path(), "c.rs", &format!("c-solo-{i}-{stamp}"));
        git_commit_paths(
            tmp.path(),
            &["c.rs"],
            &format!("solo-c-{i}"),
            &format!("2001-01-{:02}T12:00:00 +0000", 6 + i),
        );
    }

    let snap_before = store.snapshot().unwrap();
    let edges_before = store.list_edges(None, usize::MAX).unwrap().len();
    let ladder_before = serde_json::to_value(loom::maturity::ladder(&store).unwrap()).unwrap();
    let queues_before = serde_json::to_value(loom::maturity::depths(&store).unwrap()).unwrap();

    let debt1 = loom::signal::debt(&store).unwrap();
    let debt2 = loom::signal::debt(&store).unwrap();

    let co: Vec<_> = debt1.iter().filter(|d| d.kind == "co_change").collect();
    assert_eq!(
        co.len(),
        1,
        "exactly one co_change cluster expected, got {debt1:?}"
    );
    let row = co[0];
    assert_eq!(row.kind, "co_change");
    assert!(
        row.cluster_id.starts_with('c') && row.cluster_id.len() == 17,
        "cluster_id must be c + 16 hex, got {}",
        row.cluster_id
    );
    assert!(
        row.cluster_id[1..].chars().all(|ch| ch.is_ascii_hexdigit()),
        "cluster_id hex tail invalid: {}",
        row.cluster_id
    );
    let mut expected_subjects = vec![a_id.clone(), b_id.clone()];
    expected_subjects.sort();
    assert_eq!(
        row.subject_ids, expected_subjects,
        "subject_ids must be sorted CodeFile ids for a.rs+b.rs"
    );
    assert!(
        row.message.contains("a.rs, b.rs"),
        "message must list sorted paths: {}",
        row.message
    );
    assert!(
        row.message.contains("4/12"),
        "message must report joint support over analyzable commits: {}",
        row.message
    );
    assert_eq!(row.impact, 4, "impact = joint_support * (members-1)");
    assert_eq!(
        row.cluster_id,
        loom::signal::debt_cluster_id("co_change", &expected_subjects)
    );

    let ser1 = serde_json::to_value(&debt1).unwrap();
    let ser2 = serde_json::to_value(&debt2).unwrap();
    assert_eq!(ser1, ser2, "debt feed must be value-stable across calls");
    assert_eq!(
        serde_json::to_string(&debt1).unwrap(),
        serde_json::to_string(&debt2).unwrap(),
        "debt feed must be byte-stable across calls"
    );

    let snap_after = store.snapshot().unwrap();
    let edges_after = store.list_edges(None, usize::MAX).unwrap().len();
    let ladder_after = serde_json::to_value(loom::maturity::ladder(&store).unwrap()).unwrap();
    let queues_after = serde_json::to_value(loom::maturity::depths(&store).unwrap()).unwrap();
    assert_eq!(
        snap_before, snap_after,
        "debt must not mutate the graph snapshot"
    );
    assert_eq!(edges_before, edges_after, "debt must never store edges");
    assert_eq!(
        ladder_before, ladder_after,
        "debt must never be a maturity gate input"
    );
    assert_eq!(
        queues_before, queues_after,
        "debt must never change queue counts"
    );

    if let Some(item) = loom::workitem::next(&store, None).unwrap() {
        let rendered = serde_json::to_string(&item).unwrap();
        assert!(
            !rendered.contains("co_change"),
            "loom next must never serve a co_change debt cluster: {rendered}"
        );
        assert!(
            !rendered.contains(&row.cluster_id),
            "loom next must never carry the debt cluster id: {rendered}"
        );
    }
}

/// Non-git roots skip co-change silently: size_outlier still fires, and the
/// public `loom debt --json` surface stays exit-zero with the same rows.
#[test]
fn debt_gracefully_skips_history_outside_git() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    // loc facets: three small, one huge → size_outlier (existing four-LOC fixture).
    let mut big_id = String::new();
    for (p, loc) in [("a.rs", 40), ("b.rs", 50), ("c.rs", 45), ("big.rs", 5000)] {
        let id = codefile(&store, p);
        if p == "big.rs" {
            big_id = id.clone();
        }
        store
            .set_facet(
                &id,
                TargetKind::Node,
                "loc",
                &loc.to_string(),
                TruthClass::Derived,
            )
            .unwrap();
    }

    let debt = loom::signal::debt(&store).unwrap();
    assert!(
        debt.iter().any(|d| d.kind == "size_outlier"),
        "size_outlier must fire without git: {debt:?}"
    );
    assert!(
        !debt.iter().any(|d| d.kind == "co_change"),
        "co_change must be absent outside a git repo: {debt:?}"
    );
    let outlier = debt
        .iter()
        .find(|d| d.kind == "size_outlier")
        .expect("size_outlier row");
    assert!(
        outlier.cluster_id.starts_with('c') && outlier.cluster_id.len() == 17,
        "stable c-prefixed cluster_id required: {}",
        outlier.cluster_id
    );
    assert_eq!(
        outlier.cluster_id,
        loom::signal::debt_cluster_id("size_outlier", &[big_id.clone()])
    );
    assert!(
        outlier.message.contains("big.rs"),
        "outlier message names the file: {}",
        outlier.message
    );

    drop(store);
    let cli = run_cli_json(tmp.path(), &["debt"]);
    let rows = cli
        .as_array()
        .unwrap_or_else(|| panic!("loom debt --json must be a top-level array: {cli}"));
    assert!(
        rows.iter().any(|r| r["kind"] == "size_outlier"),
        "CLI must expose size_outlier: {cli}"
    );
    assert!(
        !rows.iter().any(|r| r["kind"] == "co_change"),
        "CLI must not invent co_change outside git: {cli}"
    );
    let cli_outlier = rows
        .iter()
        .find(|r| r["kind"] == "size_outlier")
        .expect("CLI size_outlier");
    assert_eq!(
        cli_outlier["cluster_id"].as_str().unwrap_or(""),
        outlier.cluster_id
    );
}

/// `loom debt` JSON/text retain legacy keys, add a stable copyable cluster id,
/// and remain pure reads (INV-3) across repeated invocations.
#[test]
fn debt_cli_json_and_text_expose_stable_cluster_ids() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    for (p, loc) in [("a.rs", 40), ("b.rs", 50), ("c.rs", 45), ("big.rs", 5000)] {
        let id = codefile(&store, p);
        store
            .set_facet(
                &id,
                TargetKind::Node,
                "loc",
                &loc.to_string(),
                TruthClass::Derived,
            )
            .unwrap();
    }
    let nodes_before = store.list_nodes(None, usize::MAX).unwrap().len();
    let edges_before = store.list_edges(None, usize::MAX).unwrap().len();
    let facets_before = store.snapshot().unwrap().facets.len();
    drop(store);

    let json1 = run_cli_json(tmp.path(), &["debt"]);
    let json2 = run_cli_json(tmp.path(), &["debt"]);
    assert_eq!(
        json1, json2,
        "debt --json must be value-stable across reads"
    );
    assert_eq!(
        serde_json::to_string(&json1).unwrap(),
        serde_json::to_string(&json2).unwrap(),
        "debt --json must be byte-stable across reads"
    );
    let rows = json1
        .as_array()
        .unwrap_or_else(|| panic!("top-level JSON array required: {json1}"));
    assert!(
        !rows.is_empty(),
        "outlier fixture must produce at least one debt row"
    );
    for row in rows {
        for key in ["kind", "message", "impact", "confirm", "cluster_id"] {
            assert!(
                row.get(key).is_some(),
                "debt JSON row missing retained/new key '{key}': {row}"
            );
        }
        // subject_ids are intentionally not serialized into the feed.
        assert!(
            row.get("subject_ids").is_none(),
            "subject_ids must stay off the wire: {row}"
        );
        let cid = row["cluster_id"]
            .as_str()
            .unwrap_or_else(|| panic!("cluster_id must be a string: {row}"));
        assert!(
            cid.starts_with('c') && cid.len() == 17,
            "cluster_id must be c + 16 hex (17 chars): {cid}"
        );
        assert!(
            cid[1..].chars().all(|ch| ch.is_ascii_hexdigit()),
            "cluster_id hex tail invalid: {cid}"
        );
    }
    let cluster_id = rows[0]["cluster_id"].as_str().unwrap().to_string();
    let kind = rows[0]["kind"].as_str().unwrap();
    let message = rows[0]["message"].as_str().unwrap();
    let impact = rows[0]["impact"].as_u64().unwrap();
    let confirm = rows[0]["confirm"].as_str().unwrap();

    let (status, stdout, stderr) = run_cli_raw(tmp.path(), &["debt"]);
    assert!(
        status.success(),
        "loom debt text must exit zero: {status:?}\n--stderr--\n{stderr}"
    );
    let primary = format!("[{kind}] {message} (impact {impact})");
    assert!(
        stdout.contains(&primary),
        "text must keep the primary ranked line:\nexpected substring: {primary}\n--stdout--\n{stdout}"
    );
    assert!(
        stdout.contains(&format!("    id: {cluster_id}")),
        "text must expose a copyable id line for {cluster_id}:\n{stdout}"
    );
    assert!(
        stdout.contains(&format!("    confirm: {confirm}")),
        "text must keep the confirm line:\n{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "{} ranked signal(s) — advisory, never required",
            rows.len()
        )),
        "text must keep the advisory footer:\n{stdout}"
    );

    let store = Store::open(tmp.path()).unwrap();
    assert_eq!(
        store.list_nodes(None, usize::MAX).unwrap().len(),
        nodes_before,
        "debt reads must not create/remove nodes"
    );
    assert_eq!(
        store.list_edges(None, usize::MAX).unwrap().len(),
        edges_before,
        "debt reads must not create/remove edges"
    );
    assert_eq!(
        store.snapshot().unwrap().facets.len(),
        facets_before,
        "debt reads must not create/remove facets"
    );
}

/// Write a tracked source file under the temp repo root.
fn write_repo_file(dir: &Path, rel: &str, body: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, body).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

/// Run `git` in `dir` with argument arrays only (no shell). Panics with
/// stdout/stderr on non-zero exit so a red co-change fixture is diagnosable.
fn git_ok(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap_or_else(|e| panic!("spawn git {:?}: {e}", args));
    assert!(
        out.status.success(),
        "git {:?} failed: {:?}\n--stdout--\n{}\n--stderr--\n{}",
        args,
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Stage only the listed paths and create a deterministic non-empty commit.
fn git_commit_paths(dir: &Path, paths: &[&str], message: &str, date: &str) {
    let mut add_args = vec!["add", "--"];
    add_args.extend_from_slice(paths);
    git_ok(dir, &add_args);

    let out = std::process::Command::new("git")
        .args(["-c", "commit.gpgsign=false", "commit", "-m", message])
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .unwrap_or_else(|e| panic!("spawn git commit: {e}"));
    assert!(
        out.status.success(),
        "git commit -m {:?} paths {:?} failed: {:?}\n--stdout--\n{}\n--stderr--\n{}",
        message,
        paths,
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---- doctor ----------------------------------------------------------------

#[test]
fn doctor_clean_on_valid_graph() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let i = intent(&store, "user can log in", "implemented");
    let cf = codefile(&store, "src/auth.rs");
    let e = store
        .add_edge(EdgeKind::Implements, &i, &cf, TruthClass::Asserted)
        .unwrap();
    store
        .record_verdict(
            &e.id,
            InspectionStatus::Passing,
            "c",
            "src/auth.rs:1",
            0.9,
            "llm",
        )
        .unwrap();
    let issues = loom::signal::doctor(&store).unwrap();
    assert!(
        issues.is_empty(),
        "valid graph must pass doctor: {issues:?}"
    );
}

#[test]
fn doctor_cli_reports_all_issues_deterministically_and_fails_closed() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let parent = intent(&store, "parent behavior", "implemented");
    let child = intent(&store, "child behavior", "implemented");
    store
        .add_edge(EdgeKind::Hierarchy, &parent, &child, TruthClass::Asserted)
        .unwrap();
    store
        .add_edge(EdgeKind::Hierarchy, &child, &parent, TruthClass::Asserted)
        .unwrap();
    store
        .add_node(
            NodeType::Validation,
            "damaged  proof",
            "",
            "not_run",
            serde_json::json!({}),
        )
        .unwrap();
    drop(store);

    let (first_status, first_stdout, first_stderr) = run_cli_raw(tmp.path(), &["doctor", "--json"]);
    let (second_status, second_stdout, second_stderr) =
        run_cli_raw(tmp.path(), &["doctor", "--json"]);

    assert!(!first_status.success(), "invalid graph must fail doctor");
    assert!(!second_status.success(), "repeated doctor must still fail");
    let first: serde_json::Value = serde_json::from_str(&first_stdout).unwrap();
    let second: serde_json::Value = serde_json::from_str(&second_stdout).unwrap();
    assert_eq!(first, second, "doctor JSON must be repeatable");

    let issues = first
        .as_array()
        .expect("doctor JSON must be an issue array");
    assert_eq!(issues.len(), 2, "doctor must report both violations");
    let kinds: Vec<_> = issues
        .iter()
        .map(|issue| issue["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        ["hierarchy_cycle", "malformed_validation_name"],
        "doctor must use canonical (kind, message) order"
    );

    for stderr in [&first_stderr, &second_stderr] {
        assert!(
            stderr.contains("doctor found 2 integrity issue(s)"),
            "doctor must report the complete failure count: {stderr}"
        );
    }
    for output in [&first_stdout, &first_stderr, &second_stdout, &second_stderr] {
        assert!(
            !output.contains("doctor: clean"),
            "invalid graph must never be reported clean: {output}"
        );
    }
}

#[test]
fn doctor_flags_restored_placeholder_criterion_verdicts() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let from = intent(&store, "legacy source behavior", "implemented");
    let to = intent(&store, "legacy target behavior", "implemented");
    let passing = store
        .add_edge(EdgeKind::Relates, &from, &to, TruthClass::Asserted)
        .unwrap();
    store
        .record_verdict(
            &passing.id,
            InspectionStatus::Passing,
            "source behavior reaches target behavior",
            "manual inspection found source behavior supports target behavior",
            0.9,
            "llm",
        )
        .unwrap();

    let blocked_from = intent(&store, "blocked source behavior", "implemented");
    let blocked_to = intent(&store, "blocked target behavior", "implemented");
    let blocked = store
        .add_edge(
            EdgeKind::Relates,
            &blocked_from,
            &blocked_to,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &blocked.id,
            InspectionStatus::Blocked,
            "blocked pending upstream behavior",
            "upstream contract has not been published yet",
            0.2,
            "llm",
        )
        .unwrap();

    let passing_id = passing.id.clone();
    let blocked_id = blocked.id.clone();
    let mut snap = store.snapshot().unwrap();
    // Criterion lives on the FACT now — the edge column is a projection of it,
    // so corrupting the import means corrupting the fact.
    snap.facts
        .iter_mut()
        .find(|f| f.subject_id == passing_id)
        .expect("the passing verdict travels as a fact")
        .criterion = "…".into();
    // A blocked verdict's reason is evidence, and evidence no longer lives on
    // the edge — the import path carries no anchor for it at all, which is
    // exactly what the doctor check below should notice.
    let _ = &blocked_id;

    let restored_tmp = Tmp::new();
    let mut restored = Store::init(restored_tmp.path(), Some("import"), false).unwrap();
    restored.restore(&snap).unwrap();

    let issues = loom::signal::doctor(&restored).unwrap();
    assert!(
        issues.iter().any(|issue| {
            issue.kind == "vacuous_verdict"
                && issue.message.contains(&passing_id)
                && issue.message.contains("criterion")
        }),
        "doctor must flag restored placeholder criterion on a passing edge: {issues:?}"
    );
    // A blocked verdict with no reason at all is now refused at the BOUNDARY
    // rather than detected afterwards: its reason is the only evidence it has,
    // and a fact with no live anchor cannot be written. Catching it at write
    // time beats catching it in an audit.
    let refused = store.record_verdict(
        &blocked_id,
        InspectionStatus::Blocked,
        "criterion",
        "",
        0.5,
        "llm",
    );
    let err = refused.expect_err("a blocked verdict with no reason must be refused");
    assert!(
        err.to_string().contains("blocker"),
        "the refusal must name what would fix it: {err}"
    );
}

#[test]
fn doctor_flags_hierarchy_cycles() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let parent = intent(&store, "parent behavior", "implemented");
    let child = intent(&store, "child behavior", "implemented");
    store
        .add_edge(EdgeKind::Hierarchy, &parent, &child, TruthClass::Asserted)
        .unwrap();
    store
        .add_edge(EdgeKind::Hierarchy, &child, &parent, TruthClass::Asserted)
        .unwrap();

    let issues = loom::signal::doctor(&store).unwrap();
    assert!(
        issues.iter().any(|issue| issue.kind == "hierarchy_cycle"),
        "cyclic hierarchy must be reported by doctor: {issues:?}"
    );
}

// ---- Journey-root map -------------------------------------------------------

#[test]
fn journey_map_reports_rooted_and_unrooted_intents_from_derives() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    tmp.write(
        "journeys/flow.yaml",
        "schema: loom.journey/v1\nid: checkout-flow\nname: Checkout flow\nactor: shopper\ngoal: Complete checkout\ninputs: {}\npreconditions: []\nsteps:\n  - id: checkout\n    name: Checkout\n    action: checks out\n    expects: []\n    produces: {}\nprofiles:\n  proof:\n    inputs: {}\n    workspace: {}\n",
    );
    let spec = loom::journey::parse(&tmp.path().join("journeys/flow.yaml")).unwrap();
    let hash = spec.semantic_hash().unwrap();
    let journey = store
        .add_node(
            NodeType::Journey,
            "checkout-flow",
            "Checkout flow",
            "authored",
            serde_json::json!({
                "schema":"loom.journey/v1",
                "stable_id":"checkout-flow",
                "name":"Checkout flow",
                "artifact":"journeys/flow.yaml",
                "semantic_hash":hash,
                "step_ids":["checkout"],
                "input_ids":[],
                "preconditions":[],
                "output_ids":[],
                "profile_ids":["proof"]
            }),
        )
        .unwrap();
    let rooted = store
        .add_node(
            NodeType::Intent,
            "checkout records a result",
            "a checkout emits one recorded result",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    let unrooted = store
        .add_node(
            NodeType::Intent,
            "receipts can be emailed",
            "a receipt reaches the shopper",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    let derives = store
        .add_edge(
            EdgeKind::Derives,
            &journey.id,
            &rooted.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &derives.id,
            TargetKind::Edge,
            "journey_hash",
            &hash,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &derives.id,
            TargetKind::Edge,
            "step_ids",
            "[\"checkout\"]",
            TruthClass::Asserted,
        )
        .unwrap();
    drop(store);

    let out = run_cli_json(tmp.path(), &["journey", "map"]);
    assert_eq!(out["journeys"][0]["journey_name"], "checkout-flow", "{out}");
    assert_eq!(out["journeys"][0]["derived"], true, "{out}");
    let unrooted_rows = out["unrooted_intents"].as_array().unwrap();
    assert!(
        unrooted_rows.iter().any(|row| row["id"] == unrooted.id),
        "{out}"
    );
    assert!(
        !unrooted_rows.iter().any(|row| row["id"] == rooted.id),
        "{out}"
    );
}

// ---- note targets: edges, node precedence, and the no-match error ----------

/// Contract: a note can be attached to an EDGE (by id or prefix) and scoped
/// `note list` returns exactly that note with `target_id` equal to the full
/// edge id. Adjudications live on claims, and claims live on edges too.
#[test]
fn note_add_attaches_to_edge_and_list_scopes_to_edge() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let _a = intent(&store, "intent alpha", "implemented");
    let _b = intent(&store, "intent beta", "implemented");
    drop(store);

    // Create the relates edge via the surface under test and capture its id.
    let relate = run_cli_json(
        tmp.path(),
        &["edge", "relate", "relates", "intent alpha", "intent beta"],
    );
    let edge_id = relate["edge"]["id"]
        .as_str()
        .expect("edge relate --json emits the edge with its id")
        .to_string();
    assert!(
        !edge_id.is_empty(),
        "relate must produce a non-empty edge id, got: {relate}"
    );
    let prefix = &edge_id[..8];

    // Attach a warning note by the edge id PREFIX (the resolution path that
    // distinguishes edges from nodes must accept the short form too).
    let added = run_cli_json(
        tmp.path(),
        &[
            "note",
            "add",
            prefix,
            "--kind",
            "warning",
            "--text",
            "verdict recorded from wrong lane",
        ],
    );
    assert_eq!(
        added["target"]["id"].as_str(),
        Some(edge_id.as_str()),
        "note add resolves the prefix to the full edge id, got: {}",
        serde_json::to_string_pretty(&added).unwrap()
    );

    // note list scoped to the edge returns exactly that note, with target_id
    // equal to the FULL edge id (not the prefix we passed).
    let notes = run_cli_json(tmp.path(), &["note", "list", &edge_id]);
    let arr = notes["items"]
        .as_array()
        .expect("note list --json emits items");
    assert_eq!(
        arr.len(),
        1,
        "exactly one note scoped to the edge, got: {}",
        serde_json::to_string_pretty(&notes).unwrap()
    );
    assert_eq!(
        arr[0]["target_id"].as_str(),
        Some(edge_id.as_str()),
        "the scoped note's target_id is the full edge id, got: {}",
        serde_json::to_string_pretty(&arr[0]).unwrap()
    );
    assert_eq!(
        arr[0]["kind"].as_str(),
        Some("warning"),
        "the note kind round-trips as warning, got: {}",
        serde_json::to_string_pretty(&arr[0]).unwrap()
    );
    assert_eq!(
        arr[0]["text"].as_str(),
        Some("verdict recorded from wrong lane"),
        "the note text is preserved verbatim, got: {}",
        serde_json::to_string_pretty(&arr[0]).unwrap()
    );
}

/// Contract: node precedence — when the target string names a node (here an
/// intent), the note lands on the node even though edges exist in the graph.
/// `resolve_note_target` tries nodes first and only falls through to edges on a
/// hard "no node matches", so a name that resolves a node must never be
/// misread as an edge prefix.
#[test]
fn note_add_on_node_name_lands_on_node_not_edge() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = intent(&store, "intent alpha", "implemented");
    let _b = intent(&store, "intent beta", "implemented");
    drop(store);

    // Add a relates edge so edges exist in the graph — proving the node path
    // is chosen by precedence, not by absence of edges.
    run_cli(
        tmp.path(),
        &["edge", "relate", "relates", "intent alpha", "intent beta"],
    );

    let added = run_cli_json(
        tmp.path(),
        &["note", "add", "intent alpha", "--text", "node note"],
    );
    assert_eq!(
        added["target"]["id"].as_str(),
        Some(a.as_str()),
        "the note attached to the intent node, not an edge, got: {}",
        serde_json::to_string_pretty(&added).unwrap()
    );

    // Scoped list on the node returns the note; scoped list on the edge must
    // NOT see it — the node note did not leak onto the edge.
    let node_notes = run_cli_json(tmp.path(), &["note", "list", "intent alpha"]);
    let arr = node_notes["items"]
        .as_array()
        .expect("note list --json emits an array");
    assert_eq!(
        arr.len(),
        1,
        "one note scoped to the intent node, got: {}",
        serde_json::to_string_pretty(&node_notes).unwrap()
    );
    assert_eq!(
        arr[0]["target_id"].as_str(),
        Some(a.as_str()),
        "the note's target_id is the intent node id, got: {}",
        serde_json::to_string_pretty(&arr[0]).unwrap()
    );
}

/// Contract: a target that matches neither a node nor an edge exits non-zero
/// with a message containing "no node or edge matches" — the single error that
/// tells the operator the target could not be resolved at all.
#[test]
fn note_add_dead_target_exits_nonzero_with_no_match_message() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let _a = intent(&store, "intent alpha", "implemented");
    drop(store);

    // "deadbeef99" cannot be a node name (no node is named that), a node id
    // prefix, or an edge id prefix in this graph.
    let (status, _stdout, stderr) =
        run_cli_raw(tmp.path(), &["note", "add", "deadbeef99", "--text", "x"]);
    assert!(
        !status.success(),
        "note add on an unresolvable target must exit non-zero, got: {status:?}\n--stderr--\n{stderr}"
    );
    assert!(
        stderr.contains("no node or edge matches"),
        "stderr must name the no-match contract so the operator knows the target resolved to nothing, got: {stderr}"
    );
}

// ---- layering smell import resolution ---------------------------------------

fn intent_layer(store: &Store, name: &str, layer: &str) -> String {
    let n = store
        .add_node(
            NodeType::Intent,
            name,
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .set_facet(
            &n.id,
            TargetKind::Node,
            "layer",
            layer,
            TruthClass::Asserted,
        )
        .unwrap();
    n.id
}

fn cf_imports(store: &Store, path: &str, imports: &[&str]) -> String {
    let n = common::codefile(store, path);
    store
        .set_facet(
            &n.id,
            TargetKind::Node,
            "imports",
            &serde_json::to_string(&imports).unwrap(),
            TruthClass::Derived,
        )
        .unwrap();
    n.id
}

fn own(store: &Store, intent: &str, cf: &str) {
    store
        .add_edge(EdgeKind::Implements, intent, cf, TruthClass::Asserted)
        .unwrap();
}

fn layering_msgs(store: &Store) -> Vec<String> {
    loom::signal::smells(store)
        .unwrap()
        .into_iter()
        .filter(|s| s.kind == "layering_violation")
        .map(|s| s.message)
        .collect()
}

#[test]
fn layering_ignores_stdlib_imports() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    store
        .set_meta(
            "layer_order",
            r#"["http","application","runtime","kernel","schema","storage"]"#,
        )
        .unwrap();

    let app = intent_layer(&store, "invoke client", "application");
    let importer = cf_imports(
        &store,
        "pulse-client/src/invocation.rs",
        &["std::time::{SystemTime, UNIX_EPOCH}"],
    );
    own(&store, &app, &importer);

    let http = intent_layer(&store, "http lifecycle", "http");
    let http_decoy = cf_imports(
        &store,
        "pulse-http/src/tests/aaa_lifecycle_and_runtime.rs",
        &[],
    );
    own(&store, &http, &http_decoy);

    let runtime = intent_layer(&store, "runtime service", "runtime");
    let runtime_decoy = cf_imports(&store, "pulse-runtime/src/runtime.rs", &[]);
    own(&store, &runtime, &runtime_decoy);

    assert_eq!(layering_msgs(&store), Vec::<String>::new());
}

#[test]
fn crate_import_stays_in_importing_crate() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    store
        .set_meta(
            "layer_order",
            r#"["http","application","runtime","kernel","schema","storage"]"#,
        )
        .unwrap();

    let app = intent_layer(&store, "machine lifecycle", "application");
    let importer = cf_imports(
        &store,
        "pulse-machine/src/lifecycle.rs",
        &["crate::principal::Principal"],
    );
    own(&store, &app, &importer);

    let principal = cf_imports(&store, "pulse-machine/src/principal.rs", &[]);
    own(&store, &app, &principal);

    let http = intent_layer(&store, "http principal", "http");
    let decoy = cf_imports(&store, "pulse-http/src/principal.rs", &[]);
    own(&store, &http, &decoy);

    assert_eq!(layering_msgs(&store), Vec::<String>::new());
}

#[test]
fn super_import_resolves_sibling_not_other_crate() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    store
        .set_meta(
            "layer_order",
            r#"["http","application","runtime","kernel","schema","storage"]"#,
        )
        .unwrap();

    let storage = intent_layer(&store, "catalog entries", "storage");
    let importer = cf_imports(
        &store,
        "pulse-http/src/platform/catalog/entries.rs",
        &["super::manifest::Manifest"],
    );
    own(&store, &storage, &importer);

    let manifest = cf_imports(&store, "pulse-http/src/platform/catalog/manifest.rs", &[]);
    own(&store, &storage, &manifest);

    let http = intent_layer(&store, "agent manifest", "http");
    let decoy = cf_imports(&store, "pulse-agent/src/manifest.rs", &[]);
    own(&store, &http, &decoy);

    assert_eq!(layering_msgs(&store), Vec::<String>::new());
}

#[test]
fn crate_import_requires_path_boundary() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    store
        .set_meta(
            "layer_order",
            r#"["http","application","runtime","kernel","schema","storage"]"#,
        )
        .unwrap();

    let storage = intent_layer(&store, "webhook delivery", "storage");
    let importer = cf_imports(
        &store,
        "pulse-machine/src/webhook.rs",
        &["crate::delivery::deliver"],
    );
    own(&store, &storage, &importer);

    let delivery = cf_imports(&store, "pulse-machine/src/delivery.rs", &[]);
    own(&store, &storage, &delivery);

    let http = intent_layer(&store, "commerce delivery", "http");
    let decoy = cf_imports(&store, "pulse-http/src/commerce_delivery.rs", &[]);
    own(&store, &http, &decoy);

    assert_eq!(layering_msgs(&store), Vec::<String>::new());
}

#[test]
fn layering_skips_module_tree_parent_child() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    store
        .set_meta(
            "layer_order",
            r#"["http","application","runtime","kernel","schema","storage"]"#,
        )
        .unwrap();

    let app = intent_layer(&store, "parent module", "application");
    let parent = cf_imports(&store, "pulse-x/src/a/parent.rs", &["self::child::C"]);
    own(&store, &app, &parent);

    let http = intent_layer(&store, "child module", "http");
    let child = cf_imports(&store, "pulse-x/src/a/parent/child.rs", &[]);
    own(&store, &http, &child);

    assert_eq!(layering_msgs(&store), Vec::<String>::new());
}

#[test]
fn layering_permits_multiowner_legal_pairing() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    store
        .set_meta(
            "layer_order",
            r#"["http","application","runtime","kernel","schema","storage"]"#,
        )
        .unwrap();

    let kernel = intent_layer(&store, "grounding kernel", "kernel");
    let importer = cf_imports(
        &store,
        "pulse-graph/src/grounding.rs",
        &["crate::selection::Selection"],
    );
    own(&store, &kernel, &importer);

    let selection = cf_imports(&store, "pulse-graph/src/selection.rs", &[]);
    let selection_kernel = intent_layer(&store, "selection kernel", "kernel");
    let selection_application = intent_layer(&store, "selection application", "application");
    own(&store, &selection_kernel, &selection);
    own(&store, &selection_application, &selection);

    assert_eq!(layering_msgs(&store), Vec::<String>::new());
}

#[test]
fn layering_ignores_unresolvable_extern() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    store
        .set_meta(
            "layer_order",
            r#"["http","application","runtime","kernel","schema","storage"]"#,
        )
        .unwrap();

    let app = intent_layer(&store, "client model", "application");
    let importer = cf_imports(&store, "pulse-client/src/model.rs", &["serde::Deserialize"]);
    own(&store, &app, &importer);

    assert_eq!(layering_msgs(&store), Vec::<String>::new());
}

#[test]
fn layering_ignores_bare_extern_import() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    store
        .set_meta(
            "layer_order",
            r#"["http","application","runtime","kernel","schema","storage"]"#,
        )
        .unwrap();

    let storage = intent_layer(&store, "x model", "storage");
    let importer = cf_imports(&store, "crates/x/src/model.rs", &["serde"]);
    own(&store, &storage, &importer);

    let http = intent_layer(&store, "serde helpers", "http");
    let decoy = cf_imports(&store, "crates/y/src/serde_helpers.rs", &[]);
    own(&store, &http, &decoy);

    assert_eq!(layering_msgs(&store), Vec::<String>::new());
}

#[test]
fn layering_still_flags_real_up_import() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    store
        .set_meta(
            "layer_order",
            r#"["http","application","runtime","kernel","schema","storage"]"#,
        )
        .unwrap();

    let app = intent_layer(&store, "api", "application");
    let api = cf_imports(&store, "pulse-x/src/api.rs", &["crate::db::Db"]);
    own(&store, &app, &api);

    let http = intent_layer(&store, "db", "http");
    let db = cf_imports(&store, "pulse-x/src/db.rs", &[]);
    own(&store, &http, &db);

    assert_eq!(
        layering_msgs(&store),
        vec![
            "pulse-x/src/api.rs (layer application) imports pulse-x/src/db.rs (layer http) — points up the declared order"
                .to_string()
        ]
    );
}

#[test]
fn layering_resolves_extern_crate_dependency() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    store
        .set_meta(
            "layer_order",
            r#"["http","application","runtime","kernel","schema","storage"]"#,
        )
        .unwrap();

    let storage = intent_layer(&store, "machine client", "storage");
    let client = cf_imports(
        &store,
        "pulse-machine/src/client.rs",
        &["pulse_http::api::Client"],
    );
    own(&store, &storage, &client);

    let http = intent_layer(&store, "http api", "http");
    let api = cf_imports(&store, "pulse-http/src/api.rs", &[]);
    own(&store, &http, &api);

    assert_eq!(
        layering_msgs(&store),
        vec![
            "pulse-machine/src/client.rs (layer storage) imports pulse-http/src/api.rs (layer http) — points up the declared order"
                .to_string()
        ]
    );
}

#[test]
fn layering_resolves_extern_in_nested_workspace() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    store
        .set_meta(
            "layer_order",
            r#"["http","application","runtime","kernel","schema","storage"]"#,
        )
        .unwrap();

    let storage = intent_layer(&store, "nested machine client", "storage");
    let client = cf_imports(
        &store,
        "crates/pulse-machine/src/client.rs",
        &["pulse_http::api::Client"],
    );
    own(&store, &storage, &client);

    let http = intent_layer(&store, "nested http api", "http");
    let api = cf_imports(&store, "crates/pulse-http/src/api.rs", &[]);
    own(&store, &http, &api);

    assert_eq!(
        layering_msgs(&store),
        vec![
            "crates/pulse-machine/src/client.rs (layer storage) imports crates/pulse-http/src/api.rs (layer http) — points up the declared order"
                .to_string()
        ]
    );
}
// ---- threshold CLI (hand-set structural finding gates) ---------------------
//
// The feature is the CLI, not just the API: these exercise `loom threshold
// set/list/reset` end-to-end through the binary — process startup, the open/
// save/clear wiring, and the snapshot().config surface a downstream consumer
// (export) sees. The error paths (value 0, unknown gate) must exit non-zero
// with a message naming the offender; the happy paths must persist to portable
// config and, after `reset`, drop the key entirely (absent = defaults, not a
// pinned snapshot).

/// Run `loom` with arbitrary args (no --graph) and return (status, stdout,
/// stderr) without asserting on exit code — for `init` and error-path cases.
fn run_loom_raw(args: &[&str]) -> (std::process::ExitStatus, String, String) {
    let mut cmd = loom_command();
    cmd.args(args);
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("spawn loom {:?}: {e}", args));
    (
        out.status,
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Contract: the `loom threshold` command persists gates to portable config
/// (`config.thresholds` appears in the snapshot after `set`), reports them via
/// `threshold list --json`, and — after `threshold reset` (all) — drops the
/// config key so `load` reverts to the shipped defaults (absent = defaults,
/// never a pinned snapshot). The CLI's `>= 1` guard and unknown-gate rejection
/// must exit non-zero with a message naming the offender.
#[test]
fn threshold_cli_persists_lists_resets_and_rejects_bad_input() {
    let tmp = Tmp::new();
    let graph = tmp.path();

    // Init the graph via the BINARY (Command::Init takes a positional path,
    // not --graph) — exercising the same init wiring a real operator runs.
    let (status, _stdout, stderr) = run_loom_raw(&["init", graph.to_str().unwrap()]);
    assert!(
        status.success(),
        "loom init <path> must succeed: {status:?}\n--stderr--\n{stderr}"
    );

    // set max_args to a non-default value (default is 6) via the binary.
    run_cli(graph, &["threshold", "set", "max_args", "8"]);

    // threshold list --json reflects the set value.
    let listed = run_cli_json(graph, &["threshold", "list"]);
    assert_eq!(
        listed["max_args"], 8,
        "threshold list --json reports the persisted max_args=8: {listed}"
    );

    // The portable config key appears in the snapshot after set. Open the store
    // in a short block and drop it before the next CLI call — Store::open takes
    // the write lock, and holding it across a child `threshold reset` would
    // make the CLI contend on the locked graph.
    {
        let store = Store::open(graph).expect("open the graph the binary inited");
        let snap = store.snapshot().expect("snapshot reads the live config");
        assert!(
            snap.config.contains_key("thresholds"),
            "snapshot().config carries the 'thresholds' portable key after set: {:?}",
            snap.config.keys().collect::<Vec<_>>()
        );
    }

    // threshold reset (all gates) drops the config entirely — absent = defaults,
    // not a pinned snapshot of today's values. A regression that wrote the
    // default values instead of removing the key would leave it present here.
    run_cli(graph, &["threshold", "reset"]);

    {
        let store = Store::open(graph).expect("open the graph after reset");
        let snap = store.snapshot().expect("snapshot after reset");
        assert!(
            !snap.config.contains_key("thresholds"),
            "after `threshold reset` the 'thresholds' key is gone (absent = defaults), \
             still present: {:?}",
            snap.config.get("thresholds")
        );
    }

    // The CLI's `>= 1` guard (lives in the handler, not the API) rejects 0 with
    // a non-zero exit — 0 would flag every symbol/file.
    let (status, _stdout, stderr) = run_cli_raw(graph, &["threshold", "set", "max_args", "0"]);
    assert!(
        !status.success(),
        "threshold set max_args 0 must exit non-zero, got {status:?}\n--stderr--\n{stderr}"
    );
    assert!(
        stderr.contains(">= 1") || stderr.contains("must be >= 1"),
        "the zero-value error names the >= 1 guard: {stderr}"
    );

    // An unknown gate must exit non-zero with a message naming the gate.
    let (status, _stdout, stderr) = run_cli_raw(graph, &["threshold", "set", "max_bogus", "3"]);
    assert!(
        !status.success(),
        "threshold set max_bogus 3 must exit non-zero, got {status:?}\n--stderr--\n{stderr}"
    );
    assert!(
        stderr.contains("max_bogus"),
        "the unknown-gate error names the offending gate: {stderr}"
    );
}

/// A retired behavior owns nothing, so it generates no structural smells.
///
/// The third place retirement was not respected. `active_intents` correctly
/// skipped deprecated intents, but the ownership index the co-ownership smells
/// read was built from every `implements` edge — so a behavior deleted on
/// purpose kept co-owning its files and kept producing findings about them.
/// Triage served me one naming the very intent I had just retired.
#[test]
fn a_retired_intent_stops_co_owning_its_files() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let shared = codefile(&store, "src/shared.rs");
    let mut ids = Vec::new();
    for name in ["a behavior that stays", "a behavior to be removed"] {
        let i = store
            .add_node(
                NodeType::Intent,
                name,
                "d",
                "implemented",
                serde_json::json!({}),
            )
            .unwrap();
        store
            .add_edge(EdgeKind::Implements, &i.id, &shared, TruthClass::Asserted)
            .unwrap();
        ids.push(i.id);
    }

    let coupled = |s: &Store| {
        loom::signal::smells(s)
            .unwrap()
            .into_iter()
            .filter(|x| x.message.contains("src/shared.rs"))
            .count()
    };
    assert!(
        coupled(&store) > 0,
        "two live owners with no relationship is a real smell"
    );

    store
        .retire_intent(&ids[1], "deleted on purpose", None)
        .unwrap();
    assert_eq!(
        coupled(&store),
        0,
        "one live owner remains — a retired behavior is not a co-owner"
    );
}
