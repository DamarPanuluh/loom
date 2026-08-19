//! Ring 64 — bounded autonomous adversarial review.
//!
//! Defends the existing Review-lane seam: a fixed high-risk frontier emits
//! analyzer-owned challenge packets; durable Challenge facts close only one
//! Verdict revision; counterexamples route through Findings without rewriting
//! truth; and reviewer-profile independence is advisory rather than blocking.

mod common;

use common::{codefile, loom_command, Tmp};
use loom::model::{Claim, EdgeKind, NodeType, TruthClass};
use loom::registry::OwnerRole;
use loom::store::{Agent, Assertion, Store, Subject};

fn as_driver(
    root: &std::path::Path,
    lane: &str,
    profile: &str,
    args: &[&str],
) -> std::process::Output {
    let mut command = loom_command();
    command.arg("--graph").arg(root);
    command.env("LOOM_AGENT", format!("llm:{lane}"));
    command.env("LOOM_AGENT_PROFILE", profile);
    command.args(args);
    command.output().unwrap()
}

fn json_driver(
    root: &std::path::Path,
    lane: &str,
    profile: &str,
    args: &[&str],
) -> serde_json::Value {
    let mut command = loom_command();
    command.arg("--graph").arg(root);
    if !lane.is_empty() {
        command.env("LOOM_AGENT", format!("llm:{lane}"));
        command.env("LOOM_AGENT_PROFILE", profile);
    }
    command.args(args).arg("--json");
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "loom {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn one_green_edge(tmp: &Tmp) -> String {
    let store = Store::init(tmp.path(), Some("adversarial-review"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "authorization is enforced",
            "requests without authority are rejected",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let file = codefile(&store, "src/authorization.rs");
    let edge = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &file.id,
            TruthClass::Asserted,
        )
        .unwrap();
    drop(store);

    let output = as_driver(
        tmp.path(),
        "builder",
        "builder-a",
        &[
            "edge",
            "verdict",
            &edge.id,
            "ground",
            "--criterion",
            "unauthorized requests must be rejected",
            "--evidence",
            "src/authorization.rs:1 — implementation rejects unauthorized requests",
            "--confidence",
            "0.95",
        ],
    );
    assert!(
        output.status.success(),
        "verdict failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    edge.id
}

#[test]
fn next_drives_challenge_then_snapshot_change_reopens_and_counterexample_routes() {
    let tmp = Tmp::new();
    let edge_id = one_green_edge(&tmp);

    let packet = json_driver(
        tmp.path(),
        "analyzer",
        "reviewer-b",
        &["next", "--mode", "review"],
    );
    assert_eq!(packet["work_item"]["mode"], "review");
    assert_eq!(packet["work_item"]["target"]["kind"], "edge_challenge");
    assert_eq!(packet["work_item"]["review"]["variant"], "adversarial");
    assert_eq!(
        packet["work_item"]["review"]["prefer_profile_not"],
        "builder-a"
    );
    assert_eq!(packet["work_item"]["owner_role"], "analyzer");

    let survived = json_driver(
        tmp.path(),
        "analyzer",
        "reviewer-b",
        &[
            "challenge",
            "record",
            &edge_id,
            "survived",
            "--hypothesis",
            "the implementation accepts a request without an authority token",
            "--evidence",
            "src/authorization.rs:1 — inspected the rejection path and found no bypass",
            "--confidence",
            "0.82",
        ],
    );
    assert_eq!(survived["challenge"]["state"], "survived");
    assert!(survived["finding"].is_null());
    assert!(survived["independence_warning"].is_null());
    assert_eq!(survived["graph_state"]["adversarial_review"], 0);

    // A new Verdict basis invalidates the Store-minted FactSnapshot and puts
    // this exact edge back into adversarial Review after one reverify pass.
    let changed = as_driver(
        tmp.path(),
        "builder",
        "builder-a",
        &[
            "edge",
            "verdict",
            &edge_id,
            "ground",
            "--criterion",
            "all unauthorized requests, including empty tokens, must be rejected",
            "--evidence",
            "src/authorization.rs:1 — implementation checks the authority path",
            "--confidence",
            "0.96",
        ],
    );
    assert!(changed.status.success());
    json_driver(tmp.path(), "", "", &["sync"]);
    let status = json_driver(tmp.path(), "", "", &["status"]);
    assert_eq!(status["graph_state"]["adversarial_review"], 1);

    let counterexample = json_driver(
        tmp.path(),
        "analyzer",
        "reviewer-b",
        &[
            "challenge",
            "record",
            &edge_id,
            "counterexample",
            "--hypothesis",
            "an empty authority token reaches the protected operation",
            "--evidence",
            "src/authorization.rs:1 — the empty-token branch lacks an early return",
            "--impact",
            "an unauthenticated caller can reach protected behavior",
            "--confidence",
            "0.9",
        ],
    );
    assert_eq!(counterexample["challenge"]["state"], "counterexample");
    assert_eq!(
        counterexample["finding"]["body"]["source"],
        "adversarial_review"
    );
    assert_eq!(
        counterexample["finding"]["body"]["challenged_edge_id"],
        edge_id
    );
    assert_eq!(counterexample["next_step"], "loom next --mode triage");

    let store = Store::open_read(tmp.path()).unwrap();
    assert_eq!(
        store.resolve_edge(&edge_id).unwrap().status.as_str(),
        "passing"
    );
    let finding_id = counterexample["finding"]["id"].as_str().unwrap();
    assert!(store.get_node(finding_id).unwrap().is_some());
}

#[test]
fn fixed_frontier_closes_after_five_instead_of_walking_the_sixth_claim() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("bounded-frontier"), false).unwrap();
    let mut edge_files = Vec::new();
    for n in 0..6 {
        let intent = store
            .add_node(
                NodeType::Intent,
                &format!("behavior {n}"),
                "a settled behavior",
                "implemented",
                serde_json::json!({}),
            )
            .unwrap();
        let path = format!("src/behavior_{n}.rs");
        let file = codefile(&store, &path);
        let edge = store
            .add_edge(
                EdgeKind::Implements,
                &intent.id,
                &file.id,
                TruthClass::Asserted,
            )
            .unwrap();
        store.set_agent(Agent::Lane(OwnerRole::Builder));
        store
            .assert_fact(
                Assertion::new(
                    Subject::Edge(edge.id.clone()),
                    Claim::Verdict,
                    "passing",
                    "llm:builder",
                )
                .criterion("the behavior is realized by this file")
                .confidence(0.95)
                .cited(
                    loom::evidence::cite(
                        tmp.path(),
                        &format!("{path}:1 — implementation evidence"),
                    )
                    .unwrap(),
                ),
            )
            .unwrap();
        edge_files.push((edge.id, path));
    }
    store.set_agent(Agent::Lane(OwnerRole::Analyzer));
    let initial = loom::review::pending(&store).unwrap();
    let frontier: Vec<_> = initial
        .iter()
        .filter(|candidate| candidate.variant == loom::review::ReviewVariant::Adversarial)
        .map(|candidate| candidate.edge.id.clone())
        .collect();
    assert_eq!(frontier.len(), 5);

    for edge_id in &frontier {
        let path = edge_files
            .iter()
            .find(|(id, _)| id == edge_id)
            .unwrap()
            .1
            .clone();
        loom::review::record(
            &store,
            loom::review::ChallengeAttempt {
                edge: edge_id,
                outcome: loom::review::ChallengeOutcome::Inconclusive,
                hypothesis: "an unmodeled boundary case contradicts the grounding",
                evidence: &format!("{path}:1 — inspected the grounding but could not decide"),
                impact: None,
                confidence: 0.6,
            },
        )
        .unwrap();
    }
    assert!(loom::review::pending(&store).unwrap().is_empty());
    let sixth = edge_files
        .iter()
        .find(|(edge, _)| !frontier.contains(edge))
        .unwrap();
    assert!(loom::review::current_challenge(&store, &sixth.0)
        .unwrap()
        .is_none());
    assert_eq!(loom::review::summary(&store).unwrap().inconclusive, 5);
}

#[test]
fn same_profile_is_recorded_as_a_non_blocking_warning() {
    let tmp = Tmp::new();
    let edge_id = one_green_edge(&tmp);
    let recorded = json_driver(
        tmp.path(),
        "analyzer",
        "builder-a",
        &[
            "challenge",
            "record",
            &edge_id,
            "inconclusive",
            "--hypothesis",
            "a bypass exists outside the inspected branch",
            "--evidence",
            "src/authorization.rs:1 — inspected the only registered grounding",
        ],
    );
    assert_eq!(recorded["independence_warning"], "challenge_same_profile");
    assert_eq!(recorded["graph_state"]["review_independence_warnings"], 1);

    let audit = json_driver(tmp.path(), "", "", &["audit"]);
    assert!(audit["warnings"].as_array().unwrap().iter().any(|warning| {
        warning["code"] == "challenge_same_profile" && warning["blocking"] == false
    }));
}
