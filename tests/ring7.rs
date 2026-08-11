//! Ring 7 tests — the dogfood-milestone contract on a controlled graph:
//! coverage, deterministic export-check, clean doctor, zero open smells,
//! meaningful maturity, and a served work item — all end to end.

use loom::model::{EdgeKind, InspectionStatus, NodeType, TruthClass};
use loom::store::Store;
use loom::{maturity, signal, travel, workitem};
mod common;
use common::*;

/// Build a small, clean, fully-grounded graph (the dogfood shape in miniature).
fn build_clean_graph(tmp: &Tmp) -> Store {
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(
        tmp.path().join("src/auth.rs"),
        "pub fn login() { /* ok */ }\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("src/cart.rs"),
        "pub fn create() { /* ok */ }\n",
    )
    .unwrap();
    let store = Store::init(tmp.path(), Some("demo"), false).unwrap();

    let sys = store
        .add_node(
            NodeType::Intent,
            "demo app works end to end",
            "purpose",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let auth = store
        .add_node(
            NodeType::Intent,
            "user can log in",
            "session on valid creds",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cart = store
        .add_node(
            NodeType::Intent,
            "cart can be created",
            "an empty cart is created",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .add_edge(EdgeKind::Hierarchy, &sys.id, &auth.id, TruthClass::Asserted)
        .unwrap();
    store
        .add_edge(EdgeKind::Hierarchy, &sys.id, &cart.id, TruthClass::Asserted)
        .unwrap();
    store
        .set_facet(
            &sys.id,
            loom::model::TargetKind::Node,
            "journey_exemption",
            r#"{"human_decision_digest":"sha256:ring7","kind":"aggregate","reason":"hierarchy parent is proven through its Journey-rooted children"}"#,
            TruthClass::Asserted,
        )
        .unwrap();

    let fa = codefile(&store, "src/auth.rs");
    let fc = codefile(&store, "src/cart.rs");
    // ground each file to exactly one intent (no overlap, no tangle)
    let ea = store
        .add_edge(EdgeKind::Implements, &auth.id, &fa.id, TruthClass::Asserted)
        .unwrap();
    let ec = store
        .add_edge(EdgeKind::Implements, &cart.id, &fc.id, TruthClass::Asserted)
        .unwrap();
    // inspect the groundings so there is no asserted residue
    store
        .record_verdict(
            &ea.id,
            InspectionStatus::Passing,
            "login implemented",
            "src/auth.rs:1",
            0.95,
            "llm",
        )
        .unwrap();
    store
        .record_verdict(
            &ec.id,
            InspectionStatus::Passing,
            "cart create implemented",
            "src/cart.rs:1",
            0.95,
            "llm",
        )
        .unwrap();
    // inspect the hierarchy edges too
    for e in store
        .edges_with(Some(EdgeKind::Hierarchy), Some(&sys.id), None)
        .unwrap()
    {
        store
            .record_verdict(
                &e.id,
                InspectionStatus::Passing,
                "decomposition holds",
                "hierarchy",
                0.95,
                "llm",
            )
            .unwrap();
    }
    // Each implemented LEAF needs a proof that reaches S2 — `proven` does not
    // accept liveness. A bare `prove_intent` with `true` lands at S1: loom ran
    // a command and it exited zero, which says nothing about the behavior. So
    // this fixture, which asserts the graph is CLEAN, has to hold proofs that
    // actually assert something. (`sys` is a hierarchy parent — proven through
    // its children.)
    for (intent, slug) in [(&auth, "login"), (&cart, "cart")] {
        s3_journey_proof(&store, tmp.path(), &intent.id, slug);
    }
    // arm duplicate detection with distinct vocab tags (no collisions)
    for (id, term) in [(&sys.id, "system"), (&auth.id, "auth"), (&cart.id, "cart")] {
        store.add_vocab_term(term, "demo plane").unwrap();
        store
            .set_tag(id, loom::model::TargetKind::Node, term)
            .unwrap();
    }
    loom::sync::run(&store, tmp.path()).unwrap();
    store
}

#[test]
fn dogfood_graph_is_clean_and_exportable() {
    let tmp = Tmp::new();
    let store = build_clean_graph(&tmp);

    // doctor: clean
    assert!(
        signal::doctor(&store).unwrap().is_empty(),
        "doctor must be clean"
    );

    // smells: zero open (no tangle/overlap; small grounded graph)
    let smells = signal::smells(&store).unwrap();
    assert!(
        smells.is_empty(),
        "expected zero open smells, got: {smells:?}"
    );

    // export is deterministic and, once written, fresh
    let path = travel::export_to_file(&store).unwrap();
    assert!(path.exists());
    assert!(
        travel::export_is_fresh(&store).unwrap(),
        "export must be fresh right after writing"
    );
}

#[test]
fn dogfood_maturity_is_meaningful_not_seed() {
    let tmp = Tmp::new();
    let store = build_clean_graph(&tmp);
    let ladder = maturity::ladder(&store).unwrap();
    assert_ne!(
        ladder.phase, "seed",
        "a populated graph must not be in the seed phase"
    );
    // seeded + realized + hardened all met on a clean grounded graph
    let met = |name: &str| {
        ladder
            .rungs
            .iter()
            .find(|r| r.name == name)
            .map(|r| r.state == maturity::RungState::Met)
            .unwrap_or(false)
    };
    assert!(met("seeded"));
    assert!(met("grounded"));
    assert!(met("covered"), "{ladder:#?}");
    assert!(
        met("inspected") && met("measured"),
        "no asserted residue should leave the verdict rungs met"
    );
}

#[test]
fn dogfood_export_check_detects_drift() {
    let tmp = Tmp::new();
    let store = build_clean_graph(&tmp);
    travel::export_to_file(&store).unwrap();
    assert!(travel::export_is_fresh(&store).unwrap());
    // mutate the graph → committed export is now stale
    store
        .add_node(
            NodeType::Intent,
            "new behavior",
            "",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    assert!(
        !travel::export_is_fresh(&store).unwrap(),
        "export --check must detect drift"
    );
}

#[test]
fn release_workflow_keeps_tests_as_proof_artifacts_and_hash_binds_semantics() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let dogfood = std::fs::read_to_string(root.join("scripts/dogfood.sh")).unwrap();
    let test_gate = dogfood
        .find("cargo test --all-targets --quiet")
        .expect("dogfood must execute the complete test gate");
    let graph_gate = dogfood
        .find("== fresh v12 graph ==")
        .expect("dogfood must create the fresh graph after code gates");
    assert!(
        test_gate < graph_gate,
        "tests must execute before fresh-graph discovery"
    );

    let reason = "Tests are Validation/proof artifacts, not implementation ownership; literal test paths may be re-registered when an Exercises edge needs source-drift tracking.";
    assert!(
        dogfood.contains(reason),
        "the exact approved policy is recorded"
    );
    assert!(
        dogfood.contains("ignore add 'tests/**'"),
        "fresh graphs must persist the coverage exclusion"
    );
    assert!(
        !dogfood.contains("'tests/**/*.rs'"),
        "blanket source discovery must not register tests as implementation CodeFiles"
    );

    let journey = loom::journey::parse(&root.join("journeys/release-workflow.yaml"))
        .expect("release workflow must remain a strict semantic Journey");
    assert_eq!(journey.semantic_hash().unwrap(), "8cd6742023f60b62");
}

#[test]
fn dogfood_next_serves_work_until_clean() {
    let tmp = Tmp::new();
    let store = build_clean_graph(&tmp);
    // A clean graph has no required residue. The only remaining packet is the
    // deliberately-open terminal deepen lane, which is optional strengthening.
    let pending = workitem::next(&store, None).unwrap();
    assert_eq!(
        pending.as_ref().map(|item| item.mode.as_str()),
        Some("deepen"),
        "clean graph may offer only optional strengthening: {pending:#?}"
    );
    // Introducing an unrooted planned intent creates required derivation work
    // before implementation can begin. The packet must name that exact intent.
    let planned = store
        .add_node(
            NodeType::Intent,
            "checkout works",
            "",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    let derive = workitem::next(&store, None).unwrap().unwrap();
    assert_eq!(derive.mode, "derive");
    assert!(
        derive
            .context
            .linked_entities
            .iter()
            .any(|entity| entity.id == planned.id && entity.role == "unrooted_intent"),
        "derive packet must carry the exact unrooted intent: {derive:#?}"
    );
}

/// Every command a packet offers must resolve.
///
/// The fix packet told workers to run `loom intent show <name>` where the name
/// came from the wrong endpoint: `validates` runs validation→intent, so `from`
/// is the validation, and the suggested command could never resolve. A packet
/// that hands out commands which fail is worse than one that hands out none —
/// it costs the reader a round trip to discover the tool was wrong, not them.
#[test]
fn fix_packet_names_the_intent_not_the_other_endpoint() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            loom::model::NodeType::Intent,
            "a behavior",
            "d",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = codefile(&store, "src/thing.rs");
    store
        .add_edge(
            loom::model::EdgeKind::Implements,
            &intent.id,
            &cf.id,
            loom::model::TruthClass::Asserted,
        )
        .unwrap();
    // A failing PROOF: validates runs validation → intent.
    loom::commands::prove_intent(&store, &intent.id, "the proof", "false").unwrap();

    let item = loom::workitem::next(&store, Some(loom::lane::Lane::Fix))
        .unwrap()
        .expect("a failing proof routes to fix");
    let suggested: Vec<&String> = item
        .prompt_contract
        .allowed_actions
        .iter()
        .filter(|a| a.starts_with("loom intent "))
        .collect();
    assert!(!suggested.is_empty(), "the packet offers intent commands");
    for action in suggested {
        // The TARGET is the first quoted argument; prose after it may mention
        // anything (the retire line warns about inconvenient proofs).
        let target = action
            .split('\'')
            .nth(1)
            .unwrap_or_else(|| panic!("an intent command quotes its target: {action}"));
        assert_eq!(
            target, "a behavior",
            "an intent command must name the INTENT endpoint, not the validation: {action}"
        );
    }
}

/// Retiring a behavior clears the claims about it.
///
/// The fix packet sanctions `loom intent retire` when code was deliberately
/// removed — so following that advice must actually move the ladder. It did
/// not: `live_edges_by_status` treated "live" as "not superseded", so a
/// retired intent's failing proof kept gating every rung above `repaired`, and
/// the operator who did exactly what the tool asked saw nothing happen.
#[test]
fn retiring_a_behavior_stops_its_claims_counting_as_debt() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            loom::model::NodeType::Intent,
            "a behavior that will be removed",
            "d",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = codefile(&store, "src/gone.rs");
    store
        .add_edge(
            loom::model::EdgeKind::Implements,
            &intent.id,
            &cf.id,
            loom::model::TruthClass::Asserted,
        )
        .unwrap();
    loom::commands::prove_intent(&store, &intent.id, "its proof", "false").unwrap();

    let before = loom::maturity::ladder(&store).unwrap();
    let repaired =
        |l: &loom::maturity::Ladder| l.rungs.iter().find(|r| r.name == "repaired").unwrap().state;
    assert_eq!(
        repaired(&before),
        loom::maturity::RungState::Unmet,
        "a failing proof gates fix"
    );

    store
        .retire_intent(&intent.id, "the capability was deleted on purpose", None)
        .unwrap();

    let after = loom::maturity::ladder(&store).unwrap();
    // Met or NotApplicable — either says "this no longer blocks". What must not
    // survive the retirement is Unmet.
    assert_ne!(
        repaired(&after),
        loom::maturity::RungState::Unmet,
        "a claim about a retired behavior is history, not debt"
    );
}

/// The proof TALLY and the proof GATE must agree about retired behavior.
///
/// After the edge counts learned to skip retired claims, the validation node
/// summary still counted them — so `loom status` reported "1 failed" for a
/// capability that had been deliberately deleted, sending the reader after a
/// repair that does not exist. Two counters describing the same thing is how a
/// display starts contradicting its own gate.
#[test]
fn a_retired_behaviors_proof_leaves_the_tally_too() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            loom::model::NodeType::Intent,
            "a behavior to remove",
            "d",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    loom::commands::prove_intent(&store, &intent.id, "its proof", "false").unwrap();
    assert_eq!(
        loom::maturity::validation_summary(&store).unwrap().failed,
        1
    );

    store
        .retire_intent(&intent.id, "deleted on purpose", None)
        .unwrap();
    let after = loom::maturity::validation_summary(&store).unwrap();
    assert_eq!(after.failed, 0, "a retired behavior owes no proof");
    assert_eq!(after.registered, 0, "and is not registered debt either");
}

/// A packet must never hand out a command the shell will split.
///
/// Stored proof commands can contain `&&`. Pasted after `--`, the calling
/// shell splits them: the wrapper sees only the first clause, records a proof
/// for THAT, and leaves the edge unrun. The queue then serves the same item
/// forever while every run reports success — fourteen iterations moved nothing
/// and every one of them printed PASSED.
#[test]
fn a_shell_shaped_command_is_not_offered_bare_after_a_double_dash() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            loom::model::NodeType::Intent,
            "a behavior with a compound proof",
            "d",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = codefile(&store, "src/thing.rs");
    store
        .add_edge(
            loom::model::EdgeKind::Implements,
            &intent.id,
            &cf.id,
            loom::model::TruthClass::Asserted,
        )
        .unwrap();
    let val = store
        .add_node(
            loom::model::NodeType::Validation,
            "compound proof",
            "",
            "not_run",
            serde_json::json!({ "type": "test", "command": "true && true" }),
        )
        .unwrap();
    store
        .ensure_edge(loom::model::EdgeKind::Validates, &val.id, &intent.id)
        .unwrap();

    let item = loom::workitem::next(&store, Some(loom::lane::Lane::Validate))
        .unwrap()
        .expect("an unrun proof routes to validate");
    for action in &item.prompt_contract.allowed_actions {
        if let Some(tail) = action.split_once(" -- ").map(|(_, t)| t) {
            assert!(
                !tail.contains("&&") || tail.starts_with("sh -c "),
                "a compound command after `--` is split by the shell: {action}"
            );
        }
    }
    // And the offer that runs the stored command exactly must be present.
    assert!(
        item.prompt_contract
            .allowed_actions
            .iter()
            .any(|a| a.starts_with("loom validation run ")),
        "offer the form that executes the stored command verbatim: {:?}",
        item.prompt_contract.allowed_actions
    );
}

/// A packet never offers an action the write boundary will refuse.
///
/// The validate packet listed `loom validation verdict … passed --evidence`
/// for a RUNNABLE proof, where the floor demands a Run. A worker following the
/// packet literally is refused — the tool contradicting itself, and costing a
/// round trip to find out.
#[test]
fn a_runnable_proof_is_never_offered_a_hand_written_verdict() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            loom::model::NodeType::Intent,
            "a behavior with a runnable proof",
            "d",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = codefile(&store, "src/thing.rs");
    store
        .add_edge(
            loom::model::EdgeKind::Implements,
            &intent.id,
            &cf.id,
            loom::model::TruthClass::Asserted,
        )
        .unwrap();
    let val = store
        .add_node(
            loom::model::NodeType::Validation,
            "runnable",
            "",
            "not_run",
            serde_json::json!({ "type": "test", "command": "true" }),
        )
        .unwrap();
    store
        .ensure_edge(loom::model::EdgeKind::Validates, &val.id, &intent.id)
        .unwrap();

    let item = loom::workitem::next(&store, Some(loom::lane::Lane::Validate))
        .unwrap()
        .expect("an unrun proof routes to validate");
    for action in &item.prompt_contract.allowed_actions {
        assert!(
            !action.contains("validation verdict"),
            "a runnable proof must be RUN, not verdicted by hand: {action}"
        );
    }
    // And the offer that reaches `verified` is present.
    assert!(
        item.prompt_contract
            .allowed_actions
            .iter()
            .any(|a| a.contains("loom observe") || a.contains("loom validation run")),
        "offer the route that actually reaches the floor: {:?}",
        item.prompt_contract.allowed_actions
    );
}

/// A passing S1 proof stays open everywhere until its shape earns S2.
///
/// This is the CLI-smoke regression: the command really ran and passed, so the
/// packet must not say "run it" and completeness must not call the proof met.
#[test]
fn passing_liveness_proof_is_routed_to_strengthen_then_clears_at_s2() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(tmp.path().join("src/thing.rs"), "pub fn behavior() {}\n").unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "implemented behavior needs meaningful proof",
            "observable behavior",
            "implemented",
            serde_json::json!({ "level": "feature" }),
        )
        .unwrap();
    store
        .set_facet(
            &intent.id,
            loom::model::TargetKind::Node,
            "level",
            "feature",
            TruthClass::Asserted,
        )
        .unwrap();
    let cf = codefile(&store, "src/thing.rs");
    store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &cf.id,
            TruthClass::Asserted,
        )
        .unwrap();
    let proof = store
        .add_node(
            NodeType::Validation,
            "weak grep proof",
            "",
            "not_run",
            serde_json::json!({ "type": "test", "command": "grep -q behavior src/thing.rs" }),
        )
        .unwrap();
    store
        .ensure_edge(EdgeKind::Validates, &proof.id, &intent.id)
        .unwrap();
    loom::commands::observe_validation(&store, &proof).unwrap();
    loom::sync::run(&store, tmp.path()).unwrap();

    assert_eq!(
        loom::proofstrength::of(&store, &proof.id).unwrap(),
        loom::proofstrength::Strength::S1
    );
    let item = workitem::next(&store, Some(loom::lane::Lane::Validate))
        .unwrap()
        .expect("weak passing proof remains in validate");
    assert!(item.reason.contains("ran and passed"), "{}", item.reason);
    assert!(item.reason.contains("S1"), "{}", item.reason);
    assert!(item.reason.contains("S2"), "{}", item.reason);
    assert!(
        item.prompt_contract.why_now.contains("S1") && item.prompt_contract.why_now.contains("S2"),
        "{}",
        item.prompt_contract.why_now
    );
    assert!(
        item.prompt_contract
            .write_back
            .contains("validation update")
            && item.prompt_contract.write_back.contains("validation run"),
        "{}",
        item.prompt_contract.write_back
    );
    assert!(
        item.prompt_contract.stop_condition.contains("S2")
            && item.prompt_contract.stop_condition.contains("meaningful"),
        "{}",
        item.prompt_contract.stop_condition
    );
    let card = loom::completeness::scorecard(&store, &intent).unwrap();
    let proof_axis = card.axes.iter().find(|axis| axis.axis == "proof").unwrap();
    assert_eq!(proof_axis.state, "open");
    assert!(
        proof_axis.detail.contains("S1") && proof_axis.detail.contains("S2"),
        "{}",
        proof_axis.detail
    );

    let mut body = proof.body.clone();
    body["command"] = serde_json::json!("printf 'test result: ok. 1 passed; 0 failed\\n'");
    store.set_node_body(&proof.id, &body).unwrap();
    let updated = store.get_node(&proof.id).unwrap().unwrap();
    loom::commands::observe_validation(&store, &updated).unwrap();
    loom::sync::run(&store, tmp.path()).unwrap();

    assert_eq!(
        loom::proofstrength::of(&store, &proof.id).unwrap(),
        loom::proofstrength::Strength::S2
    );
    assert!(
        loom::proofstrength::assess(&store, &intent.id)
            .unwrap()
            .meaningful_passing,
        "S2 clears the validate proof gate"
    );
    let card = loom::completeness::scorecard(&store, &intent).unwrap();
    let proof_axis = card.axes.iter().find(|axis| axis.axis == "proof").unwrap();
    assert_eq!(proof_axis.state, "met");
    assert!(proof_axis.detail.contains("S2"), "{}", proof_axis.detail);
}
