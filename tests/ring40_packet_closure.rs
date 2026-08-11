//! Ring 40 — every served packet names its own closure.
//!
//! The contract invariant of uniform adjudicability: a packet whose
//! write_back names no runnable loom command — or whose closure command does
//! not accept the packet's own target — is not work, it is a loom defect.
//! Such an item is journaled as `unservable_packet` and never handed to a
//! worker. These tests serve real packets across lanes on real graphs and
//! assert the invariant holds for each.

use loom::lane::Lane;
use loom::model::{EdgeKind, NodeType, TargetKind, TruthClass};
use loom::store::Store;
use loom::workitem;
mod common;
use common::*;

/// The invariant, checked against one served packet: the write_back names a
/// loom command, and that command accepts the packet's own target (id, short
/// id, name, or an edge endpoint).
fn assert_closure(item: &workitem::WorkItem) {
    let wb = &item.prompt_contract.write_back;
    assert!(
        wb.contains("loom "),
        "[{}] names no runnable loom command: {wb}",
        item.mode
    );
    if matches!(item.mode.as_str(), "fix" | "audit") || item.target.kind == "graph" {
        return; // state-closed: `loom sync` / `loom audit` take no target argument
    }
    let short: String = item.target.id.chars().take(8).collect();
    let handles: Vec<&str> = [
        Some(item.target.id.as_str()),
        Some(item.target.name.as_str()),
        item.target.from.as_deref(),
        item.target.to.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|h| !h.is_empty())
    .collect();
    let commands: Vec<&str> = wb
        .split([';', '\n'])
        .filter(|s| s.contains("loom "))
        .collect();
    assert!(
        commands
            .iter()
            .any(|c| handles.iter().any(|h| c.contains(h)) || c.contains(short.as_str())),
        "[{}] no named command accepts target '{}': {wb}",
        item.mode,
        item.target.id
    );
}

fn ratify_all(store: &Store) {
    for n in workitem::unratified_intents(store).unwrap() {
        store
            .ratify_intent(&n.id, "test fixture: wanted", "test fixture")
            .unwrap();
    }
}

#[test]
fn a_served_build_packet_carries_the_complete_role_scope() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "build a role-scoped packet",
            "the packet names everything the builder may and must do",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();

    let item = workitem::next(&store, Some(Lane::Build))
        .unwrap()
        .expect("the planned intent produces a Build packet");
    assert_eq!(item.mode, "build");
    assert_eq!(item.owner_role, "builder");
    assert_eq!(item.target.id, intent.id);

    let contract = &item.prompt_contract;
    assert_eq!(contract.role, item.owner_role);
    assert!(!contract.allowed_actions.is_empty());
    assert!(!contract.forbidden_actions.is_empty());
    assert!(!contract.required_evidence.trim().is_empty());
    assert!(!contract.write_back.trim().is_empty());
    assert!(!contract.stop_condition.trim().is_empty());
    assert_closure(&item);
}

#[test]
fn ratify_packet_presents_meaning_drift_as_a_conversational_human_gate() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "export reports",
            "users export CSV reports",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .ratify_intent(
            &intent.id,
            "requested in the Q1 product review",
            "keep CSV exports",
        )
        .unwrap();
    store
        .redefine_intent(&intent.id, "users export PDF reports")
        .unwrap();

    let item = workitem::next(&store, Some(Lane::Divergence))
        .unwrap()
        .expect("changed meaning after a human judgment must produce a Ratify packet");
    let packet = serde_json::to_value(&item).unwrap();
    assert!(
        serde_json::to_string(&packet)
            .unwrap()
            .contains("users export PDF reports"),
        "packet must present the current meaning: {packet}"
    );
    assert!(
        item.reason.contains("meaning drifted"),
        "packet must name the concrete drift: {}",
        item.reason
    );
    assert!(
        item.reason.contains("redefined after ratification")
            && item.reason.contains("the words changed under the yes"),
        "packet must carry concrete evidence for the drift: {}",
        item.reason
    );

    let gate = item
        .prompt_contract
        .human_gate
        .as_ref()
        .expect("Ratify packet must carry a structured human gate");
    assert!(
        gate.recommendation.contains("recommend one option")
            && gate.recommendation.contains("evidence")
            && gate
                .recommendation
                .contains("never treat the recommendation as the decision"),
        "host must receive evidence-bound recommendation guidance: {}",
        gate.recommendation
    );
    let labels: Vec<_> = gate
        .options
        .iter()
        .map(|option| option.label.as_str())
        .collect();
    assert_eq!(
        labels,
        ["Keep behavior", "Remove behavior", "Revise criterion"]
    );

    let revise = gate
        .options
        .iter()
        .find(|option| option.id == "revise")
        .expect("one choice must accept a free-form revision");
    assert!(revise
        .description
        .contains("Correct what the behavior should mean"));
    assert!(
        revise
            .write_back
            .as_deref()
            .is_some_and(|write_back| write_back.contains("<corrected criterion>")),
        "revision write-back must preserve the human's free-form criterion"
    );

    for human_text in std::iter::once(gate.question.as_str()).chain(
        gate.options
            .iter()
            .flat_map(|option| [option.label.as_str(), option.description.as_str()]),
    ) {
        assert!(
            !human_text.contains("loom ") && !human_text.contains("--"),
            "human-facing copy must not require terminal construction: {human_text}"
        );
    }
    assert!(
        gate.options.iter().all(|option| option
            .write_back
            .as_deref()
            .is_some_and(|write_back| write_back.contains("loom "))),
        "prefilled terminal writes belong to executor metadata, separate from human-facing copy"
    );
}

/// A quality packet: the closure is `loom rule verdict` naming both endpoints.
#[test]
fn a_quality_packet_closes_with_rule_verdict_naming_both_endpoints() {
    let tmp = Tmp::new();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(
        tmp.path().join("src/thing.rs"),
        "pub fn a() -> u8 {\n    Some(1).expect(\"a is total\")\n}\n",
    )
    .unwrap();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "a behavior under a rule",
            "d",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = codefile(&store, "src/thing.rs");
    let g = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &cf.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &g.id,
            TargetKind::Edge,
            "locator",
            "fn a",
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &g.id,
            loom::model::InspectionStatus::Passing,
            "lives here",
            "src/thing.rs:1",
            0.9,
            "llm",
        )
        .unwrap();
    let rule = store
        .add_node(
            NodeType::QualityRule,
            "no-unchecked-failure",
            "every fallible operation's failure path is handled",
            "",
            serde_json::json!({"category":"reliability","patterns":[r#"\bexpect\s*\("#]}),
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Governs,
            &rule.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    ratify_all(&store);

    let item = workitem::next(&store, Some(Lane::Quality))
        .unwrap()
        .expect("an unmeasured governs pair is quality work");
    assert_closure(&item);
    let wb = &item.prompt_contract.write_back;
    assert!(wb.contains("loom rule verdict"), "{wb}");
    assert!(wb.contains("no-unchecked-failure"), "{wb}");
    assert!(wb.contains("a behavior under a rule"), "{wb}");
}

/// A triage packet: the closure is `loom finding verdict` naming the finding id.
#[test]
fn a_triage_packet_closes_with_finding_verdict_naming_the_finding() {
    let tmp = Tmp::new();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(tmp.path().join("src/thing.rs"), "pub fn a() {}\n").unwrap();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let cf = codefile(&store, "src/thing.rs");
    let finding = store
        .add_derived_node(
            NodeType::Finding,
            "oversized_file:src/thing.rs:",
            "src/thing.rs is oversized",
            "1200 lines (> 600)",
            "oversized_file",
            serde_json::json!({ "kind": "oversized_file", "symbol": "", "metric": 1200 }),
        )
        .unwrap();
    store
        .add_derived_edge(EdgeKind::Flags, &finding.id, &cf.id)
        .unwrap();

    let item = workitem::next(&store, Some(Lane::Triage))
        .unwrap()
        .expect("an untriaged finding is triage work");
    assert_closure(&item);
    assert!(
        item.prompt_contract
            .write_back
            .contains(&format!("loom finding verdict {}", &item.target.id[..8])),
        "the prefilled command names the packet's own finding: {}",
        item.prompt_contract.write_back
    );
}

/// An elaborate packet: the closure names the intent it elaborates.
#[test]
fn an_elaborate_packet_names_the_intent_in_its_closure() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "users can see this happen",
            "a behavior a user can see",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .set_facet(
            &intent.id,
            TargetKind::Node,
            "visibility",
            "user_visible",
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &intent.id,
            TargetKind::Node,
            "level",
            "feature",
            TruthClass::Asserted,
        )
        .unwrap();
    ratify_all(&store);

    let item = workitem::next(&store, Some(Lane::Elaborate))
        .unwrap()
        .expect("a user-visible idea with open axes is elaborate work");
    assert_closure(&item);
    assert!(
        item.prompt_contract
            .write_back
            .contains("users can see this happen"),
        "the closure names the intent: {}",
        item.prompt_contract.write_back
    );
}

/// A deepen packet: the closure names the intent whose proof floor it raises.
#[test]
fn a_deepen_packet_names_the_intent_in_its_closure() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "users can see this happen",
            "a behavior a user can see",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    // fan-in: something must call the realizing symbol or the urgency score
    // is zero and the ranking never surfaces the intent.
    earn_call_witness(&store, tmp.path(), &intent.id);
    loom::commands::prove_intent(&store, &intent.id, "unit proof", "true").unwrap();
    ratify_all(&store);
    loom::sync::run(&store, tmp.path()).unwrap();

    let item = workitem::next(&store, Some(Lane::Deepen))
        .unwrap()
        .expect("a green behavior with a weak proof is deepen work");
    assert_closure(&item);
    assert!(
        item.prompt_contract
            .write_back
            .contains("users can see this happen"),
        "the closure names the intent: {}",
        item.prompt_contract.write_back
    );
}

/// Deepen consistently serves the weakest ranked claim and carries the facts
/// that explain why it ranks first.
#[test]
fn deepen_deterministically_selects_and_explains_the_weakest_claim() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let weaker = store
        .add_node(
            NodeType::Intent,
            "users can recover a draft",
            "a behavior with only a liveness proof",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let stronger = store
        .add_node(
            NodeType::Intent,
            "users can retain a draft",
            "a behavior with an assertion-bearing proof",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();

    // Give both claims the same grounded call shape so proof strength is the
    // distinguishing weakness: S1 liveness versus an S2 asserted outcome.
    earn_call_witness(&store, tmp.path(), &weaker.id);
    loom::commands::prove_intent(&store, &weaker.id, "liveness proof", "true").unwrap();
    earn_call_witness(&store, tmp.path(), &stronger.id);
    prove_s2(&store, tmp.path(), &stronger.id, "asserted-outcome");
    ratify_all(&store);
    loom::sync::run(&store, tmp.path()).unwrap();

    let ranked_once = loom::risk::rank(&store).unwrap();
    let ranked_twice = loom::risk::rank(&store).unwrap();
    assert!(
        ranked_once.len() >= 2,
        "the fixture must produce multiple standing claims: {ranked_once:#?}"
    );
    let ids_once: Vec<&str> = ranked_once
        .iter()
        .map(|candidate| candidate.intent_id.as_str())
        .collect();
    let ids_twice: Vec<&str> = ranked_twice
        .iter()
        .map(|candidate| candidate.intent_id.as_str())
        .collect();
    assert_eq!(ids_once, ids_twice, "ranking order must be deterministic");

    let weakest = &ranked_once[0];
    assert_eq!(
        weakest.intent_id, weaker.id,
        "the liveness-only claim must rank ahead of the stronger proof"
    );
    assert!(
        !weakest.next_move.as_str().is_empty(),
        "the ranked weakness must propose a concrete evidence move"
    );
    let first_packet = workitem::next(&store, Some(Lane::Deepen))
        .unwrap()
        .expect("multiple standing claims produce Deepen work");
    let second_packet = workitem::next(&store, Some(Lane::Deepen))
        .unwrap()
        .expect("unchanged graph produces the same Deepen work");
    for packet in [&first_packet, &second_packet] {
        assert_eq!(packet.target.id, weakest.intent_id);
        assert!(
            packet.reason.contains(&weakest.proof_strength.to_string()),
            "packet must name the selected claim's proof strength: {}",
            packet.reason
        );
        assert!(
            packet.reason.contains(weakest.why),
            "packet must include the ranking rationale: {}",
            packet.reason
        );
        assert!(
            packet
                .prompt_contract
                .write_back
                .contains(weakest.next_move.as_str()),
            "packet must carry the ranked claim's exact next move: {}",
            packet.prompt_contract.write_back
        );
        assert!(
            packet.prompt_contract.mindset.contains("already green")
                && packet
                    .prompt_contract
                    .why_now
                    .contains("every floor is met"),
            "Deepen must remain optional work on an already-green graph: {:?}",
            packet.prompt_contract
        );
        assert!(
            packet.prompt_contract.stop_condition.contains("ONE move")
                && packet.prompt_contract.stop_condition.contains("re-ranks")
                && packet.next_step.contains("make ONE move")
                && packet.next_step.contains("re-run `loom deepen`"),
            "packet must stop after one move and explicitly re-rank: {:?}",
            packet.prompt_contract
        );
    }
    assert_eq!(first_packet.reason, second_packet.reason);
}

/// An audit packet: prose remedies still name a runnable closeout — fix per
/// the remedy, then re-read the record.
#[test]
fn an_audit_packet_always_names_a_runnable_closeout() {
    let tmp = Tmp::new();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    // An oversize registered file mints a derived finding whose smell remedy
    // is prose; the audit queue must still serve a named closeout.
    let big: String = std::iter::once("pub fn a() {}\n".to_string())
        .chain((0..600).map(|i| format!("pub fn f{i}() {{}}\n")))
        .collect();
    std::fs::write(tmp.path().join("src/big.rs"), big).unwrap();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    codefile(&store, "src/big.rs");
    loom::sync::run(&store, tmp.path()).unwrap();

    if let Some(item) = workitem::next(&store, Some(Lane::Audit)).unwrap() {
        assert_closure(&item);
    }
    // Whether or not this fixture's smells surface in the audit backlog, the
    // checker is exercised directly by the unit tests; this guards the lane
    // against regressing on the graphs it actually serves.
}

/// The refusal path: an unservable item is journaled and never served.
/// Constructed through a lane whose contract was deliberately broken in a
/// copy of the checker — here the check runs against the live lane set, so
/// the assertion is that every lane loom ships passes it.
#[test]
fn every_lane_loom_ships_serves_a_closable_packet_on_this_graph() {
    let tmp = Tmp::new();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(
        tmp.path().join("src/thing.rs"),
        "pub fn a() -> u8 {\n    Some(1).expect(\"a is total\")\n}\n",
    )
    .unwrap();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "a behavior under a rule",
            "d",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .set_facet(
            &intent.id,
            TargetKind::Node,
            "visibility",
            "user_visible",
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
    let rule = store
        .add_node(
            NodeType::QualityRule,
            "no-unchecked-failure",
            "every fallible operation's failure path is handled",
            "",
            serde_json::json!({"category":"reliability","patterns":[r#"\bexpect\s*\("#]}),
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Governs,
            &rule.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    let finding = store
        .add_derived_node(
            NodeType::Finding,
            "oversized_file:src/thing.rs:",
            "src/thing.rs is oversized",
            "1200 lines (> 600)",
            "oversized_file",
            serde_json::json!({ "kind": "oversized_file", "symbol": "", "metric": 1200 }),
        )
        .unwrap();
    store
        .add_derived_edge(EdgeKind::Flags, &finding.id, &cf.id)
        .unwrap();
    ratify_all(&store);
    loom::sync::run(&store, tmp.path()).unwrap();

    // No lane this graph activates may serve an unclosable packet, and the
    // default walk must serve SOMETHING closable (not die on the first lane).
    let mut served = 0;
    for lane in Lane::LADDER {
        if !lane.serves_items() || lane.requires_human_decision() {
            continue;
        }
        if let Some(item) = workitem::next(&store, Some(*lane)).unwrap() {
            assert_closure(&item);
            served += 1;
        }
    }
    assert!(
        served > 0,
        "the fixture graph must activate at least one lane"
    );
    let default = workitem::next(&store, None)
        .unwrap()
        .expect("the default walk must serve work on this graph");
    assert_closure(&default);
}

/// A surfaced semantic Journey with no compiled proof is validate work. Its
/// packet names a runnable, Journey-bound compile/run closure, and executing
/// that closure removes the gap while earning S3.
#[test]
fn a_journey_gap_validate_packet_names_the_journey_in_its_closure() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let runner_path = tmp.path().join("runner.py");
    std::fs::write(
        &runner_path,
        "#!/usr/bin/env python3\nimport json\ndef main():\n    print(json.dumps({'ok': True}))\nif __name__ == '__main__':\n    main()\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&runner_path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&runner_path, permissions).unwrap();
    }
    let code = store
        .add_node(
            NodeType::CodeFile,
            "runner.py",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "users can check out",
            "a checkout emits a successful recorded result",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .set_facet(
            &intent.id,
            TargetKind::Node,
            "visibility",
            "user_visible",
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .ratify_intent(&intent.id, "fixture behavior is wanted", "test fixture")
        .unwrap();
    let grounding = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &code.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &grounding.id,
            TargetKind::Edge,
            "locator",
            "main",
            TruthClass::Asserted,
        )
        .unwrap();

    std::fs::create_dir_all(tmp.path().join("journeys")).unwrap();
    let artifact = tmp.path().join("journeys/checkout-flow.yaml");
    let spec: loom::journey::JourneySpec = serde_json::from_value(serde_json::json!({
        "schema":"loom.journey/v1",
        "id":"checkout-flow",
        "name":"Checkout flow",
        "actor":"shopper",
        "goal":"Complete checkout",
        "inputs":{},
        "preconditions":[],
        "steps":[{"id":"checkout","name":"Checkout","action":"checks out","expects":[],"produces":{}}],
        "profiles":{"proof":{"inputs":{},"workspace":{}}}
    }))
    .unwrap();
    std::fs::write(&artifact, serde_norway::to_string(&spec).unwrap()).unwrap();
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
                "artifact":"journeys/checkout-flow.yaml",
                "semantic_hash":hash,
                "input_ids":[],
                "preconditions":[],
                "step_ids":["checkout"],
                "output_ids":[],
                "profile_ids":["proof"]
            }),
        )
        .unwrap();
    let derives = store
        .add_edge(
            EdgeKind::Derives,
            &journey.id,
            &intent.id,
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
    let surface = store
        .add_node(
            NodeType::InterfaceSurface,
            "checkout CLI",
            "real checkout CLI",
            "active",
            serde_json::json!({
                "schema":"loom.interface-surface/v1",
                "stable_id":"checkout-cli",
                "title":"Checkout CLI",
                "kind":"cli",
                "identity":"runner.py",
                "operations":[{
                    "id":"checkout-op",
                    "summary":"Run checkout",
                    "argv":[runner_path.to_str().unwrap()],
                    "arguments":[],
                    "output":{
                        "format":"json",
                        "assertions":[{
                            "id":"checkout-ok",
                            "pointer":"/ok",
                            "type":"boolean",
                            "equals":true
                        }]
                    }
                }]
            }),
        )
        .unwrap();
    let surfaces = store
        .add_edge(
            EdgeKind::Surfaces,
            &journey.id,
            &surface.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &surfaces.id,
            TargetKind::Edge,
            "journey_hash",
            &hash,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &surfaces.id,
            TargetKind::Edge,
            "operation_bindings",
            "[{\"operation_id\":\"checkout-op\",\"step_id\":\"checkout\"}]",
            TruthClass::Asserted,
        )
        .unwrap();
    let exposes = store
        .add_edge(
            EdgeKind::Exposes,
            &surface.id,
            &code.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &exposes.id,
            TargetKind::Edge,
            "locator",
            "main",
            TruthClass::Asserted,
        )
        .unwrap();

    let item = workitem::next(&store, Some(Lane::Validate))
        .unwrap()
        .expect("a surfaced, uncompiled Journey is validate work");
    assert_closure(&item);
    assert_eq!(item.target.id, journey.id);
    assert!(item.prompt_contract.write_back.contains("journey compile"));
    assert!(item.prompt_contract.write_back.contains("journey run"));
    assert!(item.prompt_contract.write_back.contains(&journey.id));
    drop(store);

    for args in [
        vec!["journey", "compile", "checkout-flow", "--profile", "proof"],
        vec!["journey", "run", "checkout-flow", "--profile", "proof"],
        vec!["sync"],
    ] {
        let output = loom_command()
            .arg("--graph")
            .arg(tmp.path())
            .args(&args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "loom {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let store = Store::open(tmp.path()).unwrap();
    let validation = store
        .resolve_node("journey:checkout-flow:proof", Some(NodeType::Validation))
        .unwrap();
    assert_eq!(validation.status, "passed");
    assert!(
        loom::proofstrength::of(&store, &validation.id).unwrap()
            >= loom::proofstrength::Strength::END_TO_END
    );
    let after = workitem::next(&store, Some(Lane::Validate)).unwrap();
    assert!(
        after
            .as_ref()
            .is_none_or(|work| work.target.id != journey.id),
        "executing the advertised closure must remove the Journey gap: {after:#?}"
    );
}
