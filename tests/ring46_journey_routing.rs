//! Ring 46 — Journey is the routing root.
//!
//! The ladder, singular packet, roster, readiness projection, and Intent
//! completeness axis all read the same hash-bound Journey facts.

use loom::completeness;
use loom::lane::{LadderInputs, Lane};
use loom::model::{EdgeKind, InspectionStatus, Node, NodeType, TargetKind, TruthClass};
use loom::store::Store;
use loom::workitem;
mod common;
use common::*;

fn authored_journey(store: &Store, root: &std::path::Path, id: &str) -> Node {
    let artifact = format!("journeys/{id}.yaml");
    std::fs::create_dir_all(root.join("journeys")).unwrap();
    std::fs::write(
        root.join(&artifact),
        format!("schema: loom.journey/v1\nid: {id}\nname: Complete the flow\nactor: user\ngoal: A user completes the flow\ninputs: {{}}\npreconditions: []\nsteps:\n  - id: choose\n    name: Choose\n    action: Choose an option\n    expects: []\n    produces: {{}}\n  - id: confirm\n    name: Confirm\n    action: Confirm the option\n    expects:\n      - The flow is completed\n    produces:\n      receipt:\n        type: string\nprofiles:\n  proof:\n    inputs: {{}}\n    workspace: {{}}\n"),
    )
    .unwrap();
    store
        .add_node(
            NodeType::Journey,
            id,
            "a user completes the flow",
            "authored",
            serde_json::json!({
                "schema": "loom.journey/v1",
                "stable_id": id,
                "name": "Complete the flow",
                "actor": "user",
                "goal": "a user completes the flow",
                "artifact": artifact,
                "semantic_hash": format!("hash-{id}"),
                "input_ids": [],
                "preconditions": [],
                "step_ids": ["choose", "confirm"],
                "output_ids": ["receipt"],
                "profile_ids": ["proof"],
            }),
        )
        .unwrap()
}

fn intent(store: &Store, name: &str, lifecycle: &str) -> Node {
    store
        .add_node(
            NodeType::Intent,
            name,
            "the technical behavior is falsifiable",
            lifecycle,
            serde_json::json!({}),
        )
        .unwrap()
}

fn derive(store: &Store, journey: &Node, intent: &Node, step_ids: &[&str]) -> loom::model::Edge {
    let edge = store
        .add_edge(
            EdgeKind::Derives,
            &journey.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &edge.id,
            TargetKind::Edge,
            "journey_hash",
            journey.body["semantic_hash"].as_str().unwrap(),
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &edge.id,
            TargetKind::Edge,
            "step_ids",
            &serde_json::to_string(step_ids).unwrap(),
            TruthClass::Asserted,
        )
        .unwrap();
    edge
}

fn ratify_and_ground(store: &Store, intent: &Node, path: &str) -> Node {
    store
        .ratify_intent(&intent.id, "the fixture human wants this", "test fixture")
        .unwrap();
    let code = codefile(store, path);
    store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &code.id,
            TruthClass::Asserted,
        )
        .unwrap();
    code
}

fn add_surface(store: &Store, journey: &Node) -> (Node, loom::model::Edge) {
    let interface = store
        .add_node(
            NodeType::InterfaceSurface,
            "flow-cli",
            "the real target-repository CLI",
            "active",
            serde_json::json!({
                "schema": "loom.interface-surface/v1",
                "stable_id": "flow-cli",
                "title": "Flow CLI",
                "kind": "cli",
                "identity": "flow",
                "operations": [
                    {"id":"choose-op", "summary":"choose", "argv":["flow","choose"], "arguments":[], "output":{"format":"json"}},
                    {"id":"confirm-op", "summary":"confirm", "argv":["flow","confirm"], "arguments":[], "output":{"format":"json"}}
                ],
            }),
        )
        .unwrap();
    let edge = store
        .add_edge(
            EdgeKind::Surfaces,
            &journey.id,
            &interface.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &edge.id,
            TargetKind::Edge,
            "journey_hash",
            journey.body["semantic_hash"].as_str().unwrap(),
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &edge.id,
            TargetKind::Edge,
            "operation_bindings",
            r#"[{"operation_id":"choose-op","step_id":"choose"},{"operation_id":"confirm-op","step_id":"confirm"}]"#,
            TruthClass::Asserted,
        )
        .unwrap();
    (interface, edge)
}

fn earn_s3(store: &Store, journey: &Node) {
    let surface_hash = loom::journey::surface_projection_hash(store, journey)
        .unwrap()
        .expect("surface projection exists");
    let validation = store
        .add_node(
            NodeType::Validation,
            "journey:flow:proof",
            "compiled Journey proof",
            "passed",
            serde_json::json!({
                "type":"journey",
                "profile":"proof",
                "journey_hash": journey.body["semantic_hash"].as_str().unwrap(),
                "surface_hash": surface_hash,
                "compiler_version": loom::journey::JOURNEY_COMPILER_VERSION,
            }),
        )
        .unwrap();
    store
        .set_facet(
            &validation.id,
            TargetKind::Node,
            "proof_strength",
            &serde_json::to_string(&loom::proofstrength::StrengthWitness {
                grade: "S3".into(),
                ran_and_passed: true,
                content_assertions: 1,
                call_witness: Some("flow_cli::main -> behavior".into()),
                next: "raise the boundary".into(),
                ..Default::default()
            })
            .unwrap(),
            TruthClass::Derived,
        )
        .unwrap();
    let proves = store
        .add_edge(
            EdgeKind::Proves,
            &validation.id,
            &journey.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &proves.id,
            InspectionStatus::Passing,
            "compiled proof reaches the surfaced CLI",
            "src/flow_cli.rs:1",
            0.99,
            "validator",
        )
        .unwrap();
}

#[test]
fn ladder_prefix_and_observed_disable_are_journey_rooted() {
    assert_eq!(
        &Lane::LADDER[..7],
        &[
            Lane::Seed,
            Lane::Fix,
            Lane::Derive,
            Lane::Build,
            Lane::Surface,
            Lane::Coverage,
            Lane::Validate,
        ]
    );
    assert_eq!(Lane::Derive.rung(), "derived");
    assert_eq!(Lane::Surface.rung(), "surfaced");
    assert_eq!(Lane::parse("derive"), Some(Lane::Derive));
    assert_eq!(Lane::parse("surface"), Some(Lane::Surface));
    assert_eq!(Lane::Derive.axis(), loom::truth::TruthAxis::Intent);
    assert_eq!(Lane::Surface.axis(), loom::truth::TruthAxis::Implementation);

    let owned = LadderInputs {
        authored_journeys: 1,
        active: 2,
        derive_gaps: 3,
        surface_gaps: 2,
        ..Default::default()
    };
    assert_eq!(Lane::Seed.depth(&owned), 0);
    assert_eq!(Lane::Derive.depth(&owned), 3);
    assert_eq!(Lane::Surface.depth(&owned), 2);
    let observed = LadderInputs {
        observed: true,
        ..owned
    };
    assert_eq!(Lane::Derive.depth(&observed), 0);
    assert_eq!(Lane::Surface.depth(&observed), 0);

    let no_root = LadderInputs {
        active: 8,
        ..Default::default()
    };
    assert_eq!(Lane::Seed.depth(&no_root), 1);
}

#[test]
fn derive_queue_covers_unmapped_stale_and_unrooted_non_exempt_work() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let journey = authored_journey(&store, tmp.path(), "flow");
    let mapped = intent(&store, "mapped behavior", "planned");
    let stale = derive(&store, &journey, &mapped, &["choose"]);
    store
        .set_facet(
            &stale.id,
            TargetKind::Edge,
            "journey_hash",
            "old-hash",
            TruthClass::Asserted,
        )
        .unwrap();
    let current = intent(&store, "currently derived behavior", "planned");
    derive(&store, &journey, &current, &["confirm"]);
    let unrooted = intent(&store, "unrooted behavior", "planned");
    let orphan = intent(&store, "unrelated orphan behavior", "planned");
    store
        .add_edge(
            EdgeKind::Relates,
            &unrooted.id,
            &current.id,
            TruthClass::Asserted,
        )
        .unwrap();
    let exempt = intent(&store, "deliberately rootless utility", "implemented");
    store
        .set_facet(
            &exempt.id,
            TargetKind::Node,
            "journey_exemption",
            r#"{"human_decision_digest":"sha256:abc","kind":"infrastructure","reason":"not user-reachable"}"#,
            TruthClass::Asserted,
        )
        .unwrap();

    let gaps = completeness::journey_derive_gaps(&store).unwrap();
    assert!(gaps.iter().any(|gap| gap.kind == "unmapped_step"));
    assert!(gaps.iter().any(|gap| gap.kind == "stale_derivation"));
    assert!(gaps
        .iter()
        .any(|gap| gap.kind == "unrooted_intent" && gap.subject_id == unrooted.id));
    assert!(
        !gaps.iter().any(|gap| gap.subject_id == orphan.id),
        "an Intent with no derived relationship neighbor must not pin a false host"
    );
    assert!(!gaps.iter().any(|gap| gap.subject_id == exempt.id));

    let roster = workitem::queue_items(&store, Lane::Derive).unwrap();
    assert_eq!(roster.len(), gaps.len());
    let item = workitem::next(&store, Some(Lane::Derive))
        .unwrap()
        .expect("derive gap is servable");
    assert_eq!(item.target.id, journey.id);
    assert_eq!(item.owner_role, "builder");
    assert!(item
        .prompt_contract
        .write_back
        .contains("loom journey derive-accept"));
    assert!(item.prompt_contract.write_back.contains(&journey.id));
}

#[test]
fn malformed_or_noncanonical_exemption_never_hides_unrooted_work() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    authored_journey(&store, tmp.path(), "flow");
    let subject = intent(&store, "utility", "implemented");
    for invalid in [
        r#"{"kind":"infra","reason":"because"}"#,
        r#"{ "human_decision_digest":"x","kind":"infra","reason":"because" }"#,
        r#"{"human_decision_digest":"","kind":"infra","reason":"because"}"#,
    ] {
        store
            .set_facet(
                &subject.id,
                TargetKind::Node,
                "journey_exemption",
                invalid,
                TruthClass::Asserted,
            )
            .unwrap();
        assert!(!completeness::intent_journey_exempt(&store, &subject.id).unwrap());
    }
}

#[test]
fn surface_waits_for_current_ratified_realizing_derivations() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let journey = authored_journey(&store, tmp.path(), "flow");
    let derived = intent(&store, "flow is executed", "planned");
    derive(&store, &journey, &derived, &["choose", "confirm"]);

    assert!(completeness::journey_surface_gaps(&store)
        .unwrap()
        .is_empty());
    store
        .ratify_intent(&derived.id, "the fixture human wants this", "test fixture")
        .unwrap();
    assert!(completeness::journey_surface_gaps(&store)
        .unwrap()
        .is_empty());
    store.set_node_status(&derived.id, "implemented").unwrap();
    assert!(completeness::journey_surface_gaps(&store)
        .unwrap()
        .is_empty());
    let code = codefile(&store, "src/behavior.rs");
    store
        .add_edge(
            EdgeKind::Implements,
            &derived.id,
            &code.id,
            TruthClass::Asserted,
        )
        .unwrap();

    let gaps = completeness::journey_surface_gaps(&store).unwrap();
    assert_eq!(gaps.len(), 1);
    let item = workitem::next(&store, Some(Lane::Surface))
        .unwrap()
        .expect("eligible Journey is surface work");
    assert_eq!(item.target.id, journey.id);
    assert!(item
        .prompt_contract
        .write_back
        .contains("loom journey surface-accept"));

    let (interface, _) = add_surface(&store, &journey);
    let surface_code = codefile(&store, "src/flow_cli.rs");
    store
        .add_edge(
            EdgeKind::Exposes,
            &interface.id,
            &surface_code.id,
            TruthClass::Asserted,
        )
        .unwrap();
    assert!(completeness::journey_surface_gaps(&store)
        .unwrap()
        .is_empty());
}

#[test]
fn readiness_and_validate_progress_to_realized_s3() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let journey = authored_journey(&store, tmp.path(), "flow");
    let derived = intent(&store, "flow is executed", "implemented");
    derive(&store, &journey, &derived, &["choose", "confirm"]);
    ratify_and_ground(&store, &derived, "src/behavior.rs");

    let r = completeness::journey_readiness(&store, &journey).unwrap();
    assert!(r.authored && r.derived && r.implemented);
    assert!(!r.surfaced && !r.compiled && !r.proven && !r.realized);

    let (interface, _) = add_surface(&store, &journey);
    let r = completeness::journey_readiness(&store, &journey).unwrap();
    assert!(!r.surfaced);
    assert!(!r.compiled);
    let surface_code = codefile(&store, "src/flow_cli.rs");
    store
        .add_edge(
            EdgeKind::Exposes,
            &interface.id,
            &surface_code.id,
            TruthClass::Asserted,
        )
        .unwrap();
    let r = completeness::journey_readiness(&store, &journey).unwrap();
    assert!(r.surfaced && !r.compiled && !r.proven && !r.realized);

    let validate = workitem::next(&store, Some(Lane::Validate))
        .unwrap()
        .expect("compiled unproven Journey is validation work");
    assert_eq!(validate.target.id, journey.id);
    assert!(validate
        .prompt_contract
        .write_back
        .contains("loom journey compile"));
    assert!(validate
        .prompt_contract
        .write_back
        .contains("loom journey run"));

    let compiled = store
        .add_node(
            NodeType::Validation,
            "journey:flow:proof",
            "compiler-owned Journey proof",
            "not_run",
            serde_json::json!({"type":"journey", "profile":"proof"}),
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Proves,
            &compiled.id,
            &journey.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Validates,
            &compiled.id,
            &derived.id,
            TruthClass::Asserted,
        )
        .unwrap();
    let compiled_packet = workitem::next(&store, Some(Lane::Validate))
        .unwrap()
        .expect("compiler-owned Journey validation is servable");
    assert_eq!(compiled_packet.target.id, journey.id);
    assert_eq!(
        compiled_packet.prompt_contract.write_back,
        format!(
            "loom journey compile '{}' --profile 'proof'; loom journey run '{}' --profile 'proof'",
            journey.id, journey.id
        )
    );
    assert!(!compiled_packet
        .prompt_contract
        .write_back
        .contains("validation verdict"));
    let roster = workitem::queue_items(&store, Lane::Validate).unwrap();
    assert_eq!(roster[0].target.id, journey.id);
    assert_eq!(
        Lane::Validate.depth(&LadderInputs::gather(&store).unwrap()),
        roster.len()
    );

    earn_s3(&store, &journey);
    let r = completeness::journey_readiness(&store, &journey).unwrap();
    assert!(r.proven && r.realized);
}

#[test]
fn validate_depth_roster_and_packet_share_profile_bearing_work_units() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let mut journey = authored_journey(&store, tmp.path(), "flow");
    journey.body["profile_ids"] = serde_json::json!(["proof", "smoke"]);
    store.set_node_body(&journey.id, &journey.body).unwrap();

    let intents = [
        intent(&store, "first visible behavior", "implemented"),
        intent(&store, "second visible behavior", "implemented"),
    ];
    for (index, target) in intents.iter().enumerate() {
        store
            .set_facet(
                &target.id,
                TargetKind::Node,
                "visibility",
                "user_visible",
                TruthClass::Asserted,
            )
            .unwrap();
        derive(&store, &journey, target, &["choose", "confirm"]);
        ratify_and_ground(&store, target, &format!("src/behavior_{index}.rs"));
    }
    let (surface, _) = add_surface(&store, &journey);
    let surface_code = codefile(&store, "src/flow_cli.rs");
    store
        .add_edge(
            EdgeKind::Exposes,
            &surface.id,
            &surface_code.id,
            TruthClass::Asserted,
        )
        .unwrap();

    let mut expected_commands = Vec::new();
    for profile in ["proof", "smoke"] {
        let validation = store
            .add_node(
                NodeType::Validation,
                &format!("journey:flow:{profile}"),
                "compiler-owned Journey proof",
                "not_run",
                serde_json::json!({"type":"journey", "profile":profile}),
            )
            .unwrap();
        store
            .add_edge(
                EdgeKind::Proves,
                &validation.id,
                &journey.id,
                TruthClass::Asserted,
            )
            .unwrap();
        for target in &intents {
            store
                .add_edge(
                    EdgeKind::Validates,
                    &validation.id,
                    &target.id,
                    TruthClass::Asserted,
                )
                .unwrap();
        }
        expected_commands.push(format!(
            "loom journey run '{}' --profile '{}'",
            journey.id, profile
        ));
    }

    let roster = workitem::queue_items(&store, Lane::Validate).unwrap();
    let depth = Lane::Validate.depth(&LadderInputs::gather(&store).unwrap());
    assert_eq!(depth, roster.len());
    assert_eq!(roster.len(), 4, "two profiles plus two unproven Intents");
    for command in &expected_commands {
        assert!(
            roster.iter().any(|entry| entry.reason.contains(command)),
            "roster omitted exact profile command {command}"
        );
    }

    let packet = workitem::next(&store, Some(Lane::Validate))
        .unwrap()
        .expect("first shared work unit is servable");
    assert!(expected_commands
        .iter()
        .any(|command| packet.prompt_contract.write_back.contains(command)));
}

#[test]
fn compiler_owned_journey_validation_rejects_manual_verdict() {
    let tmp = Tmp::new();
    let (
        validation_id,
        intent_id,
        proves_id,
        validates_id,
        surface_id,
        codefile_id,
        successor_codefile_id,
    ) = {
        let store = Store::init(tmp.path(), Some("t"), false).unwrap();
        let journey = authored_journey(&store, tmp.path(), "flow");
        let target = intent(&store, "compiled behavior", "implemented");
        let (surface, _) = add_surface(&store, &journey);
        let successor = codefile(&store, "src/behavior_v2.rs");
        let codefile = codefile(&store, "src/behavior.rs");
        let validation = store
            .add_node(
                NodeType::Validation,
                "journey:flow:proof",
                "compiler-owned Journey proof",
                "not_run",
                serde_json::json!({"type":"journey", "profile":"proof"}),
            )
            .unwrap();
        let proves = store
            .add_edge(
                EdgeKind::Proves,
                &validation.id,
                &journey.id,
                TruthClass::Asserted,
            )
            .unwrap();
        let validates = store
            .add_edge(
                EdgeKind::Validates,
                &validation.id,
                &target.id,
                TruthClass::Asserted,
            )
            .unwrap();
        store
            .add_edge(
                EdgeKind::Exercises,
                &validation.id,
                &codefile.id,
                TruthClass::Asserted,
            )
            .unwrap();
        (
            validation.id,
            target.id,
            proves.id,
            validates.id,
            surface.id,
            codefile.id,
            successor.id,
        )
    };

    for cmd in [
        loom::cli::ValidationCmd::Verdict {
            key: validation_id.clone(),
            outcome: "passed".into(),
            evidence: "not a runtime observation".into(),
            reason: String::new(),
        },
        loom::cli::ValidationCmd::Update {
            key: validation_id.clone(),
            r#type: Some("manual_check".into()),
            command: None,
        },
        loom::cli::ValidationCmd::Run {
            key: validation_id.clone(),
            all: false,
        },
        loom::cli::ValidationCmd::Unlink {
            validation: validation_id.clone(),
            intent: intent_id,
        },
        loom::cli::ValidationCmd::Remove {
            key: validation_id.clone(),
        },
    ] {
        let rejected = loom::commands::run(loom::cli::Cli {
            graph: Some(tmp.path().to_path_buf()),
            json: true,
            command: Some(loom::cli::Command::Validation { cmd }),
        })
        .expect_err("generic mutation of compiler-owned Journey validation must fail");
        assert!(
            format!("{rejected:#}").contains("compiler-owned Journey"),
            "unexpected rejection: {rejected:#}"
        );
    }

    for command in [
        loom::cli::Command::Edge {
            cmd: loom::cli::EdgeCmd::Remove {
                edge_id: proves_id,
                reason: Some("attempted generic removal".into()),
            },
        },
        loom::cli::Command::Edge {
            cmd: loom::cli::EdgeCmd::Verdict {
                edge_id: validates_id,
                verdict: "ground".into(),
                criterion: "generic mutation is forbidden".into(),
                evidence: "src/behavior.rs:1".into(),
                confidence: 1.0,
            },
        },
        loom::cli::Command::Edge {
            cmd: loom::cli::EdgeCmd::Call {
                validation: validation_id.clone(),
                surface: surface_id,
            },
        },
        loom::cli::Command::Edge {
            cmd: loom::cli::EdgeCmd::Exercises {
                validation: validation_id.clone(),
                codefile: codefile_id.clone(),
                locator: None,
            },
        },
    ] {
        let rejected = loom::commands::run(loom::cli::Cli {
            graph: Some(tmp.path().to_path_buf()),
            json: true,
            command: Some(command),
        })
        .expect_err("generic edge mutation of compiler-owned topology must fail");
        assert!(
            format!("{rejected:#}").contains("compiler-owned Journey"),
            "unexpected rejection: {rejected:#}"
        );
    }

    let rejected = loom::commands::run(loom::cli::Cli {
        graph: Some(tmp.path().to_path_buf()),
        json: true,
        command: Some(loom::cli::Command::Codefile {
            cmd: loom::cli::CodefileCmd::Remove {
                key: codefile_id.clone(),
                successor: Some(successor_codefile_id),
            },
        }),
    })
    .expect_err("codefile cascade must not retarget compiler-owned Exercises topology");
    assert!(
        format!("{rejected:#}").contains("compiler-owned Journey"),
        "unexpected rejection: {rejected:#}"
    );

    let store = Store::open(tmp.path()).unwrap();
    let unchanged = store.get_node(&validation_id).unwrap().unwrap();
    assert_eq!(unchanged.body["type"], "journey");
    assert_eq!(unchanged.status, "not_run");
    assert_eq!(
        store
            .edges_with(Some(EdgeKind::Validates), Some(&validation_id), None)
            .unwrap()
            .len(),
        1
    );
    assert!(store.get_node(&codefile_id).unwrap().is_some());
    assert_eq!(
        store
            .edges_with(
                Some(EdgeKind::Exercises),
                Some(&validation_id),
                Some(&codefile_id),
            )
            .unwrap()
            .len(),
        1
    );
}

/// The exact Grid state: a compiler-owned Journey proof whose Proves and
/// Validates edges settled from a passing run, and whose `calls` edge did not.
fn passed_proof_with_uninspected_calls(
    store: &Store,
    root: &std::path::Path,
) -> (Node, loom::model::Edge) {
    let journey = authored_journey(store, root, "flow");
    let target = intent(store, "compiled behavior", "implemented");
    let (surface, _) = add_surface(store, &journey);
    codefile(store, "src/flow_cli.rs");

    let validation = store
        .add_node(
            NodeType::Validation,
            "journey:flow:proof",
            "compiler-owned Journey proof",
            "passed",
            serde_json::json!({"type":"journey", "profile":"proof"}),
        )
        .unwrap();
    for (kind, to) in [
        (EdgeKind::Proves, &journey.id),
        (EdgeKind::Validates, &target.id),
    ] {
        let edge = store
            .add_edge(kind, &validation.id, to, TruthClass::Asserted)
            .unwrap();
        store
            .record_verdict(
                &edge.id,
                InspectionStatus::Passing,
                "compiled proof observed",
                "src/flow_cli.rs:1",
                0.99,
                "validator",
            )
            .unwrap();
    }
    let calls = store
        .add_edge(
            EdgeKind::Calls,
            &validation.id,
            &surface.id,
            TruthClass::Asserted,
        )
        .unwrap();
    (journey, calls)
}

#[test]
fn compiler_owned_proof_topology_routes_to_validate_never_analyze() {
    // The serve path must agree with the generic-mutation cut. A `calls` edge
    // out of a compiler-owned Journey Validation is proof topology only
    // `journey compile/run` can inspect, so Analyze — whose write-back is
    // `loom edge verdict`, which that cut rejects — must not serve it. The
    // validate lane owns it and names the run. An ordinary `calls` edge from a
    // hand-authored Validation stays analyze work.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let (journey, compiled_calls) = passed_proof_with_uninspected_calls(&store, tmp.path());
    let surface = store.get_node(&compiled_calls.to_id).unwrap().unwrap();

    let hand_authored = store
        .add_node(
            NodeType::Validation,
            "hand-authored-cli-check",
            "an ordinary validation nobody compiled",
            "not_run",
            serde_json::json!({"type":"command", "command":"flow --help"}),
        )
        .unwrap();
    let generic_calls = store
        .add_edge(
            EdgeKind::Calls,
            &hand_authored.id,
            &surface.id,
            TruthClass::Asserted,
        )
        .unwrap();

    let analyze = workitem::queue_items(&store, Lane::Analyze).unwrap();
    assert!(
        analyze
            .iter()
            .all(|entry| entry.target.id != compiled_calls.id),
        "analyze must not enumerate compiler-owned proof topology: {analyze:#?}"
    );
    assert!(
        analyze
            .iter()
            .any(|entry| entry.target.id == generic_calls.id),
        "an ordinary calls edge is still an analyze claim: {analyze:#?}"
    );
    assert_eq!(
        Lane::Analyze.depth(&LadderInputs::gather(&store).unwrap()),
        analyze.len(),
        "the relationships rung must count exactly what the analyze queue serves"
    );

    // Which uninspected claim the picker reaches first is id-ordered, so only
    // the exclusion is asserted here; the roster above proves the ordinary
    // calls edge is still analyze work.
    let served = workitem::next(&store, Some(Lane::Analyze))
        .unwrap()
        .expect("analyze has servable work");
    assert_ne!(served.target.id, compiled_calls.id);

    let validate = workitem::next(&store, Some(Lane::Validate))
        .unwrap()
        .expect("the uninspected compiler-owned calls edge is validate work");
    assert_eq!(validate.target.id, journey.id);
    assert!(
        validate.prompt_contract.write_back.contains(&format!(
            "loom journey run '{}' --profile 'proof'",
            journey.id
        )),
        "validate must name the only legal door: {}",
        validate.prompt_contract.write_back
    );

    // With no ordinary calls claim left, `--mode analyze` moves on to the rest
    // of its lane. It neither serves the compiler-owned edge nor refuses with
    // an unservable-packet defect — the packet simply is not its work.
    store.delete_edge(&generic_calls.id).unwrap();
    let residue = workitem::next(&store, Some(Lane::Analyze))
        .expect("analyze must not refuse the lane over compiler-owned topology");
    assert!(
        residue
            .as_ref()
            .is_none_or(|item| item.target.id != compiled_calls.id),
        "analyze served the edge only journey run can inspect: {residue:#?}"
    );
    assert!(workitem::queue_items(&store, Lane::Analyze)
        .unwrap()
        .iter()
        .all(|entry| entry.target.id != compiled_calls.id),);
    assert_eq!(
        store
            .get_edge(&compiled_calls.id)
            .unwrap()
            .expect("the calls edge survives")
            .status,
        InspectionStatus::Uninspected,
        "routing must not fake a verdict on the edge it declines to serve"
    );
}

#[test]
fn failing_compiler_owned_proof_edge_names_the_rerun_not_sync_alone() {
    // A failed run marks the same closure `failing`, which is fix-lane work.
    // Sync cannot re-measure compiler-owned topology, so the fixer packet must
    // name the re-run as well, not send the worker to a door that never closes.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let (journey, compiled_calls) = passed_proof_with_uninspected_calls(&store, tmp.path());
    store
        .record_verdict(
            &compiled_calls.id,
            InspectionStatus::Failing,
            "the compiled profile failed against its surface",
            "src/flow_cli.rs:1",
            0.99,
            "validator",
        )
        .unwrap();

    let fix = workitem::next(&store, Some(Lane::Fix))
        .unwrap()
        .expect("a failing proof edge is fix work");
    assert_eq!(fix.target.id, compiled_calls.id);
    let rerun = format!("loom journey run '{}' --profile 'proof'", journey.id);
    assert!(
        fix.prompt_contract.write_back.contains(&rerun),
        "the fixer must be told sync alone cannot re-measure this claim: {}",
        fix.prompt_contract.write_back
    );
    assert!(
        fix.prompt_contract
            .allowed_actions
            .iter()
            .any(|action| action.contains(&rerun)),
        "the re-run must be an allowed fixer action: {:#?}",
        fix.prompt_contract.allowed_actions
    );
}

#[test]
fn commandless_non_manual_validation_routes_to_configuration_not_verdict() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let target = intent(&store, "observable contract", "implemented");
    let validation = store
        .add_node(
            NodeType::Validation,
            "unconfigured contract",
            "must be made executable",
            "not_run",
            serde_json::json!({"type":"contract", "command":""}),
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Validates,
            &validation.id,
            &target.id,
            TruthClass::Asserted,
        )
        .unwrap();

    let packet = workitem::next(&store, Some(Lane::Validate))
        .unwrap()
        .expect("unconfigured validation is servable");
    assert!(packet
        .prompt_contract
        .write_back
        .contains("loom validation update 'unconfigured contract' --command"));
    assert!(packet
        .prompt_contract
        .write_back
        .contains("loom validation run 'unconfigured contract'"));
    assert!(!packet
        .prompt_contract
        .write_back
        .contains("validation verdict"));
}

#[test]
fn intrinsic_human_binding_completes_readiness_without_becoming_a_cli_witness() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let journey = authored_journey(&store, tmp.path(), "flow");
    let derived = intent(&store, "flow is executed", "implemented");
    derive(&store, &journey, &derived, &["choose", "confirm"]);
    ratify_and_ground(&store, &derived, "src/behavior.rs");

    let (interface, surfaces) = add_surface(&store, &journey);
    store
        .set_facet(
            &surfaces.id,
            TargetKind::Edge,
            "operation_bindings",
            r#"[{"operation_id":"choose-op","step_id":"choose"},{"human_decision":{"operation_id":"choose-op","pointer":"/work_item"},"step_id":"confirm"}]"#,
            TruthClass::Asserted,
        )
        .unwrap();
    let surface_code = codefile(&store, "src/flow_cli.rs");
    store
        .add_edge(
            EdgeKind::Exposes,
            &interface.id,
            &surface_code.id,
            TruthClass::Asserted,
        )
        .unwrap();

    let readiness = completeness::journey_readiness(&store, &journey).unwrap();
    assert!(readiness.surfaced, "{:?}", readiness.surface_gaps);
    assert!(readiness.surface_gaps.is_empty());
    assert!(completeness::journey_surface_gaps(&store)
        .unwrap()
        .is_empty());
    assert!(store
        .edges_with(Some(EdgeKind::Calls), None, None)
        .unwrap()
        .is_empty());
    assert!(store
        .edges_with(Some(EdgeKind::Exercises), None, None)
        .unwrap()
        .is_empty());

    store
        .set_facet(
            &surfaces.id,
            TargetKind::Edge,
            "operation_bindings",
            r#"[{"operation_id":"choose-op","step_id":"choose"},{"human_decision":{"operation_id":"missing-op","pointer":"/work_item"},"step_id":"confirm"}]"#,
            TruthClass::Asserted,
        )
        .unwrap();
    let rejected = completeness::journey_readiness(&store, &journey).unwrap();
    assert!(!rejected.surfaced);
    assert!(rejected
        .surface_gaps
        .iter()
        .any(|gap| gap.contains("canonical complete operation bindings")));
}

#[test]
fn intent_keeps_six_axes_and_journey_requires_realized_root() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let journey = authored_journey(&store, tmp.path(), "flow");
    let derived = intent(&store, "flow is executed", "implemented");
    store
        .set_facet(
            &derived.id,
            TargetKind::Node,
            "visibility",
            "user_visible",
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &derived.id,
            TargetKind::Node,
            "level",
            "feature",
            TruthClass::Asserted,
        )
        .unwrap();
    derive(&store, &journey, &derived, &["choose", "confirm"]);
    ratify_and_ground(&store, &derived, "src/behavior.rs");

    let before = completeness::scorecard(&store, &derived).unwrap();
    assert_eq!(before.axes.len(), 6);
    assert_eq!(
        before
            .axes
            .iter()
            .find(|axis| axis.axis == "journey")
            .unwrap()
            .state,
        "open"
    );

    let (interface, _) = add_surface(&store, &journey);
    let surface_code = codefile(&store, "src/flow_cli.rs");
    store
        .add_edge(
            EdgeKind::Exposes,
            &interface.id,
            &surface_code.id,
            TruthClass::Asserted,
        )
        .unwrap();
    earn_s3(&store, &journey);

    let after = completeness::scorecard(&store, &derived).unwrap();
    assert_eq!(after.axes.len(), 6);
    assert_eq!(
        after
            .axes
            .iter()
            .find(|axis| axis.axis == "journey")
            .unwrap()
            .state,
        "met"
    );
}

#[test]
fn observed_graph_serves_neither_derive_nor_surface() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), true).unwrap();
    authored_journey(&store, tmp.path(), "flow");
    intent(&store, "unrooted", "planned");
    assert!(workitem::queue_items(&store, Lane::Derive)
        .unwrap()
        .is_empty());
    assert!(workitem::queue_items(&store, Lane::Surface)
        .unwrap()
        .is_empty());
    assert!(workitem::next(&store, Some(Lane::Derive))
        .unwrap()
        .is_none());
    assert!(workitem::next(&store, Some(Lane::Surface))
        .unwrap()
        .is_none());
}
