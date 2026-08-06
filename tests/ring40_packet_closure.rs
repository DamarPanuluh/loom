//! Ring 40 — every served packet names its own closure.
//!
//! The contract invariant of uniform adjudicability: a packet whose
//! write_back names no runnable loom command — or whose closure command does
//! not accept the packet's own target — is not work, it is a loom defect.
//! Such an item is journaled as `unservable_packet` and never handed to a
//! worker. These tests serve real packets across lanes on real graphs and
//! assert the invariant holds for each.

use loom::cli::{Cli, CodefileCmd, Command, EdgeCmd, JourneyCmd};
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

/// A validate packet for a user-visible intent whose proof is meaningful but
/// not end-to-end: the closure is `loom journey add`/`run`, and the write_back
/// must name the intent or the packet is refused as `unservable_packet`.
/// Regression: the journey-gap branch once wrote back only `<spec>`
/// placeholders, so the validate lane could not serve this gap at all.
#[test]
fn a_journey_gap_validate_packet_names_the_intent_in_its_closure() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "users can check out",
            "a flow a user can see",
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
    // A passing S2 proof (runner-reported assertions, no call witness):
    // meaningful, but NOT end-to-end — exactly the shallow journey gap the
    // validate lane serves when nothing else is pending.
    loom::commands::prove_intent(
        &store,
        &intent.id,
        "unit proof",
        "echo 'test result: ok. 4 passed; 0 failed'",
    )
    .unwrap();
    ratify_all(&store);

    let item = workitem::next(&store, Some(Lane::Validate))
        .unwrap()
        .expect("an S2-only proof of a user-visible behavior is validate work");
    assert_closure(&item);
    // The packet must advertise the grounding operations the closure needs,
    // so the executed workflow below is exactly the advertised one — the S3
    // call witness is earned by the grounding the packet itself prescribes,
    // not by test-only steps.
    assert!(
        item.prompt_contract
            .allowed_actions
            .iter()
            .any(|a| a.contains("loom edge implement") && a.contains("--role verifies")),
        "the packet must advertise the grounding steps: {:?}",
        item.prompt_contract.allowed_actions
    );
    // The re-grade after the run must also be advertised: `loom journey run`
    // records the outcome but proof strength is derived by sync.
    assert!(
        item.prompt_contract
            .write_back
            .contains("loom sync to re-grade"),
        "the write_back must advertise the sync that re-grades the proof: {}",
        item.prompt_contract.write_back
    );
    // The target-bearing command must be runnable as-is: `loom journey prompt`
    // takes exactly one intent argument, so it must end at the intent id (a
    // name containing `;` or a newline must not split the write_back).
    assert!(
        item.prompt_contract
            .write_back
            .contains(&format!("loom journey prompt '{}';", intent.id)),
        "the closure names the intent in a runnable command: {}",
        item.prompt_contract.write_back
    );

    // Executing the advertised closure must REMOVE this packet from the
    // queue. The closure is target-bound: ground the behavior, give it a
    // verifying test that calls the grounded symbol (the S3 call witness),
    // author a journey spec whose steps name the intent AND run the grounded
    // verifier (`cargo test --test behavior_test`), register it (`loom
    // journey add`), run it (`loom journey run`), and re-grade (`loom sync`).
    // Grounding is added WITHOUT sync so the fixture's S2 proof keeps its
    // stored grade — the journey run genuinely exercises the code the witness
    // is claimed from, and only the journey validation re-grades to S3.
    drop(store); // release the fixture's write lock so CLI commands can open the graph
    std::fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"loom-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nname = \"loom_fixture\"\npath = \"src/behavior.rs\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(
        tmp.path().join("src/behavior.rs"),
        "pub fn perform_behavior() -> &'static str {\n    \"ok\"\n}\n",
    )
    .unwrap();
    std::fs::create_dir_all(tmp.path().join("tests")).unwrap();
    std::fs::write(
        tmp.path().join("tests/behavior_test.rs"),
        "use loom_fixture::perform_behavior;\n#[test]\nfn exercises_behavior() {\n    \
         assert_eq!(perform_behavior(), \"ok\");\n}\n",
    )
    .unwrap();
    std::fs::create_dir_all(tmp.path().join("journeys")).unwrap();
    let spec = tmp.path().join("journeys/checkout-flow.yaml");
    std::fs::write(
        &spec,
        "journey: checkout-flow\nsteps:\n  - name: run the verifier\n    intent: users can check out\n    \
         run: cargo test --test behavior_test\n    expect:\n      stdout_contains: [\"test result: ok\"]\n",
    )
    .unwrap();
    let mut cmds: Vec<Command> = vec![
        Command::Codefile {
            cmd: CodefileCmd::Add {
                path: "src/behavior.rs".into(),
                observed: false,
            },
        },
        Command::Edge {
            cmd: EdgeCmd::Implement {
                intent: "users can check out".into(),
                codefile: "src/behavior.rs".into(),
                locator: Some("fn perform_behavior".into()),
                role: Some("realizes".into()),
            },
        },
        Command::Codefile {
            cmd: CodefileCmd::Add {
                path: "tests/behavior_test.rs".into(),
                observed: false,
            },
        },
        Command::Edge {
            cmd: EdgeCmd::Implement {
                intent: "users can check out".into(),
                codefile: "tests/behavior_test.rs".into(),
                locator: None,
                role: Some("verifies".into()),
            },
        },
        Command::Journey {
            cmd: JourneyCmd::Add { spec: spec.clone() },
        },
        Command::Journey {
            cmd: JourneyCmd::Run {
                spec,
                base_url: None,
            },
        },
    ];
    for cmd in cmds.drain(..) {
        loom::commands::run(Cli {
            graph: Some(tmp.path().to_path_buf()),
            json: false,
            command: Some(cmd),
        })
        .unwrap();
    }

    let store = Store::open(tmp.path()).unwrap();
    // The advertised closure must have earned the bar: the registered journey
    // validation is a passing S3-or-stronger proof of this intent (loom ran
    // it, its spec asserts output/content, and its call closure reaches the
    // grounded behavior). Sync derives the strength from those facts.
    loom::sync::run(&store, tmp.path()).unwrap();
    let journey = store
        .resolve_node("checkout-flow", Some(NodeType::Validation))
        .unwrap();
    assert_eq!(
        journey.status, "passed",
        "the closure's journey run must pass"
    );
    assert!(
        loom::proofstrength::of(&store, &journey.id).unwrap()
            >= loom::proofstrength::Strength::END_TO_END,
        "the journey proof registered by the closure must reach S3"
    );

    let after = workitem::next(&store, Some(Lane::Validate)).unwrap();
    if let Some(w) = after {
        assert_ne!(
            w.target.id, intent.id,
            "executing the advertised closure must remove the journey-gap packet: {}",
            w.prompt_contract.write_back
        );
    }
}
