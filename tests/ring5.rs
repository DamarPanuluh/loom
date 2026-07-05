//! Ring 5 tests — quality, validation, hypothesis, journey model, vocab/layer.

use loom::cli::{
    Cli, CodefileCmd, Command, EdgeCmd, HypothesisCmd, IntentCmd, JourneyCmd, ValidationCmd,
    WikiCmd,
};
use loom::model::{EdgeKind, InspectionStatus, NodeType, TargetKind, TruthClass};
use loom::store::Store;
use loom::workitem::{self, Mode};
use std::path::Path;

mod common;
use common::*;

// Intentionally separate from ring6's binary-spawning `run_cli`: these tests
// exercise the in-process dispatcher with an already-parsed `Command`.
fn run(graph: &Path, command: Command) {
    let debug_command = format!("{command:?}");
    loom::commands::run(Cli {
        graph: Some(graph.to_path_buf()),
        json: false,
        command,
    })
    .unwrap_or_else(|e| panic!("command {debug_command} failed: {e}"));
}

// ---- quality packs + verdicts ----------------------------------------------

#[test]
fn pack_seed_is_idempotent() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let n1 = loom::packs::seed(&store, "iso5055").unwrap();
    let after1 = store
        .list_nodes(Some(NodeType::QualityRule), usize::MAX)
        .unwrap()
        .len();
    loom::packs::seed(&store, "iso5055").unwrap();
    let after2 = store
        .list_nodes(Some(NodeType::QualityRule), usize::MAX)
        .unwrap()
        .len();
    assert_eq!(n1, after1);
    assert_eq!(after1, after2, "re-seeding must not duplicate rules");
}

#[test]
fn quality_queue_serves_then_clears_on_verdict() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    loom::packs::seed(&store, "iso5055").unwrap();
    let rule = store
        .resolve_node(
            "iso5055-sec-no-hardcoded-secrets",
            Some(NodeType::QualityRule),
        )
        .unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "config loads secrets from env",
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let gov = store
        .ensure_edge(EdgeKind::Governs, &rule.id, &intent.id)
        .unwrap();

    // quality queue serves the uninspected governs edge with the rule's guide
    let item = workitem::next(&store, Some(Mode::Quality))
        .unwrap()
        .unwrap();
    assert_eq!(item.owner_role, "quality");
    assert!(
        item.prompt_contract.mindset.contains("inspection guide")
            || !item.prompt_contract.allowed_actions.is_empty()
    );

    store
        .record_verdict(
            &gov.id,
            InspectionStatus::Passing,
            "secrets from env",
            "src/config.rs:1",
            0.95,
            "llm",
        )
        .unwrap();

    // The settled edge leaves the queue, but seeding created 4 sibling rules
    // that were never measured against this root intent: the queue proposes
    // the first unmeasured pair instead of going quiet.
    let pair = workitem::next(&store, Some(Mode::Quality))
        .unwrap()
        .expect("unmeasured rule×intent pairs are open quality work");
    assert_eq!(pair.target.kind, "rule_intent_pair");
    assert_ne!(pair.target.from.as_deref(), Some(rule.name.as_str()));
    assert!(
        pair.prompt_contract
            .write_back
            .contains("loom rule verdict"),
        "pair packet must carry the exact verdict command"
    );

    // Measuring every seeded rule against the intent truly clears the queue.
    for r in loom::packs::pack("iso5055") {
        let rn = store
            .resolve_node(r.name, Some(NodeType::QualityRule))
            .unwrap();
        let e = store
            .ensure_edge(EdgeKind::Governs, &rn.id, &intent.id)
            .unwrap();
        if e.status != InspectionStatus::Passing {
            store
                .record_verdict(
                    &e.id,
                    InspectionStatus::Independent,
                    "rule surface absent here",
                    "inspected: no such surface in the grounded code",
                    0.9,
                    "llm",
                )
                .unwrap();
        }
    }
    assert!(workitem::next(&store, Some(Mode::Quality))
        .unwrap()
        .is_none());
}

// ---- validations -----------------------------------------------------------

#[test]
fn validate_runs_command_and_records_result() {
    let tmp = Tmp::new();
    run(
        tmp.path(),
        Command::Init {
            path: Some(tmp.path().to_path_buf()),
            name: Some("t".into()),
            observed: false,
        },
    );
    run(
        tmp.path(),
        Command::Intent {
            cmd: IntentCmd::Add {
                name: "always passes".into(),
                description: "demo".into(),
                level: "feature".into(),
                lifecycle: "implemented".into(),
                visibility: None,
                layer: None,
                aspect: None,
                allow_symbol_name: false,
            },
        },
    );
    run(
        tmp.path(),
        Command::Validation {
            cmd: ValidationCmd::Add {
                name: "true-proof".into(),
                r#type: "test".into(),
                command: "true".into(),
                intent: "always passes".into(),
                proof_level: None,
                proof_kind: None,
                journey_id: None,
                repo_native_kind: None,
                artifact: None,
            },
        },
    );
    run(
        tmp.path(),
        Command::Validation {
            cmd: ValidationCmd::Run {
                intent: "always passes".into(),
                all: false,
            },
        },
    );

    let store = Store::open(tmp.path()).unwrap();
    let v = store
        .resolve_node("true-proof", Some(NodeType::Validation))
        .unwrap();
    assert_eq!(v.status, "passed");
    // the validates edge is passing
    let edges = store
        .edges_with(Some(EdgeKind::Validates), Some(&v.id), None)
        .unwrap();
    assert_eq!(edges[0].status, InspectionStatus::Passing);
}

/// Contract: `loom validation run --json` must record a failed row carrying bounded
/// command output — `output.stdout`/`stderr` excerpts, byte counts, and
/// truncation flags — and the persisted Validates-edge evidence must include
/// an excerpt string, not just the exit code. Drives the compiled binary for
/// the JSON step (in-process `run()` prints JSON to the test process's stdout,
/// not a capturable buffer), then reads the stored edge back through the Store
/// API. The failing command emits deterministic stdout AND stderr so both
/// streams are exercisable; validate_cmd prefers the stderr excerpt for the
/// evidence when stderr is non-empty.
#[test]
fn validate_failing_command_records_failure() {
    let tmp = Tmp::new();
    run(
        tmp.path(),
        Command::Init {
            path: Some(tmp.path().to_path_buf()),
            name: Some("t".into()),
            observed: false,
        },
    );
    run(
        tmp.path(),
        Command::Intent {
            cmd: IntentCmd::Add {
                name: "always fails".into(),
                description: "demo".into(),
                level: "feature".into(),
                lifecycle: "implemented".into(),
                visibility: None,
                layer: None,
                aspect: None,
                allow_symbol_name: false,
            },
        },
    );
    let stdout_sentinel = "loom-out-sentinel";
    let stderr_sentinel = "loom-err-sentinel";
    // `echo` on POSIX sh emits exactly <arg>\n, so byte counts are deterministic
    // (sentinel.len() + 1) — letting us assert the real captured byte count,
    // not just field presence. exit 7 gives a non-zero exit code to record.
    let command = format!("echo {stdout_sentinel}; echo {stderr_sentinel} >&2; exit 7");
    run(
        tmp.path(),
        Command::Validation {
            cmd: ValidationCmd::Add {
                name: "false-proof".into(),
                r#type: "test".into(),
                command,
                intent: "always fails".into(),
                proof_level: None,
                proof_kind: None,
                journey_id: None,
                repo_native_kind: None,
                artifact: None,
            },
        },
    );

    // Drive `validation run` through the compiled binary with --json so the
    // bounded output object on the failed row is capturable on stdout. The
    // runner returns Ok even when a proof fails (it records the failure, then
    // emits), so the binary exits zero and loom_json_out's assertion holds.
    let v = loom_json_out(tmp.path(), &["validation", "run", "always fails", "--json"]);
    let ran = v
        .get("ran")
        .and_then(|r| r.as_array())
        .expect("validation run --json emits a `ran` array");
    assert_eq!(ran.len(), 1, "exactly one validation targets this intent");
    let row = &ran[0];
    assert_eq!(row["status"], "failed", "failed-proof row status");
    assert_eq!(row["exit_code"], 7, "recorded exit code");

    // The bounded-output contract: both stream excerpts, their full byte
    // counts, and truncation flags. Small output must not be truncated.
    let output = &row["output"];
    assert!(
        output["stdout"].as_str().unwrap().contains(stdout_sentinel),
        "output.stdout carries the stdout excerpt: {:?}",
        output["stdout"],
    );
    assert!(
        output["stderr"].as_str().unwrap().contains(stderr_sentinel),
        "output.stderr carries the stderr excerpt: {:?}",
        output["stderr"],
    );
    assert_eq!(
        output["stdout_bytes"].as_u64().unwrap(),
        (stdout_sentinel.len() + 1) as u64,
        "stdout_bytes is the real captured byte count (sentinel + newline)",
    );
    assert_eq!(
        output["stderr_bytes"].as_u64().unwrap(),
        (stderr_sentinel.len() + 1) as u64,
        "stderr_bytes is the real captured byte count (sentinel + newline)",
    );
    assert_eq!(
        output["stdout_truncated"], false,
        "small stdout is not truncated",
    );
    assert_eq!(
        output["stderr_truncated"], false,
        "small stderr is not truncated",
    );

    // The stored Validates-edge evidence must carry an excerpt string — not
    // just `exit 7`. validate_cmd prefers stderr when non-empty, so the
    // stderr sentinel is the one persisted.
    let store = Store::open(tmp.path()).unwrap();
    let proof = store
        .resolve_node("false-proof", Some(NodeType::Validation))
        .unwrap();
    assert_eq!(proof.status, "failed", "node status recorded as failed");
    let edges = store
        .edges_with(Some(EdgeKind::Validates), Some(&proof.id), None)
        .unwrap();
    assert!(!edges.is_empty(), "validates edge exists for the proof");
    let evidence = &edges[0].evidence;
    assert!(
        evidence.contains(stderr_sentinel),
        "edge evidence carries the stderr excerpt, not just the exit code: {evidence:?}",
    );
    assert!(
        evidence.contains("exit 7"),
        "edge evidence also records the exit code alongside the excerpt: {evidence:?}",
    );
}

#[test]
fn validate_timed_out_command_records_blocked() {
    let tmp = Tmp::new();
    run(
        tmp.path(),
        Command::Init {
            path: Some(tmp.path().to_path_buf()),
            name: Some("t".into()),
            observed: false,
        },
    );
    run(
        tmp.path(),
        Command::Intent {
            cmd: IntentCmd::Add {
                name: "can hang".into(),
                description: "demo".into(),
                level: "feature".into(),
                lifecycle: "implemented".into(),
                visibility: None,
                layer: None,
                aspect: None,
                allow_symbol_name: false,
            },
        },
    );
    run(
        tmp.path(),
        Command::Validation {
            cmd: ValidationCmd::Add {
                name: "slow-proof".into(),
                r#type: "test".into(),
                command: "sleep 2".into(),
                intent: "can hang".into(),
                proof_level: None,
                proof_kind: None,
                journey_id: None,
                repo_native_kind: None,
                artifact: None,
            },
        },
    );
    {
        let store = Store::open(tmp.path()).unwrap();
        let mut proof = store
            .resolve_node("slow-proof", Some(NodeType::Validation))
            .unwrap();
        proof.body["timeout_seconds"] = serde_json::json!(1);
        store.set_node_body(&proof.id, &proof.body).unwrap();
    }

    run(
        tmp.path(),
        Command::Validation {
            cmd: ValidationCmd::Run {
                intent: "can hang".into(),
                all: false,
            },
        },
    );

    let store = Store::open(tmp.path()).unwrap();
    let proof = store
        .resolve_node("slow-proof", Some(NodeType::Validation))
        .unwrap();
    assert_eq!(proof.status, "blocked");
}

// ---- hypothesis: invisible to maturity until adopted -----------------------

#[test]
fn hypothesis_invisible_until_adopted() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let target = store
        .add_node(
            NodeType::Intent,
            "checkout works",
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let before = loom::maturity::ladder(&store).unwrap();
    let before_planned = before
        .rungs
        .iter()
        .find(|r| r.name == "realized")
        .unwrap()
        .detail
        .clone();

    let h = store
        .add_node(
            NodeType::Hypothesis,
            "retry is duplicated",
            "claim",
            "proposed",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .ensure_edge(EdgeKind::Targets, &h.id, &target.id)
        .unwrap();

    // an unproven/unadopted hypothesis does not change the intent maturity
    let mid = loom::maturity::ladder(&store).unwrap();
    let mid_planned = mid
        .rungs
        .iter()
        .find(|r| r.name == "realized")
        .unwrap()
        .detail
        .clone();
    assert_eq!(
        before_planned, mid_planned,
        "hypothesis must not change maturity until adopted"
    );

    // prove queue serves it
    assert!(workitem::next(&store, Some(Mode::Prove)).unwrap().is_some());

    // adopt → a planned intent appears (now visible as build work)
    store.set_node_status(&h.id, "supported").unwrap();
    store.set_node_status(&h.id, "adopted").unwrap();
    store
        .add_node(
            NodeType::Intent,
            "dedupe retry logic",
            "",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    assert!(workitem::next(&store, Some(Mode::Build)).unwrap().is_some());
}

// ---- journey model ---------------------------------------------------------

#[test]
fn journey_add_links_steps_with_validates_not_sequence() {
    let tmp = Tmp::new();
    run(
        tmp.path(),
        Command::Init {
            path: Some(tmp.path().to_path_buf()),
            name: Some("t".into()),
            observed: false,
        },
    );
    let store = Store::open(tmp.path()).unwrap();
    for n in ["create cart", "capture payment"] {
        store
            .add_node(
                NodeType::Intent,
                n,
                "",
                "implemented",
                serde_json::json!({}),
            )
            .unwrap();
    }
    drop(store);

    let spec = tmp.path().join("checkout.journey.json");
    std::fs::write(
        &spec,
        r#"{"journey":"checkout-flow","steps":[{"intent":"create cart"},{"intent":"capture payment"}]}"#,
    )
    .unwrap();
    run(
        tmp.path(),
        Command::Journey {
            cmd: JourneyCmd::Add { spec },
        },
    );

    let store = Store::open(tmp.path()).unwrap();
    let journey = store
        .resolve_node("checkout-flow", Some(NodeType::Validation))
        .unwrap();
    assert_eq!(
        journey.body.get("type").and_then(|t| t.as_str()),
        Some("journey")
    );
    let validates = store
        .edges_with(Some(EdgeKind::Validates), Some(&journey.id), None)
        .unwrap();
    assert_eq!(validates.len(), 2, "journey validates each step intent");
    // journey add does NOT auto-create sequence edges — a spec's step order
    // is a test script, not a domain claim (assert deliberately via `edge relate`).
    let cart = store
        .resolve_node("create cart", Some(NodeType::Intent))
        .unwrap();
    let seq = store
        .edges_with(Some(EdgeKind::Sequence), Some(&cart.id), None)
        .unwrap();
    assert!(
        seq.is_empty(),
        "journey add must not auto-create sequence edges"
    );
}

#[test]
fn journey_add_is_idempotent_and_dedupes() {
    // Reported bug: `journey add` twice for the same id created duplicate
    // validation nodes, and `journey run` then failed with "add it first".
    let tmp = Tmp::new();
    run(
        tmp.path(),
        Command::Init {
            path: Some(tmp.path().to_path_buf()),
            name: Some("t".into()),
            observed: false,
        },
    );
    let store = Store::open(tmp.path()).unwrap();
    for n in ["create cart", "capture payment"] {
        store
            .add_node(
                NodeType::Intent,
                n,
                "",
                "implemented",
                serde_json::json!({}),
            )
            .unwrap();
    }
    drop(store);
    let spec = tmp.path().join("checkout.journey.json");
    std::fs::write(
        &spec,
        r#"{"journey":"checkout-flow","steps":[{"intent":"create cart"},{"intent":"capture payment"}]}"#,
    )
    .unwrap();
    for _ in 0..3 {
        run(
            tmp.path(),
            Command::Journey {
                cmd: JourneyCmd::Add { spec: spec.clone() },
            },
        );
    }
    let store = Store::open(tmp.path()).unwrap();
    let vals = loom::journey::journey_validations(&store, "checkout-flow").unwrap();
    assert_eq!(
        vals.len(),
        1,
        "repeated add upserts one validation, not N duplicates"
    );
    // the run guard resolves cleanly — no ambiguity, no misleading "add it first"
    assert!(loom::journey::require(&store, "checkout-flow").is_ok());
    let validates = store
        .edges_with(Some(EdgeKind::Validates), Some(&vals[0].id), None)
        .unwrap();
    assert_eq!(
        validates.len(),
        2,
        "steps linked once, not duplicated per add"
    );
}

#[test]
fn resolve_validation_repairs_a_duplicated_graph() {
    // A graph already bricked by the old bug (N duplicate nodes) self-heals:
    // resolve_validation picks the canonical, merges links, deletes the rest.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let cart = store
        .add_node(
            NodeType::Intent,
            "create cart",
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let mut ids = vec![];
    for _ in 0..3 {
        let v = store
            .add_node(
                NodeType::Validation,
                "dup-flow",
                "",
                "not_run",
                serde_json::json!({"type":"journey","proof_kind":"journey","journey_id":"dup-flow"}),
            )
            .unwrap();
        ids.push(v.id);
    }
    // A later fixed add linked a step onto one dup only.
    store
        .ensure_edge(EdgeKind::Validates, ids.last().unwrap(), &cart.id)
        .unwrap();
    assert_eq!(
        loom::journey::journey_validations(&store, "dup-flow")
            .unwrap()
            .len(),
        3
    );
    let canonical = loom::journey::resolve_validation(&store, "dup-flow", true).unwrap();
    let after = loom::journey::journey_validations(&store, "dup-flow").unwrap();
    assert_eq!(after.len(), 1, "duplicates removed");
    assert_eq!(after[0].id, canonical.id);
    let validates = store
        .edges_with(Some(EdgeKind::Validates), Some(&canonical.id), None)
        .unwrap();
    assert_eq!(
        validates.len(),
        1,
        "the deleted dup's link merged onto the canonical — no coverage lost"
    );
    assert!(loom::journey::require(&store, "dup-flow").is_ok());
}

#[test]
fn missing_journey_gives_add_it_first_not_ambiguous() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let err = loom::journey::require(&store, "nope")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("add it first"),
        "0-case gives the honest error: {err}"
    );
}

#[test]
fn journey_remove_deletes_the_journey() {
    let tmp = Tmp::new();
    run(
        tmp.path(),
        Command::Init {
            path: Some(tmp.path().to_path_buf()),
            name: Some("t".into()),
            observed: false,
        },
    );
    let spec = tmp.path().join("f.journey.json");
    std::fs::write(&spec, r#"{"journey":"gone-flow","steps":[]}"#).unwrap();
    run(
        tmp.path(),
        Command::Journey {
            cmd: JourneyCmd::Add { spec },
        },
    );
    let store = Store::open(tmp.path()).unwrap();
    assert_eq!(
        loom::journey::journey_validations(&store, "gone-flow")
            .unwrap()
            .len(),
        1
    );
    drop(store);
    run(
        tmp.path(),
        Command::Journey {
            cmd: JourneyCmd::Remove {
                id: "gone-flow".into(),
            },
        },
    );
    let store = Store::open(tmp.path()).unwrap();
    assert!(loom::journey::journey_validations(&store, "gone-flow")
        .unwrap()
        .is_empty());
}

#[test]
fn journey_readd_after_spec_change_resets_proof() {
    let tmp = Tmp::new();
    run(
        tmp.path(),
        Command::Init {
            path: Some(tmp.path().to_path_buf()),
            name: Some("t".into()),
            observed: false,
        },
    );
    let store = Store::open(tmp.path()).unwrap();
    store
        .add_node(
            NodeType::Intent,
            "step a",
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    drop(store);
    let spec = tmp.path().join("r.journey.json");
    std::fs::write(
        &spec,
        r#"{"journey":"reset-flow","steps":[{"intent":"step a"}]}"#,
    )
    .unwrap();
    run(
        tmp.path(),
        Command::Journey {
            cmd: JourneyCmd::Add { spec: spec.clone() },
        },
    );
    let store = Store::open(tmp.path()).unwrap();
    let v = loom::journey::journey_validations(&store, "reset-flow")
        .unwrap()
        .remove(0);
    store.set_node_status(&v.id, "passed").unwrap();
    drop(store);
    // Edit the spec at the SAME path (different bytes) and re-add.
    std::fs::write(
        &spec,
        r#"{"journey":"reset-flow","base":"http://x","steps":[{"intent":"step a"}]}"#,
    )
    .unwrap();
    run(
        tmp.path(),
        Command::Journey {
            cmd: JourneyCmd::Add { spec },
        },
    );
    let store = Store::open(tmp.path()).unwrap();
    let v = loom::journey::journey_validations(&store, "reset-flow")
        .unwrap()
        .remove(0);
    assert_eq!(
        v.status, "not_run",
        "a changed spec resets the passed proof to not_run"
    );
}

#[test]
fn journey_list_recognizes_proof_kind_journey() {
    let tmp = Tmp::new();
    run(
        tmp.path(),
        Command::Init {
            path: Some(tmp.path().to_path_buf()),
            name: Some("t".into()),
            observed: false,
        },
    );
    let store = Store::open(tmp.path()).unwrap();
    store
        .add_node(
            NodeType::Validation,
            "checkout-flow-runner",
            "",
            "not_run",
            serde_json::json!({"type":"manual_check","proof_kind":"journey"}),
        )
        .unwrap();
    drop(store);

    // `journey list` recognizes a journey proof by `proof_kind`, not only by
    // `type: journey` — a repo-native runner stays visible to the family.
    let journey_rows = loom_json_out(tmp.path(), &["journey", "list", "--json"]);
    assert!(
        journey_rows
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["name"] == "checkout-flow-runner"),
        "journey list includes proof_kind=journey validations: {journey_rows}"
    );
}

// ---- vocab + layer ---------------------------------------------------------

#[test]
fn vocab_gates_tagging_and_layer_order_persists() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "checkout works",
            "",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    assert!(!store.vocab_has("payments").unwrap());
    store
        .add_vocab_term("payments", "checkout money flow")
        .unwrap();
    assert!(store.vocab_has("payments").unwrap());
    store
        .set_tag(&intent.id, loom::model::TargetKind::Node, "payments")
        .unwrap();
    assert_eq!(
        store
            .tags_of(&intent.id, loom::model::TargetKind::Node)
            .unwrap(),
        vec!["payments".to_string()]
    );
    store
        .set_meta("layer_order", r#"["presentation","domain","storage"]"#)
        .unwrap();
    assert!(store
        .get_meta("layer_order")
        .unwrap()
        .unwrap()
        .contains("domain"));
}

// ---- integration monitoring: the `edge call` contract link ------------------

#[test]
fn edge_call_binds_validation_to_surface_idempotently() {
    let tmp = Tmp::new();
    {
        let store = Store::init(tmp.path(), Some("t"), false).unwrap();
        let intent = store
            .add_node(
                NodeType::Intent,
                "consume X",
                "",
                "planned",
                serde_json::json!({}),
            )
            .unwrap();
        store
            .add_node(
                NodeType::InterfaceSurface,
                "XSurface",
                "",
                "",
                serde_json::json!({ "kind": "http" }),
            )
            .unwrap();
        let val = store
            .add_node(
                NodeType::Validation,
                "x-contract",
                "",
                "not_run",
                serde_json::json!({ "type": "contract" }),
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
    } // drop the store → release the lock before the CLI reopens it

    let call = || {
        run(
            tmp.path(),
            Command::Edge {
                cmd: EdgeCmd::Call {
                    validation: "x-contract".into(),
                    surface: "XSurface".into(),
                },
            },
        )
    };
    call();
    call(); // re-binding the same pair must not duplicate

    let store = Store::open(tmp.path()).unwrap();
    let calls = store.edges_with(Some(EdgeKind::Calls), None, None).unwrap();
    assert_eq!(
        calls.len(),
        1,
        "edge call must be idempotent (no duplicate)"
    );
}

// ---- cold-LLM entrypoints must never strand on a fresh graph ----------------

#[test]
fn monitoring_self_teaching_surfaces_run_clean() {
    let tmp = Tmp::new();
    Store::init(tmp.path(), Some("t"), false).unwrap(); // temporary → lock released at `;`

    // The integration-monitoring playbook and the fresh-graph entrypoints must
    // run without erroring — a cold LLM relies on them for direction.
    run(tmp.path(), Command::Guide { role: None });
    run(
        tmp.path(),
        Command::Guide {
            role: Some(loom::cli::RoleArg::Monitor),
        },
    );
    run(tmp.path(), Command::Session);
    run(
        tmp.path(),
        Command::Next {
            mode: None,
            all: false,
        },
    );
}

// ---- mark is atomic: a rejected verdict must not persist 'passed' -----------

#[test]
fn validation_mark_passed_without_evidence_is_atomic() {
    let tmp = Tmp::new();
    {
        let store = Store::init(tmp.path(), Some("t"), false).unwrap();
        let intent = store
            .add_node(NodeType::Intent, "x", "", "planned", serde_json::json!({}))
            .unwrap();
        let val = store
            .add_node(
                NodeType::Validation,
                "probe",
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
    }
    // A passing verdict with empty evidence violates INV-6: it must error AND
    // leave the validation at not_run (no partial commit).
    let res = loom::commands::run(Cli {
        graph: Some(tmp.path().to_path_buf()),
        json: false,
        command: Command::Validation {
            cmd: ValidationCmd::Verdict {
                key: "probe".into(),
                outcome: "passed".into(),
                evidence: "".into(),
                reason: "".into(),
            },
        },
    });
    assert!(res.is_err(), "passing without evidence must be rejected");
    let store = Store::open(tmp.path()).unwrap();
    assert_eq!(
        store
            .resolve_node("probe", Some(NodeType::Validation))
            .unwrap()
            .status,
        "not_run",
        "a rejected mark must not leave the validation showing passed"
    );
}

// ---- the short id loom prints must resolve back to the node -----------------

#[test]
fn resolve_node_accepts_printed_short_id() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let n = store
        .add_node(
            NodeType::Intent,
            "some intent",
            "",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    let short = &n.id[..8]; // the form every command echoes in brackets
    let resolved = store.resolve_node(short, Some(NodeType::Intent)).unwrap();
    assert_eq!(
        resolved.id, n.id,
        "the short id loom prints must resolve back to the same node"
    );
}

// ---- rescan registers files that appeared after the original glob add -------

#[test]
fn codefile_rescan_picks_up_new_files() {
    let tmp = Tmp::new();
    let write = |rel: &str, c: &str| {
        let p = tmp.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, c).unwrap();
    };
    write("vendor/up/a.rs", "fn a() {}\n");
    run(
        tmp.path(),
        Command::Init {
            path: Some(tmp.path().to_path_buf()),
            name: Some("t".into()),
            observed: false,
        },
    );
    run(
        tmp.path(),
        Command::Codefile {
            cmd: CodefileCmd::Add {
                path: "vendor/up/**/*.rs".into(),
            },
        },
    );
    // a new endpoint appears upstream after the initial registration
    write("vendor/up/b.rs", "fn b() {}\n");
    run(
        tmp.path(),
        Command::Codefile {
            cmd: CodefileCmd::Rescan,
        },
    );

    let store = Store::open(tmp.path()).unwrap();
    let files: Vec<String> = store
        .list_nodes(Some(NodeType::CodeFile), usize::MAX)
        .unwrap()
        .into_iter()
        .map(|n| n.name)
        .collect();
    assert!(files.contains(&"vendor/up/a.rs".to_string()));
    assert!(
        files.contains(&"vendor/up/b.rs".to_string()),
        "rescan must register the newly-appeared file"
    );
}

// ---- intent list honors the global --json flag (was silently ignored) -------

#[test]
fn intent_list_json_runs_clean() {
    let tmp = Tmp::new();
    {
        let store = Store::init(tmp.path(), Some("t"), false).unwrap();
        store
            .add_node(
                NodeType::Intent,
                "alpha",
                "",
                "planned",
                serde_json::json!({}),
            )
            .unwrap();
    }
    // --json must be honored; the run() helper unwraps, so a serialization error
    // (or the flag being ignored in a way that panics) would fail the test.
    loom::commands::run(Cli {
        graph: Some(tmp.path().to_path_buf()),
        json: true,
        command: Command::Intent {
            cmd: IntentCmd::List { limit: 50 },
        },
    })
    .unwrap();
}

// ---- observed graphs disable the build/fix lanes (you can't change an upstream you only watch) ----

#[test]
fn observed_graph_disables_build_and_fix_lanes() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("foreign"), true).unwrap(); // observed
    store
        .add_node(
            NodeType::Intent,
            "upstream does X",
            "",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    assert!(
        workitem::next(&store, Some(Mode::Build)).unwrap().is_none(),
        "an observed graph offers no build work"
    );
    assert!(
        workitem::next(&store, Some(Mode::Fix)).unwrap().is_none(),
        "an observed graph offers no fix work"
    );

    // The same planned intent on an OWNED graph does offer build work.
    let tmp2 = Tmp::new();
    let owned = Store::init(tmp2.path(), Some("mine"), false).unwrap();
    owned
        .add_node(
            NodeType::Intent,
            "service does X",
            "",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    assert!(
        workitem::next(&owned, Some(Mode::Build)).unwrap().is_some(),
        "an owned graph offers build work for a planned intent"
    );
}

// ---- vocab remove cascade-untags nodes that carry the term -------------------

#[test]
fn vocab_remove_cascade_untags() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    store.add_vocab_term("payments", "money").unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "checkout",
            "",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .set_tag(&intent.id, loom::model::TargetKind::Node, "payments")
        .unwrap();
    assert_eq!(
        store
            .tags_of(&intent.id, loom::model::TargetKind::Node)
            .unwrap(),
        vec!["payments".to_string()]
    );

    store.remove_vocab_term("payments").unwrap();
    assert!(
        store
            .tags_of(&intent.id, loom::model::TargetKind::Node)
            .unwrap()
            .is_empty(),
        "removing a vocab term must untag every node carrying it"
    );
    assert!(
        store.remove_vocab_term("nope").is_err(),
        "removing a non-existent term errors"
    );
}

// ---- UPDATE-path coverage: facet correction + endpoint-named unlink ----------

#[test]
fn intent_set_corrects_facets() {
    let tmp = Tmp::new();
    let iid = {
        let store = Store::init(tmp.path(), Some("t"), false).unwrap();
        store
            .add_node(NodeType::Intent, "f", "", "planned", serde_json::json!({}))
            .unwrap()
            .id
    };
    run(
        tmp.path(),
        Command::Intent {
            cmd: IntentCmd::Update {
                key: "f".into(),
                description: None,
                name: None,
                level: Some("system".into()),
                visibility: Some("internal".into()),
                aspect: None,
                lifecycle: None,
                reason: "attribute correction".into(),
                reword: false,
            },
        },
    );
    let store = Store::open(tmp.path()).unwrap();
    assert_eq!(
        store
            .get_facet(&iid, loom::model::TargetKind::Node, "level")
            .unwrap()
            .as_deref(),
        Some("system")
    );
    assert_eq!(
        store
            .get_facet(&iid, loom::model::TargetKind::Node, "visibility")
            .unwrap()
            .as_deref(),
        Some("internal")
    );
}
// ---- `intent add --layer` writes the node-scoped `layer` facet -------------
//
// The `--layer` flag is the user-facing surface for stamping an architecture
// layer label onto an intent node. It must write a Node-scoped Asserted facet
// under the exact key `layer` (the same key the layering detector reads), not a
// body field or an edge. A regression that drops the flag, mis-scopes it, or
// writes the wrong key reddens this. Drives the compiled binary end-to-end,
// then reads the stored facet back through the Store API for precision.

#[test]
fn intent_add_layer_writes_node_layer_facet() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "checkout places order",
            "--lifecycle",
            "implemented",
            "--layer",
            "domain",
        ],
    );

    let store = Store::open(tmp.path()).unwrap();
    let node = store
        .resolve_node("checkout places order", Some(NodeType::Intent))
        .unwrap();
    assert_eq!(
        store
            .get_facet(&node.id, TargetKind::Node, "layer")
            .unwrap()
            .as_deref(),
        Some("domain"),
        "--layer must stamp a Node-scoped Asserted `layer` facet with the label"
    );
}

#[test]
fn validation_unlink_removes_the_validates_edge() {
    let tmp = Tmp::new();
    let vid = {
        let store = Store::init(tmp.path(), Some("t"), false).unwrap();
        let intent = store
            .add_node(NodeType::Intent, "f", "", "planned", serde_json::json!({}))
            .unwrap();
        let val = store
            .add_node(
                NodeType::Validation,
                "v",
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
        val.id
    };
    run(
        tmp.path(),
        Command::Validation {
            cmd: ValidationCmd::Unlink {
                validation: "v".into(),
                intent: "f".into(),
            },
        },
    );
    let store = Store::open(tmp.path()).unwrap();
    assert!(
        store
            .edges_with(Some(EdgeKind::Validates), Some(&vid), None)
            .unwrap()
            .is_empty(),
        "unlink removes the validates edge by endpoint name"
    );
}
// ---- global --json must be honored by read-style commands -------------------
//
// Regression: `loom status` historically ignored `cli.json` and emitted human
// text even with `--json`. These drive the compiled binary end-to-end
// (std::process::Command + CARGO_BIN_EXE_loom), placing `--json` after the
// subcommand so Clap's global-flag propagation from a subcommand is the surface
// under test. Each asserts stdout parses as JSON with a command-specific shape
// that human text cannot satisfy — a regression to human output reddens them.

/// Path to the compiled `loom` binary, provided by Cargo at build time.
fn loom_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_loom"))
}
/// Initialize a fresh graph at `tmp` through the binary under test. `init`
/// takes its target as a positional `path` (the global `--graph` does not
/// steer it), so the temp dir is passed positionally, not via `--graph`.
fn loom_init(tmp: &Path, name: Option<&str>) {
    let mut cmd = std::process::Command::new(loom_bin());
    cmd.arg("init").arg(tmp);
    if let Some(n) = name {
        cmd.args(["--name", n]);
    }
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("spawn loom init: {e}"));
    assert!(
        out.status.success(),
        "loom init {:?} failed: {:?}\n{}",
        tmp,
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Run `loom --graph <tmp> <args>` and assert it exits successfully, returning
/// stdout parsed as a JSON value. Panics with stdout/stderr on failure so the
/// regression (human text emitted under --json) is diagnosed, not hidden.
fn loom_json_out(tmp: &Path, args: &[&str]) -> serde_json::Value {
    let mut cmd = std::process::Command::new(loom_bin());
    cmd.arg("--graph").arg(tmp).args(args);
    let out = cmd.output().unwrap_or_else(|e| panic!("spawn loom: {e}"));
    assert!(
        out.status.success(),
        "loom {:?} failed: {:?}\n--stderr--\n{}",
        args,
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "loom {:?} did not emit JSON under --json (status {:?}):\n--stdout--\n{}\nparse error: {e}",
            args, out.status, stdout
        )
    })
}
fn loom_json_err(tmp: &Path, args: &[&str]) -> serde_json::Value {
    let mut cmd = std::process::Command::new(loom_bin());
    cmd.arg("--graph").arg(tmp).args(args);
    let out = cmd.output().unwrap_or_else(|e| panic!("spawn loom: {e}"));
    assert!(
        !out.status.success(),
        "loom {:?} unexpectedly succeeded:\n--stdout--\n{}\n--stderr--\n{}",
        args,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "loom {:?} did not emit JSON on failure:\n--stdout--\n{}\n--stderr--\n{}\nparse error: {e}",
            args,
            stdout,
            String::from_utf8_lossy(&out.stderr)
        )
    })
}
/// Run `loom --graph <tmp> <args>` and assert it exits zero, discarding stdout.
/// For setup commands (`intent add`, `validation add`) that emit human text
/// and have no `--json` output — reserve `loom_json_out` for read-style /
/// `--json`-emitting commands.
fn loom_ok(tmp: &Path, args: &[&str]) {
    let mut cmd = std::process::Command::new(loom_bin());
    cmd.arg("--graph").arg(tmp).args(args);
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("spawn loom {:?}: {e}", args));
    assert!(
        out.status.success(),
        "loom {:?} failed: {:?}\n--stderr--\n{}",
        args,
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn global_json_status_emits_machine_readable_envelope() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("jsonreg"));

    let v = loom_json_out(tmp.path(), &["status", "--json"]);
    // Human status prints `graph: …`; JSON must be an object with the envelope.
    let obj = v.as_object().expect("status --json is a JSON object");
    assert!(obj.contains_key("graph"), "status --json has `graph`");
    assert!(obj.contains_key("counts"), "status --json has `counts`");
    assert!(obj.contains_key("maturity"), "status --json has `maturity`");
    assert!(
        obj.contains_key("graph_state"),
        "status --json has `graph_state`"
    );
    // Shape, not pretty formatting: the name we set and integer counts.
    assert_eq!(obj["graph"]["name"], "jsonreg");
    assert!(obj["graph"]["graph_id"].is_string(), "graph_id is a string");
    assert_eq!(obj["counts"]["intents"], 0);
    assert_eq!(obj["counts"]["codefiles"], 0);
    assert_eq!(obj["counts"]["edges"], 0);
}

#[test]
fn global_json_finding_list_emits_an_array() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), None);

    // A fresh graph has zero findings. Human output prints the line `no findings`;
    // JSON must be a (possibly empty) array.
    let v = loom_json_out(tmp.path(), &["finding", "list", "--json"]);
    let arr = v.as_array().expect("finding list --json is a JSON array");
    assert!(arr.is_empty(), "fresh graph lists no findings");
}

#[test]
fn global_json_next_all_emits_per_mode_queues() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), None);

    let v = loom_json_out(tmp.path(), &["next", "--all", "--json"]);
    let obj = v.as_object().expect("next --all --json is a JSON object");
    assert!(
        obj.contains_key("compass"),
        "next --all --json has `compass`"
    );
    assert!(
        obj.contains_key("graph_state"),
        "next --all --json has `graph_state`"
    );
    let queues = obj
        .get("queues")
        .and_then(|q| q.as_object())
        .expect("next --all --json has a `queues` object");
    // The closeout view emits one queue per mode; every one is present.
    for mode in [
        "fix", "validate", "build", "quality", "prove", "analyze", "triage",
    ] {
        assert!(queues.contains_key(mode), "queues has `{mode}`");
    }
}

#[test]
fn json_read_commands_emit_json_and_full_fields() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("json-read"));
    let long = "Rhai op-count determinism and replay: intent 6520b0e7 currently has a long audit note that must not be truncated in JSON";
    {
        let store = Store::open(tmp.path()).unwrap();
        let a = store
            .add_node(
                NodeType::Intent,
                "layered behavior a",
                "",
                "implemented",
                serde_json::json!({}),
            )
            .unwrap();
        let b = store
            .add_node(
                NodeType::Intent,
                "layered behavior b",
                "",
                "implemented",
                serde_json::json!({}),
            )
            .unwrap();
        store
            .set_facet(
                &a.id,
                TargetKind::Node,
                "layer",
                "api",
                TruthClass::Asserted,
            )
            .unwrap();
        store
            .set_facet(
                &b.id,
                TargetKind::Node,
                "layer",
                "storage",
                TruthClass::Asserted,
            )
            .unwrap();
        let cf = store
            .add_node(
                NodeType::CodeFile,
                "src/a.rs",
                "",
                "",
                serde_json::json!({}),
            )
            .unwrap();
        store
            .add_node(
                NodeType::CodeFile,
                "src/orphan.rs",
                "",
                "",
                serde_json::json!({}),
            )
            .unwrap();
        store
            .add_edge(EdgeKind::Implements, &a.id, &cf.id, TruthClass::Asserted)
            .unwrap();
        let validation = store
            .add_node(
                NodeType::Validation,
                "proof-a",
                "",
                "not_run",
                serde_json::json!({"type":"test","command":"true"}),
            )
            .unwrap();
        store
            .add_edge(
                EdgeKind::Validates,
                &validation.id,
                &a.id,
                TruthClass::Asserted,
            )
            .unwrap();
        store
            .add_node(
                NodeType::InboxItem,
                "truncated human title",
                long,
                "new",
                serde_json::json!({"source":"test","link":"file:notes.md"}),
            )
            .unwrap();
    }

    let status = loom_json_out(tmp.path(), &["status", "--json"]);
    assert_eq!(status["validation_summary"]["registered"], 1);
    assert_eq!(status["validation_summary"]["not_run"], 1);
    let proven = status["maturity"]["rungs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "proven")
        .unwrap();
    assert_eq!(proven["state"], "unmet");
    assert!(proven["detail"].as_str().unwrap().contains("0 passed"));
    assert_eq!(status["code_ownership"]["registered"], 2);
    assert_eq!(status["code_ownership"]["owned"], 1);
    assert_eq!(status["code_ownership"]["unowned"], 1);
    assert_eq!(status["detectors"]["layering"]["armed"], false);
    assert_eq!(
        status["detectors"]["layering"]["warning"],
        "no layer order declared"
    );

    let coverage = loom_json_out(tmp.path(), &["coverage", "--json"]);
    assert_eq!(coverage["codefiles"]["registered"], 2);
    assert_eq!(coverage["codefiles"]["unowned"], 1);

    let inbox = loom_json_out(tmp.path(), &["inbox", "list", "--json"]);
    assert_eq!(inbox[0]["text"], long);
    assert_eq!(inbox[0]["source"], "test");
    assert_eq!(inbox[0]["link"], "file:notes.md");

    let validation = loom_json_out(tmp.path(), &["validation", "show", "proof-a", "--json"]);
    assert_eq!(validation["status"], "not_run");
    assert_eq!(validation["validates"][0]["name"], "layered behavior a");

    let gaps = loom_json_out(tmp.path(), &["surface", "gaps", "--json"]);
    assert_eq!(gaps["armed"], false);
    assert_eq!(gaps["surface_count"], 0);
    assert_eq!(gaps["warning"], "no surfaces declared");

    let layer = loom_json_out(tmp.path(), &["layer", "list", "--json"]);
    assert_eq!(layer["armed"], false);
    assert_eq!(layer["warning"], "no layer order declared");
}

#[test]
fn next_all_routes_new_inbox_items_and_discovery_alias_parses() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("inbox-json"));
    {
        let store = Store::open(tmp.path()).unwrap();
        store
            .add_node(
                NodeType::InboxItem,
                "raw lead",
                "raw lead full text",
                "new",
                serde_json::json!({"source":"test"}),
            )
            .unwrap();
    }

    let next = loom_json_out(tmp.path(), &["next", "--all", "--json"]);
    assert_eq!(next["graph_state"]["inbox"], 1);
    assert_eq!(next["queues"]["triage"]["target"]["kind"], "inbox_item");

    let discovery = loom_json_out(tmp.path(), &["next", "--mode", "discovery", "--json"]);
    assert!(discovery.get("work_item").is_some());
}

#[test]
fn advertised_global_json_read_commands_parse_as_json() {
    let tmp = Tmp::new();

    let init_out = std::process::Command::new(loom_bin())
        .arg("init")
        .arg(tmp.path())
        .arg("--json")
        .output()
        .unwrap_or_else(|e| panic!("spawn loom init --json: {e}"));
    assert!(
        init_out.status.success(),
        "loom init --json failed: {:?}\n{}",
        init_out.status,
        String::from_utf8_lossy(&init_out.stderr)
    );
    serde_json::from_slice::<serde_json::Value>(&init_out.stdout)
        .expect("loom init --json emits JSON");

    let edge_id: String;

    {
        let store = Store::open(tmp.path()).unwrap();
        let intent = store
            .add_node(
                NodeType::Intent,
                "behavior matrix",
                "matrix behavior",
                "implemented",
                serde_json::json!({}),
            )
            .unwrap();
        let codefile = store
            .add_node(
                NodeType::CodeFile,
                "src/matrix.rs",
                "",
                "",
                serde_json::json!({}),
            )
            .unwrap();
        let edge = store
            .add_edge(
                EdgeKind::Implements,
                &intent.id,
                &codefile.id,
                TruthClass::Asserted,
            )
            .unwrap();
        edge_id = edge.id.clone();
        store
            .add_node(
                NodeType::Validation,
                "matrix proof",
                "",
                "not_run",
                serde_json::json!({"type":"test","command":"true"}),
            )
            .unwrap();
        store
            .add_node(
                NodeType::QualityRule,
                "matrix rule",
                "matrix rule description",
                "",
                serde_json::json!({"category":"test"}),
            )
            .unwrap();
        store
            .add_node(
                NodeType::Hypothesis,
                "matrix hypothesis",
                "claim",
                "proposed",
                serde_json::json!({"proposal":"try it","predicted_outcome":"works"}),
            )
            .unwrap();
        store
            .add_node(
                NodeType::InterfaceSurface,
                "matrix surface",
                "",
                "",
                serde_json::json!({"kind":"api","identity":"matrix"}),
            )
            .unwrap();
        store
            .add_node(
                NodeType::InboxItem,
                "matrix inbox",
                "matrix inbox full text",
                "new",
                serde_json::json!({"source":"test"}),
            )
            .unwrap();
        store.add_vocab_term("matrix", "test vocabulary").unwrap();
        store
            .set_meta(
                "ignores",
                &serde_json::to_string(&serde_json::json!([
                    {"glob":"target/**","reason":"generated"}
                ]))
                .unwrap(),
            )
            .unwrap();
        store
            .add_node(
                NodeType::TaskRecord,
                "matrix task",
                "",
                "proposed",
                serde_json::json!({"kind":"test"}),
            )
            .unwrap();
    }

    let commands: &[&[&str]] = &[
        &["status", "--json"],
        &["next", "--all", "--json"],
        &["coverage", "--json"],
        &["inbox", "list", "--json"],
        &["validation", "list", "--json"],
        &["surface", "gaps", "--json"],
        &["layer", "list", "--json"],
        &["finding", "list", "--json"],
        &["doctor", "--json"],
        &["smells", "--json"],
        &["debt", "--json"],
        &["codefile", "list", "--json"],
        &["intent", "list", "--json"],
        &["rule", "list", "--json"],
        &["hypothesis", "list", "--json"],
        &["surface", "list", "--json"],
        &["journey", "list", "--json"],
        &["vocab", "list", "--json"],
        &["ignore", "list", "--json"],
        &["task", "list", "--json"],
        &["whoami", "--json"],
        &["session", "--json"],
        &["detect", "--json"],
        &["schema", "--json"],
        &["find", "behavior", "--json"],
        &["edge", "list", "--json"],
        &["guide", "--json"],
        &["sync", "--json"],
        &["validation", "run", "--all", "--json"],
    ];

    for args in commands {
        let _ = loom_json_out(tmp.path(), args);
    }

    let _ = loom_json_out(
        tmp.path(),
        &["validation", "show", "matrix proof", "--json"],
    );
    let _ = loom_json_out(tmp.path(), &["codefile", "show", "src/matrix.rs", "--json"]);
    let _ = loom_json_out(tmp.path(), &["rule", "show", "matrix rule", "--json"]);
    let _ = loom_json_out(
        tmp.path(),
        &["hypothesis", "show", "matrix hypothesis", "--json"],
    );
    let _ = loom_json_out(tmp.path(), &["surface", "show", "matrix surface", "--json"]);
    let _ = loom_json_out(tmp.path(), &["task", "show", "matrix task", "--json"]);
    let _ = loom_json_out(tmp.path(), &["edge", "show", edge_id.as_str(), "--json"]);
    let _ = loom_json_out(tmp.path(), &["export", "--json"]);
    let _ = loom_json_out(tmp.path(), &["export", "--check", "--json"]);
    let _ = loom_json_out(tmp.path(), &["door", "raw matrix note", "--json"]);
}
// ---- proposal MVP: a durable plan artifact, decomposable into adopted work ----
//
// These drive the compiled binary end-to-end (std::process::Command +
// CARGO_BIN_EXE_loom), placing `--json` after each subcommand so Clap's
// global-flag propagation from a subcommand is the surface under test. Each
// asserts stdout parses as JSON with a contract-specific shape that human
// output cannot satisfy. They assert the MVP contract only — proposal JSON
// carries `id`, `name`, `status`, `description`, `body`; `body` carries
// `raw`, `source`, and `items`; an adopted item is reflected in `show` and
// emits a `spawned` intent carrying source proposal/item metadata.

#[test]
fn proposal_add_text_emits_full_raw_and_empty_items() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("prop"));

    let raw = "Journey Catalog: a browsable index of every user-facing journey";
    let v = loom_json_out(
        tmp.path(),
        &[
            "proposal",
            "add",
            "--title",
            "Journey Catalog",
            "--text",
            raw,
            "--json",
        ],
    );

    // Contract: a proposal is one asserted node whose `name` is the title,
    // `status` starts as `captured`, `description` is a short summary, and
    // `body` stores structured JSON.
    let obj = v.as_object().expect("proposal add --json is a JSON object");
    assert!(obj["id"].is_string(), "proposal add --json has `id`");
    assert_eq!(obj["name"], "Journey Catalog");
    assert_eq!(obj["status"], "captured");
    assert!(
        obj["description"].is_string(),
        "has a `description` summary"
    );

    // Contract: body includes `raw`, `source`, and `items`.
    let body = obj
        .get("body")
        .and_then(|b| b.as_object())
        .expect("proposal add --json has a `body` object");
    assert_eq!(body["raw"], raw, "`body.raw` holds the full source text");
    assert!(body["source"].is_string(), "`body.source` is present");
    assert!(
        body["items"].is_array() && body["items"].as_array().unwrap().is_empty(),
        "`body.items` is an empty array on add"
    );

    let frontmatter_raw =
        "---\ntitle: \"With Frontmatter\"\ntype: proposal\n---\n\n# With Frontmatter\n\nBody";
    let frontmatter_text_arg = format!("--text={frontmatter_raw}");
    let frontmatter = loom_json_out(
        tmp.path(),
        &[
            "proposal",
            "add",
            "--title",
            "With Frontmatter",
            &frontmatter_text_arg,
            "--json",
        ],
    );
    assert_eq!(
        frontmatter["description"], "# With Frontmatter",
        "proposal summary skips YAML frontmatter"
    );
}

#[test]
fn proposal_item_adopt_spawns_intent_and_show_reflects_it() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("prop-adopt"));

    let raw = "Add a journey catalog so planners can browse user journeys.";
    let added = loom_json_out(
        tmp.path(),
        &[
            "proposal",
            "add",
            "--title",
            "Journey Catalog",
            "--text",
            raw,
            "--json",
        ],
    );
    let proposal_id = added["id"]
        .as_str()
        .expect("proposal add --json emits an `id`")
        .to_string();

    // Contract: `item add` appends an item; the first is number 1.
    let item = loom_json_out(
        tmp.path(),
        &[
            "proposal",
            "item",
            "add",
            &proposal_id,
            "--text",
            "Index every user-facing journey in a browsable catalog",
            "--kind",
            "intent_candidate",
            "--json",
        ],
    );
    let item_obj = item.as_object().expect("item add --json is a JSON object");
    assert_eq!(item_obj["number"], 1, "first item is number 1");
    assert_eq!(
        item_obj["kind"], "intent_candidate",
        "item kind round-trips"
    );

    // Contract: `item adopt ... 1 --as intent` marks the item adopted and
    // spawns a planned Intent node using --name/--description, carrying source
    // proposal/item metadata in its body.
    let adopt = loom_json_out(
        tmp.path(),
        &[
            "proposal",
            "item",
            "adopt",
            &proposal_id,
            "1",
            "--as",
            "intent",
            "--name",
            "journey-catalog",
            "--description",
            "Browse every user-facing journey",
            "--json",
        ],
    );
    let adopt_obj = adopt
        .as_object()
        .expect("item adopt --json is a JSON object");
    assert!(
        adopt_obj.contains_key("proposal"),
        "adopt --json includes the updated `proposal`"
    );
    let updated_item = adopt_obj
        .get("item")
        .and_then(|i| i.as_object())
        .expect("adopt --json includes the updated `item`");
    assert_eq!(updated_item["number"], 1);
    assert_eq!(
        updated_item["status"], "adopted",
        "adopt sets the item status to `adopted`"
    );

    let spawned = adopt_obj
        .get("spawned")
        .and_then(|s| s.as_object())
        .expect("adopt --as intent emits a `spawned` object");
    assert!(spawned["id"].is_string(), "spawned intent has a node `id`");
    let spawned_id = spawned["id"]
        .as_str()
        .expect("spawned id is a string")
        .to_string();
    assert_eq!(spawned["name"], "journey-catalog");
    assert_eq!(spawned["status"], "planned", "spawned intent is `planned`");

    // Contract: the spawned node carries source proposal/item metadata in body.
    let spawned_body = spawned
        .get("body")
        .and_then(|b| b.as_object())
        .expect("spawned intent has a `body` object");
    // Field-name-agnostic: the contract only requires the spawned body record
    // source proposal/item metadata. The proposal id is the most stable
    // identifier, so check it surfaces anywhere in the serialized body rather
    // than under a fixed key — an implementation may store the item number as
    // a number (`1`) or string (`"1"`), so relying on the id alone avoids that.
    let body_str = serde_json::to_string(&spawned_body).unwrap_or_default();
    assert!(
        body_str.contains(&proposal_id),
        "spawned body records the source proposal id (found: {body_str})"
    );

    // Contract: `show` returns the adopted item and the spawned ref.
    let show = loom_json_out(tmp.path(), &["proposal", "show", &proposal_id, "--json"]);
    let show_obj = show.as_object().expect("show --json is a JSON object");
    assert_eq!(show_obj["id"], proposal_id);
    let items = show_obj["body"]["items"]
        .as_array()
        .expect("show --json lists `body.items`");
    assert_eq!(items.len(), 1, "show lists the one item");
    assert_eq!(items[0]["number"], 1);
    assert_eq!(
        items[0]["status"], "adopted",
        "show reflects the adopted item"
    );
    // Contract: show returns the adopted item and the spawned ref — verify the
    // spawned node id actually surfaces on the item, not merely that some
    // `spawned`-like key exists (which could be empty or wrong).
    let item_str = serde_json::to_string(&items[0]).unwrap_or_default();
    assert!(
        item_str.contains(&spawned_id),
        "show --json item references the spawned node id `{spawned_id}` (found: {item_str})"
    );
    let duplicate = std::process::Command::new(loom_bin())
        .arg("--graph")
        .arg(tmp.path())
        .args([
            "proposal",
            "item",
            "adopt",
            &proposal_id,
            "1",
            "--as",
            "intent",
            "--name",
            "duplicate-journey-catalog",
            "--json",
        ])
        .output()
        .expect("spawn duplicate proposal adopt");
    assert!(
        !duplicate.status.success(),
        "re-adopting an already adopted proposal item must fail instead of spawning duplicate work"
    );
}

#[test]
fn proposal_item_reject_marks_item_and_defer_records_reason() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("prop-reject"));

    let added = loom_json_out(
        tmp.path(),
        &[
            "proposal",
            "add",
            "--title",
            "Catalog Rejected",
            "--text",
            "A proposal whose items get rejected and deferred.",
            "--json",
        ],
    );
    let proposal_id = added["id"]
        .as_str()
        .expect("proposal add --json emits an `id`")
        .to_string();

    // Two items: one to defer, one to reject.
    loom_json_out(
        tmp.path(),
        &[
            "proposal",
            "item",
            "add",
            &proposal_id,
            "--text",
            "deferred candidate",
            "--json",
        ],
    );
    loom_json_out(
        tmp.path(),
        &[
            "proposal",
            "item",
            "add",
            &proposal_id,
            "--text",
            "rejected candidate",
            "--json",
        ],
    );

    // Contract: defer records a reason and updates item status.
    let defer = loom_json_out(
        tmp.path(),
        &[
            "proposal",
            "item",
            "defer",
            &proposal_id,
            "1",
            "--reason",
            "needs design before it can be adopted",
            "--json",
        ],
    );
    let defer_item = defer
        .get("item")
        .and_then(|i| i.as_object())
        .expect("defer --json includes the updated `item`");
    assert_eq!(defer_item["number"], 1);
    assert_eq!(
        defer_item["status"], "deferred",
        "defer sets the item status to `deferred`"
    );

    let overwrite_deferred = std::process::Command::new(loom_bin())
        .arg("--graph")
        .arg(tmp.path())
        .args([
            "proposal",
            "item",
            "reject",
            &proposal_id,
            "1",
            "--reason",
            "should not overwrite a deferred decision",
            "--json",
        ])
        .output()
        .expect("spawn overwrite deferred proposal item");
    assert!(
        !overwrite_deferred.status.success(),
        "terminal proposal item decisions must be one-way in the MVP"
    );

    // Contract: reject records a reason and updates item status.
    let reject = loom_json_out(
        tmp.path(),
        &[
            "proposal",
            "item",
            "reject",
            &proposal_id,
            "2",
            "--reason",
            "out of scope for this milestone",
            "--json",
        ],
    );
    let reject_item = reject
        .get("item")
        .and_then(|i| i.as_object())
        .expect("reject --json includes the updated `item`");
    assert_eq!(reject_item["number"], 2);
    assert_eq!(
        reject_item["status"], "rejected",
        "reject sets the item status to `rejected`"
    );

    // Contract: show reflects both terminal items.
    let show = loom_json_out(tmp.path(), &["proposal", "show", &proposal_id, "--json"]);
    let items = show["body"]["items"]
        .as_array()
        .expect("show --json lists `body.items`");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["status"], "deferred");
    assert_eq!(items[1]["status"], "rejected");
}

// ---- JourneyProof metadata: validation add round-trips through show --json ----
//
// `loom validation add` accepts optional JourneyProof metadata flags
// (--proof-level, --proof-kind, --journey-id, --repo-native-kind, --artifact)
// and stores them in the Validation body JSON. `loom validation show --json`
// must echo them back — a regression that drops a flag or misnames a key
// reddens this. Drives the compiled binary end-to-end.

#[test]
fn validation_add_journey_metadata_round_trips_in_show_json() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "register a person",
            "--description",
            "demo",
            "--level",
            "feature",
            "--lifecycle",
            "implemented",
        ],
    );
    loom_ok(
        tmp.path(),
        &[
            "validation",
            "add",
            "--name",
            "journey-proof-http",
            "--type",
            "contract",
            "--command",
            "loom journey run sample-service-http.contract.json",
            "--intent",
            "register a person",
            "--proof-level",
            "L5",
            "--proof-kind",
            "journey",
            "--journey-id",
            "sample-service-http",
            "--repo-native-kind",
            "http_contract_json",
            "--artifact",
            "sample-service-http.contract.json",
        ],
    );

    let v = loom_json_out(
        tmp.path(),
        &["validation", "show", "journey-proof-http", "--json"],
    );
    assert_eq!(v["name"], "journey-proof-http");
    let body = &v["body"];
    // Every metadata flag lands in the stored body with the exact value passed.
    assert_eq!(body["proof_level"], "L5", "proof_level stored: {body}");
    assert_eq!(body["proof_kind"], "journey", "proof_kind stored: {body}");
    assert_eq!(
        body["journey_id"], "sample-service-http",
        "journey_id stored: {body}"
    );
    assert_eq!(
        body["repo_native_kind"], "http_contract_json",
        "repo_native_kind stored: {body}"
    );
    assert_eq!(
        body["artifact"], "sample-service-http.contract.json",
        "artifact stored: {body}"
    );
    // the baseline command/type are preserved alongside the metadata
    assert_eq!(body["type"], "contract");
    assert_eq!(
        body["command"],
        "loom journey run sample-service-http.contract.json"
    );
}

/// Contract: a validation added WITHOUT journey metadata must not synthesize
/// JourneyProof keys — the metadata is opt-in, not defaulted. A regression that
/// always stamps `proof_level` would redden this.
#[test]
fn validation_add_without_metadata_has_no_journey_keys() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "plain intent",
            "--description",
            "demo",
            "--level",
            "feature",
            "--lifecycle",
            "implemented",
        ],
    );
    loom_ok(
        tmp.path(),
        &[
            "validation",
            "add",
            "--name",
            "plain-proof",
            "--type",
            "test",
            "--command",
            "true",
            "--intent",
            "plain intent",
        ],
    );
    let v = loom_json_out(tmp.path(), &["validation", "show", "plain-proof", "--json"]);
    let body = &v["body"];
    for key in [
        "proof_level",
        "proof_kind",
        "journey_id",
        "repo_native_kind",
        "artifact",
    ] {
        assert!(
            body.get(key).map(|x| x.is_null()).unwrap_or(true),
            "no journey metadata key `{key}` should be present: {body}"
        );
    }
}

// ---- journey add on an HTTP contract writes JourneyProof metadata ----------
//
// `loom journey add <contract.json>` must create a journey Validation whose body
// carries repo-agnostic JourneyProof metadata (proof_level L5, proof_kind
// journey, a journey_id, repo_native_kind, artifact) — and must link the
// route intents it can resolve. No grid-specific names appear anywhere.
// Drives the compiled binary end-to-end and reads the stored node back.

#[test]
fn journey_add_http_contract_creates_validation_with_journey_metadata() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));
    // the route intents the contract declares
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "register a person",
            "--description",
            "demo",
            "--level",
            "feature",
            "--lifecycle",
            "implemented",
        ],
    );
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "fetch the person record",
            "--description",
            "demo",
            "--level",
            "feature",
            "--lifecycle",
            "implemented",
        ],
    );

    let spec = tmp.path().join("sample-service-http.contract.json");
    std::fs::write(
        &spec,
        serde_json::json!({
            "name": "sample-service-http",
            "base": "http://127.0.0.1:0",
            "routes": [
                {
                    "method": "POST",
                    "path": "/v1/example/persons",
                    "intent": "register a person",
                    "success_status": 201,
                    "extract": [{ "field": "person_id", "as": "person_id" }],
                    "response_fields": ["person_id"]
                },
                {
                    "method": "GET",
                    "path": "/v1/example/persons/{{ person_id }}",
                    "intent": "fetch the person record",
                    "success_status": 200,
                    "response_fields": ["person_id"]
                }
            ]
        })
        .to_string(),
    )
    .unwrap();

    let added = loom_json_out(
        tmp.path(),
        &["journey", "add", spec.to_str().unwrap(), "--json"],
    );
    assert_eq!(added["added"], true);
    assert_eq!(
        added["linked_steps"], 2,
        "both resolvable route intents linked: {added}"
    );

    // The journey Validation node carries the JourneyProof metadata contract.
    let store = Store::open(tmp.path()).unwrap();
    let journey = store
        .resolve_node("sample-service-http", Some(NodeType::Validation))
        .unwrap();
    assert_eq!(
        journey.body.get("type").and_then(|t| t.as_str()),
        Some("journey"),
        "journey node body type is `journey`: {}",
        journey.body
    );
    assert_eq!(
        journey.body.get("proof_level").and_then(|v| v.as_str()),
        Some("L5"),
        "proof_level L5 stamped: {}",
        journey.body
    );
    assert_eq!(
        journey.body.get("proof_kind").and_then(|v| v.as_str()),
        Some("journey"),
        "proof_kind journey stamped: {}",
        journey.body
    );
    assert_eq!(
        journey.body.get("journey_id").and_then(|v| v.as_str()),
        Some("sample-service-http"),
        "journey_id is the contract name: {}",
        journey.body
    );
    assert_eq!(
        journey
            .body
            .get("repo_native_kind")
            .and_then(|v| v.as_str()),
        Some("http_contract_json"),
        "repo_native_kind marks the HTTP contract: {}",
        journey.body
    );
    let artifact = journey
        .body
        .get("artifact")
        .and_then(|v| v.as_str())
        .expect("artifact present");
    assert!(
        artifact.ends_with("sample-service-http.contract.json"),
        "artifact points at the spec path: {artifact}"
    );

    // both route intents are validates-linked
    let validates = store
        .edges_with(Some(EdgeKind::Validates), Some(&journey.id), None)
        .unwrap();
    assert_eq!(validates.len(), 2, "both routes linked: {validates:?}");
    // journey add does NOT order routes with sequence edges
    let first = store
        .resolve_node("register a person", Some(NodeType::Intent))
        .unwrap();
    let seq = store
        .edges_with(Some(EdgeKind::Sequence), Some(&first.id), None)
        .unwrap();
    assert!(
        seq.is_empty(),
        "journey add must not auto-create sequence edges"
    );
}

// ---- journey coverage: derived status (single truth source) ---------------

/// A coverage node starts uncovered; `coverage list --json` reports
/// effective_status derived from the linked intent's validations.
#[test]
fn journey_coverage_starts_uncovered_without_journey_proof() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "checkout completes",
            "--lifecycle",
            "implemented",
        ],
    );
    loom_ok(
        tmp.path(),
        &[
            "journey",
            "coverage",
            "add",
            "--name",
            "checkout flow",
            "--flow",
            "src/api.rs::post -> record -> standing",
            "--description",
            "core trust ingress",
            "checkout completes",
        ],
    );
    let v = loom_json_out(tmp.path(), &["journey", "coverage", "list", "--json"]);
    let row = v
        .as_array()
        .expect("coverage list is an array")
        .first()
        .expect("one row");
    assert_eq!(row["name"], "checkout flow");
    assert_eq!(row["effective_status"], "uncovered");
    assert_eq!(row["covers"], "checkout completes");
}

/// Coverage status is DERIVED from a passing L5 journey proof on the covered
/// intent — and flips back to uncovered when the proof is staled by sync. This
/// is the single-truth-source contract: no second asserted "covered" claim.
#[test]
fn journey_coverage_status_derived_from_journey_proof_and_stales_with_it() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "checkout completes",
            "--lifecycle",
            "implemented",
        ],
    );
    // artifact-backed journey proof
    std::fs::create_dir_all(tmp.path().join("contracts")).unwrap();
    std::fs::write(
        tmp.path().join("contracts/checkout.v1.json"),
        r#"{"routes":[]}"#,
    )
    .unwrap();
    loom_ok(
        tmp.path(),
        &[
            "validation",
            "add",
            "--name",
            "checkout journey",
            "--type",
            "journey",
            "--command",
            "loom journey run checkout",
            "--intent",
            "checkout completes",
            "--proof-level",
            "L5",
            "--proof-kind",
            "journey",
            "--artifact",
            "contracts/checkout.v1.json",
        ],
    );
    loom_ok(
        tmp.path(),
        &[
            "journey",
            "coverage",
            "add",
            "--name",
            "checkout flow",
            "--flow",
            "f",
            "checkout completes",
        ],
    );
    // mark the validation passed via the CLI
    loom_ok(
        tmp.path(),
        &[
            "validation",
            "verdict",
            "checkout journey",
            "passed",
            "--evidence",
            "journey green",
        ],
    );

    // before sync establishes the artifact hash, the edge is still uninspected
    // → still uncovered. Run sync to baseline, then record the verdict on the edge.
    loom_ok(tmp.path(), &["sync"]);
    // The validation verdict set node status=passed, but the Validates edge needs a
    // Passing verdict to count as a current proof. Use the store directly to
    // stamp the edge (mirrors what `loom journey run` does on a green run).
    {
        use loom::model::{EdgeKind, InspectionStatus, NodeType};
        use loom::store::Store;
        let store = Store::open(tmp.path()).unwrap();
        let val = store
            .resolve_node("checkout journey", Some(NodeType::Validation))
            .unwrap();
        let intent = store
            .resolve_node("checkout completes", Some(NodeType::Intent))
            .unwrap();
        let e = store
            .edges_with(Some(EdgeKind::Validates), Some(&val.id), Some(&intent.id))
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        store
            .record_verdict(
                &e.id,
                InspectionStatus::Passing,
                "journey green",
                "journey passed",
                0.9,
                "test",
            )
            .unwrap();
    }

    let covered = loom_json_out(tmp.path(), &["journey", "coverage", "list", "--json"]);
    let row = covered.as_array().unwrap().first().unwrap();
    assert_eq!(
        row["effective_status"], "covered",
        "a passing L5 journey proof must derive coverage=covered: {row}"
    );

    // Artifact drifts → sync stales the proof → coverage reads uncovered again.
    std::fs::write(
        tmp.path().join("contracts/checkout.v1.json"),
        r#"{"routes":[{"path":"/x"}]}"#,
    )
    .unwrap();
    loom_ok(tmp.path(), &["sync"]);
    let drifted = loom_json_out(tmp.path(), &["journey", "coverage", "list", "--json"]);
    let row = drifted.as_array().unwrap().first().unwrap();
    assert_eq!(
        row["effective_status"], "uncovered",
        "a staled journey proof must flip coverage back to uncovered (single truth source): {row}"
    );
}

/// A non-L5 (too shallow) proof does NOT cover, even when passing — coverage
/// mirrors the journey smell's L5+ threshold.
#[test]
fn journey_coverage_requires_l5_plus_proof_not_just_any_passing_validation() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "checkout completes",
            "--lifecycle",
            "implemented",
        ],
    );
    loom_ok(
        tmp.path(),
        &[
            "validation",
            "add",
            "--name",
            "unit checkout",
            "--type",
            "test",
            "--command",
            "cargo test checkout",
            "--intent",
            "checkout completes",
            "--proof-level",
            "L1",
            "--proof-kind",
            "unit",
        ],
    );
    loom_ok(
        tmp.path(),
        &[
            "journey",
            "coverage",
            "add",
            "--name",
            "checkout flow",
            "--flow",
            "f",
            "checkout completes",
        ],
    );
    loom_ok(
        tmp.path(),
        &[
            "validation",
            "verdict",
            "unit checkout",
            "passed",
            "--evidence",
            "unit green",
        ],
    );
    {
        use loom::model::{EdgeKind, InspectionStatus, NodeType};
        use loom::store::Store;
        let store = Store::open(tmp.path()).unwrap();
        let val = store
            .resolve_node("unit checkout", Some(NodeType::Validation))
            .unwrap();
        let intent = store
            .resolve_node("checkout completes", Some(NodeType::Intent))
            .unwrap();
        let e = store
            .edges_with(Some(EdgeKind::Validates), Some(&val.id), Some(&intent.id))
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        store
            .record_verdict(
                &e.id,
                InspectionStatus::Passing,
                "unit green",
                "test passed",
                0.9,
                "test",
            )
            .unwrap();
    }
    let v = loom_json_out(tmp.path(), &["journey", "coverage", "list", "--json"]);
    let row = v.as_array().unwrap().first().unwrap();
    assert_eq!(
        row["effective_status"], "uncovered",
        "an L1 unit proof must NOT cover a journey: {row}"
    );
}

/// Journey invariant points record their asserted invariant + link to an intent.
#[test]
fn journey_invariant_point_links_to_intent_and_lists() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "compute standing",
            "--lifecycle",
            "implemented",
        ],
    );
    loom_ok(
        tmp.path(),
        &[
            "journey",
            "invariant",
            "add",
            "--name",
            "standing threshold",
            "compute standing",
            "--field",
            "headline",
            "--assertion",
            "headline > 1.0 when voucher_count >= 1",
            "--reason",
            "trust math not serialized in HTTP",
        ],
    );
    let v = loom_json_out(tmp.path(), &["journey", "invariant", "list", "--json"]);
    let row = v.as_array().unwrap().first().unwrap();
    assert_eq!(row["name"], "standing threshold");
    assert_eq!(row["field"], "headline");
    assert_eq!(row["assertion"], "headline > 1.0 when voucher_count >= 1");
    assert_eq!(row["asserts"], "compute standing");
}

/// `journey coverage discover` surfaces user-visible implemented intents with
/// no passing L5 journey proof and no coverage node; --spawn-missing creates
/// one per gap. Graph-derived, not static call-graph analysis.
#[test]
fn journey_coverage_discover_finds_and_spawns_gaps() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));
    // two user-visible implemented intents; one internal; one planned.
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "checkout completes",
            "--lifecycle",
            "implemented",
            "--visibility",
            "user_visible",
        ],
    );
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "search returns results",
            "--lifecycle",
            "implemented",
            "--visibility",
            "user_visible",
        ],
    );
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "index rebuild",
            "--lifecycle",
            "implemented",
            "--visibility",
            "internal",
        ],
    );
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "future feature",
            "--lifecycle",
            "planned",
            "--visibility",
            "user_visible",
        ],
    );

    let gaps = loom_json_out(tmp.path(), &["journey", "coverage", "discover", "--json"]);
    let gap_names = gaps["gaps"].as_array().unwrap();
    assert_eq!(
        gaps["gap_count"], 2,
        "two user-visible implemented gaps: {gaps}"
    );
    let names: Vec<String> = gap_names
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"checkout completes".to_string()));
    assert!(names.contains(&"search returns results".to_string()));
    assert!(
        !names.contains(&"index rebuild".to_string()),
        "internal intents are not gaps"
    );
    assert!(
        !names.contains(&"future feature".to_string()),
        "planned intents are not gaps"
    );

    // spawn-missing creates coverage nodes for each gap.
    let spawned = loom_json_out(
        tmp.path(),
        &[
            "journey",
            "coverage",
            "discover",
            "--spawn-missing",
            "--json",
        ],
    );
    assert_eq!(
        spawned["spawned_count"], 2,
        "two coverage nodes spawned: {spawned}"
    );

    // re-discover: the spawned intents are now covered by a node → no gaps.
    let again = loom_json_out(tmp.path(), &["journey", "coverage", "discover", "--json"]);
    assert_eq!(
        again["gap_count"], 0,
        "spawned coverage nodes remove the gaps: {again}"
    );
}

/// `journey prompt` emits typed-runner prompt context from graph knowledge:
/// intent meaning, implementing modules/locators, coverage flows, and invariant
/// markers. It does not generate code; it packages context for an on-site LLM.
#[test]
fn journey_prompt_emits_context_from_graph() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(tmp.path().join("src/checkout.rs"), "pub fn checkout() {}\n").unwrap();
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "checkout completes",
            "--description",
            "buyer can pay and see confirmation",
            "--lifecycle",
            "implemented",
        ],
    );
    loom_ok(tmp.path(), &["codefile", "add", "src/checkout.rs"]);
    loom_ok(
        tmp.path(),
        &[
            "edge",
            "implement",
            "checkout completes",
            "src/checkout.rs",
            "--locator",
            "fn checkout",
        ],
    );
    loom_ok(
        tmp.path(),
        &[
            "journey",
            "coverage",
            "add",
            "--name",
            "checkout flow",
            "--flow",
            "src/checkout.rs::checkout",
            "checkout completes",
        ],
    );
    loom_ok(
        tmp.path(),
        &[
            "journey",
            "invariant",
            "add",
            "--name",
            "paid order visible",
            "checkout completes",
            "--field",
            "order.status",
            "--assertion",
            "status == paid",
            "--reason",
            "payment mutation must project",
        ],
    );

    let v = loom_json_out(
        tmp.path(),
        &["journey", "prompt", "checkout completes", "--json"],
    );
    assert_eq!(v["intent"]["name"], "checkout completes");
    assert_eq!(v["modules"][0]["path"], "src/checkout.rs");
    assert_eq!(v["modules"][0]["locator"], "fn checkout");
    assert_eq!(v["flows"][0]["flow"], "src/checkout.rs::checkout");
    assert_eq!(v["invariant_points"][0]["field"], "order.status");
    assert_eq!(v["invariant_points"][0]["assertion"], "status == paid");
    assert!(v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| { r.as_str().unwrap().contains("Assert internal domain state") }));
}

/// Drift enforcement is clean when a covered journey has an existing contract
/// artifact and configured runner/test refs that resolve on disk.
#[test]
fn journey_coverage_drift_clean_when_artifact_runner_and_test_match() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));
    std::fs::create_dir_all(tmp.path().join("contracts")).unwrap();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::create_dir_all(tmp.path().join("tests")).unwrap();
    std::fs::write(
        tmp.path().join("contracts/checkout.v1.json"),
        r#"{"routes":[]}"#,
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("src/journey_runner.rs"),
        "pub fn checkout_runner() {}\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("tests/journey_runner.rs"),
        "fn checkout_runner_test() {}\n",
    )
    .unwrap();
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "checkout completes",
            "--lifecycle",
            "implemented",
        ],
    );
    loom_ok(
        tmp.path(),
        &[
            "validation",
            "add",
            "--name",
            "checkout journey",
            "--type",
            "journey",
            "--command",
            "cargo test checkout_runner_test",
            "--intent",
            "checkout completes",
            "--proof-level",
            "L5",
            "--proof-kind",
            "journey",
            "--artifact",
            "contracts/checkout.v1.json",
        ],
    );
    loom_ok(
        tmp.path(),
        &[
            "journey",
            "coverage",
            "add",
            "--name",
            "checkout flow",
            "--flow",
            "src/journey_runner.rs::checkout_runner",
            "--runner-ref",
            "src/journey_runner.rs::checkout_runner",
            "--test-ref",
            "tests/journey_runner.rs::checkout_runner_test",
            "--contract-artifact",
            "contracts/checkout.v1.json",
            "checkout completes",
        ],
    );
    {
        use loom::model::{EdgeKind, InspectionStatus, NodeType};
        use loom::store::Store;
        let store = Store::open(tmp.path()).unwrap();
        let val = store
            .resolve_node("checkout journey", Some(NodeType::Validation))
            .unwrap();
        let intent = store
            .resolve_node("checkout completes", Some(NodeType::Intent))
            .unwrap();
        store.set_node_status(&val.id, "passed").unwrap();
        let e = store
            .edges_with(Some(EdgeKind::Validates), Some(&val.id), Some(&intent.id))
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        store
            .record_verdict(
                &e.id,
                InspectionStatus::Passing,
                "journey green",
                "journey passed",
                0.9,
                "test",
            )
            .unwrap();
    }
    let findings = loom_json_out(tmp.path(), &["journey", "coverage", "drift", "--json"]);
    assert_eq!(
        findings.as_array().unwrap().len(),
        0,
        "drift clean: {findings}"
    );
}

/// Declared typed runner/test refs are checked even when the coverage is still
/// uncovered; ref drift is independent of proof availability.
#[test]
fn journey_coverage_drift_reports_declared_refs_without_current_proof() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "checkout completes",
            "--lifecycle",
            "implemented",
        ],
    );
    loom_ok(
        tmp.path(),
        &[
            "journey",
            "coverage",
            "add",
            "--name",
            "checkout flow",
            "--flow",
            "f",
            "--runner-ref",
            "src/missing_runner.rs::checkout_runner",
            "--test-ref",
            "tests/missing_runner.rs::checkout_runner_test",
            "checkout completes",
        ],
    );
    let findings = loom_json_err(tmp.path(), &["journey", "coverage", "drift", "--json"]);
    let kinds: Vec<&str> = findings
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"journey_runner_ref_missing"));
    assert!(kinds.contains(&"journey_test_ref_missing"));
}

/// A coverage node's configured contract artifact must match a current passing
/// L5 journey proof artifact. Mismatch is a drift failure.
#[test]
fn journey_coverage_drift_reports_contract_artifact_mismatch() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));
    std::fs::create_dir_all(tmp.path().join("contracts")).unwrap();
    std::fs::write(
        tmp.path().join("contracts/proof.v1.json"),
        r#"{"routes":[]}"#,
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("contracts/expected.v1.json"),
        r#"{"routes":[]}"#,
    )
    .unwrap();
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "checkout completes",
            "--lifecycle",
            "implemented",
        ],
    );
    loom_ok(
        tmp.path(),
        &[
            "validation",
            "add",
            "--name",
            "checkout journey",
            "--type",
            "journey",
            "--command",
            "loom journey run proof",
            "--intent",
            "checkout completes",
            "--proof-level",
            "L5",
            "--proof-kind",
            "journey",
            "--artifact",
            "contracts/proof.v1.json",
        ],
    );
    loom_ok(
        tmp.path(),
        &[
            "journey",
            "coverage",
            "add",
            "--name",
            "checkout flow",
            "--flow",
            "f",
            "--contract-artifact",
            "contracts/expected.v1.json",
            "checkout completes",
        ],
    );
    {
        use loom::model::{EdgeKind, InspectionStatus, NodeType};
        use loom::store::Store;
        let store = Store::open(tmp.path()).unwrap();
        let val = store
            .resolve_node("checkout journey", Some(NodeType::Validation))
            .unwrap();
        let intent = store
            .resolve_node("checkout completes", Some(NodeType::Intent))
            .unwrap();
        store.set_node_status(&val.id, "passed").unwrap();
        let e = store
            .edges_with(Some(EdgeKind::Validates), Some(&val.id), Some(&intent.id))
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        store
            .record_verdict(
                &e.id,
                InspectionStatus::Passing,
                "journey green",
                "journey passed",
                0.9,
                "test",
            )
            .unwrap();
    }
    let findings = loom_json_err(tmp.path(), &["journey", "coverage", "drift", "--json"]);
    let arr = findings.as_array().unwrap();
    assert_eq!(arr[0]["kind"], "journey_contract_artifact_mismatch");
    assert_eq!(arr[0]["expected_artifact"], "contracts/expected.v1.json");
}

/// Multiple current L5 proofs should not cause false drift: when coverage names
/// contract_artifact=B and one passing proof uses A while another uses B, the
/// drift checker must select the matching proof and stay clean.
#[test]
fn journey_coverage_drift_selects_matching_artifact_among_multiple_proofs() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));
    std::fs::create_dir_all(tmp.path().join("contracts")).unwrap();
    std::fs::write(tmp.path().join("contracts/a.json"), r#"{"a":1}"#).unwrap();
    std::fs::write(tmp.path().join("contracts/b.json"), r#"{"b":1}"#).unwrap();
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "checkout completes",
            "--lifecycle",
            "implemented",
        ],
    );
    for (name, artifact) in [
        ("journey a", "contracts/a.json"),
        ("journey b", "contracts/b.json"),
    ] {
        loom_ok(
            tmp.path(),
            &[
                "validation",
                "add",
                "--name",
                name,
                "--type",
                "journey",
                "--command",
                "loom journey run",
                "--intent",
                "checkout completes",
                "--proof-level",
                "L5",
                "--proof-kind",
                "journey",
                "--artifact",
                artifact,
            ],
        );
    }
    loom_ok(
        tmp.path(),
        &[
            "journey",
            "coverage",
            "add",
            "--name",
            "checkout flow",
            "--flow",
            "f",
            "--contract-artifact",
            "contracts/b.json",
            "checkout completes",
        ],
    );
    {
        use loom::model::{EdgeKind, InspectionStatus, NodeType};
        use loom::store::Store;
        let store = Store::open(tmp.path()).unwrap();
        let intent = store
            .resolve_node("checkout completes", Some(NodeType::Intent))
            .unwrap();
        for name in ["journey a", "journey b"] {
            let val = store
                .resolve_node(name, Some(NodeType::Validation))
                .unwrap();
            store.set_node_status(&val.id, "passed").unwrap();
            let e = store
                .edges_with(Some(EdgeKind::Validates), Some(&val.id), Some(&intent.id))
                .unwrap()
                .into_iter()
                .next()
                .unwrap();
            store
                .record_verdict(
                    &e.id,
                    InspectionStatus::Passing,
                    "journey green",
                    "journey passed",
                    0.9,
                    "test",
                )
                .unwrap();
        }
    }
    let findings = loom_json_out(tmp.path(), &["journey", "coverage", "drift", "--json"]);
    assert_eq!(
        findings.as_array().unwrap().len(),
        0,
        "matching proof must avoid false drift: {findings}"
    );
}

/// Self-healing (slice 2): editing a covered journey's `runner_ref` source
/// after the proof passed stales that journey proof on the next `loom sync`,
/// resetting it to `not_run` so the flow re-enters the validate queue and
/// coverage flips back to uncovered. The compiler/test suite catches the
/// breakage; loom's job is to make the stale proof tracked work again.
#[test]
fn sync_stales_journey_proof_when_runner_ref_source_changes() {
    use loom::model::{EdgeKind, InspectionStatus, NodeType};
    use loom::store::Store;

    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::create_dir_all(tmp.path().join("contracts")).unwrap();
    std::fs::write(
        tmp.path().join("src/journey_runner.rs"),
        "pub fn checkout_runner() { /* v1 */ }\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("contracts/checkout.v1.json"),
        r#"{"routes":[]}"#,
    )
    .unwrap();
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "checkout completes",
            "--lifecycle",
            "implemented",
            "--visibility",
            "user_visible",
        ],
    );
    loom_ok(
        tmp.path(),
        &[
            "validation",
            "add",
            "--name",
            "checkout journey",
            "--type",
            "journey",
            "--command",
            "cargo test checkout_runner_test",
            "--intent",
            "checkout completes",
            "--proof-level",
            "L5",
            "--proof-kind",
            "journey",
            "--artifact",
            "contracts/checkout.v1.json",
        ],
    );
    loom_ok(
        tmp.path(),
        &[
            "journey",
            "coverage",
            "add",
            "--name",
            "checkout flow",
            "--flow",
            "src/journey_runner.rs::checkout_runner",
            "--runner-ref",
            "src/journey_runner.rs::checkout_runner",
            "--contract-artifact",
            "contracts/checkout.v1.json",
            "checkout completes",
        ],
    );
    // Make the proof pass.
    {
        let store = Store::open(tmp.path()).unwrap();
        let val = store
            .resolve_node("checkout journey", Some(NodeType::Validation))
            .unwrap();
        let intent = store
            .resolve_node("checkout completes", Some(NodeType::Intent))
            .unwrap();
        store.set_node_status(&val.id, "passed").unwrap();
        let e = store
            .edges_with(Some(EdgeKind::Validates), Some(&val.id), Some(&intent.id))
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        store
            .record_verdict(
                &e.id,
                InspectionStatus::Passing,
                "journey green",
                "journey passed",
                0.9,
                "test",
            )
            .unwrap();
    }
    // First sync SEEDS the runner_ref hash — it must NOT stale the fresh proof.
    loom_ok(tmp.path(), &["sync"]);
    {
        let store = Store::open(tmp.path()).unwrap();
        let val = store
            .resolve_node("checkout journey", Some(NodeType::Validation))
            .unwrap();
        assert_eq!(
            val.status, "passed",
            "seeding sync must not stale the proof"
        );
    }
    // Edit the runner source → next sync must stale the proof.
    std::fs::write(
        tmp.path().join("src/journey_runner.rs"),
        "pub fn checkout_runner() { /* v2 — added a field */ }\n",
    )
    .unwrap();
    loom_ok(tmp.path(), &["sync"]);
    {
        let store = Store::open(tmp.path()).unwrap();
        let val = store
            .resolve_node("checkout journey", Some(NodeType::Validation))
            .unwrap();
        assert_eq!(
            val.status, "not_run",
            "editing the runner_ref source must reset the journey proof"
        );
    }
    // Coverage now reads uncovered (derived from the reset proof).
    let rows = loom_json_out(tmp.path(), &["journey", "coverage", "list", "--json"]);
    assert_eq!(
        rows[0]["effective_status"], "uncovered",
        "coverage flips to uncovered after runner drift: {rows}"
    );
}

/// Signal-fed prompt (slice 1): when the intent's grounded code imports an
/// infra crate (sqlx), `journey prompt` reports it in `signals.infra_hints`
/// and emits the "needs infrastructure" rule — instead of blindly assuming an
/// in-process typed runner. `grounded` reflects IMPLEMENTS modules, not imports.
#[test]
fn journey_prompt_signals_flag_infra_and_condition_rules() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(
        tmp.path().join("src/repo.rs"),
        "use sqlx::PgPool;\npub fn load(pool: &PgPool) {}\n",
    )
    .unwrap();
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "orders persist to the database",
            "--lifecycle",
            "implemented",
            "--visibility",
            "user_visible",
        ],
    );
    loom_ok(tmp.path(), &["codefile", "add", "src/repo.rs"]);
    loom_ok(
        tmp.path(),
        &[
            "edge",
            "implement",
            "orders persist to the database",
            "src/repo.rs",
            "--locator",
            "fn load",
        ],
    );
    // sync so the codefile's `imports` facet is extracted.
    loom_ok(tmp.path(), &["sync"]);

    let v = loom_json_out(
        tmp.path(),
        &[
            "journey",
            "prompt",
            "orders persist to the database",
            "--json",
        ],
    );
    assert_eq!(
        v["signals"]["grounded"], true,
        "grounded via IMPLEMENTS: {v}"
    );
    let infra = v["signals"]["infra_hints"].as_array().unwrap();
    assert!(
        infra.iter().any(|h| h["capability"] == "database"),
        "sqlx import flagged as database infra: {infra:?}"
    );
    let rules = v["rules"].as_array().unwrap();
    assert!(
        rules
            .iter()
            .any(|r| r.as_str().unwrap().contains("needs infrastructure")),
        "infra hint emits the needs-infrastructure rule: {rules:?}"
    );
}

/// Signal-fed prompt (slice 1): an intent with no code grounding must NOT get
/// the in-process typed-runner rules; it should be steered to a consumer-facing
/// HTTP/journey proof instead.
#[test]
fn journey_prompt_ungrounded_intent_steers_to_http_proof() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "external billing settles",
            "--lifecycle",
            "implemented",
            "--visibility",
            "user_visible",
        ],
    );

    let v = loom_json_out(
        tmp.path(),
        &["journey", "prompt", "external billing settles", "--json"],
    );
    assert_eq!(v["signals"]["grounded"], false, "no modules: {v}");
    let rules = v["rules"].as_array().unwrap();
    assert!(
        rules
            .iter()
            .any(|r| r.as_str().unwrap().contains("no in-process code grounding")),
        "ungrounded intent is steered to an HTTP/journey proof: {rules:?}"
    );
    assert!(
        !rules
            .iter()
            .any(|r| r.as_str().unwrap().contains("actual domain types")),
        "ungrounded intent must NOT get the in-process typed-runner rule: {rules:?}"
    );
}

/// Self-healing narrowing (slice 2): when an intent has two passing L5 journey
/// proofs (artifacts A and B) and a coverage declares `contract_artifact=B`,
/// editing that coverage's runner_ref must stale ONLY the B proof — the sibling
/// A proof stays passed. Guards the over-stale bug the artifact match fixes.
#[test]
fn sync_runner_drift_stales_only_the_artifact_matched_proof() {
    use loom::model::{EdgeKind, InspectionStatus, NodeType};
    use loom::store::Store;

    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::create_dir_all(tmp.path().join("contracts")).unwrap();
    std::fs::write(
        tmp.path().join("src/runner_b.rs"),
        "pub fn runner_b() { /* v1 */ }\n",
    )
    .unwrap();
    std::fs::write(tmp.path().join("contracts/a.json"), r#"{"a":1}"#).unwrap();
    std::fs::write(tmp.path().join("contracts/b.json"), r#"{"b":1}"#).unwrap();
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "checkout completes",
            "--lifecycle",
            "implemented",
            "--visibility",
            "user_visible",
        ],
    );
    for (name, artifact) in [
        ("journey a", "contracts/a.json"),
        ("journey b", "contracts/b.json"),
    ] {
        loom_ok(
            tmp.path(),
            &[
                "validation",
                "add",
                "--name",
                name,
                "--type",
                "journey",
                "--command",
                "loom journey run",
                "--intent",
                "checkout completes",
                "--proof-level",
                "L5",
                "--proof-kind",
                "journey",
                "--artifact",
                artifact,
            ],
        );
    }
    // Coverage stands behind the B proof, with a runner_ref we will edit.
    loom_ok(
        tmp.path(),
        &[
            "journey",
            "coverage",
            "add",
            "--name",
            "checkout flow b",
            "--flow",
            "f",
            "--runner-ref",
            "src/runner_b.rs::runner_b",
            "--contract-artifact",
            "contracts/b.json",
            "checkout completes",
        ],
    );
    // Pass both proofs.
    {
        let store = Store::open(tmp.path()).unwrap();
        let intent = store
            .resolve_node("checkout completes", Some(NodeType::Intent))
            .unwrap();
        for name in ["journey a", "journey b"] {
            let val = store
                .resolve_node(name, Some(NodeType::Validation))
                .unwrap();
            store.set_node_status(&val.id, "passed").unwrap();
            let e = store
                .edges_with(Some(EdgeKind::Validates), Some(&val.id), Some(&intent.id))
                .unwrap()
                .into_iter()
                .next()
                .unwrap();
            store
                .record_verdict(
                    &e.id,
                    InspectionStatus::Passing,
                    "journey green",
                    "journey passed",
                    0.9,
                    "test",
                )
                .unwrap();
        }
    }
    loom_ok(tmp.path(), &["sync"]); // seed
    std::fs::write(
        tmp.path().join("src/runner_b.rs"),
        "pub fn runner_b() { /* v2 */ }\n",
    )
    .unwrap();
    loom_ok(tmp.path(), &["sync"]); // drift
    let store = Store::open(tmp.path()).unwrap();
    let a = store
        .resolve_node("journey a", Some(NodeType::Validation))
        .unwrap();
    let b = store
        .resolve_node("journey b", Some(NodeType::Validation))
        .unwrap();
    assert_eq!(a.status, "passed", "sibling A proof must stay passed");
    assert_eq!(b.status, "not_run", "artifact-matched B proof must reset");
}

// ---- wiki: reader-first docs as a tracked projection -----------------------

#[test]
fn wiki_scope_hash_tracks_documented_intent_state() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let page = store
        .add_node(NodeType::WikiPage, "P", "", "draft", serde_json::json!({}))
        .unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "x behaves",
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .ensure_edge(EdgeKind::Documents, &page.id, &intent.id)
        .unwrap();
    let h1 = loom::sync::wiki_scope_hash(&store, &page.id).unwrap();
    assert_eq!(
        h1,
        loom::sync::wiki_scope_hash(&store, &page.id).unwrap(),
        "deterministic"
    );
    // A documented intent's lifecycle change shifts the page's scope fingerprint.
    store.set_node_status(&intent.id, "needs_change").unwrap();
    assert_ne!(h1, loom::sync::wiki_scope_hash(&store, &page.id).unwrap());
}

#[test]
fn wiki_plan_record_loop_and_stale_on_documented_change() {
    let tmp = Tmp::new();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(tmp.path().join("src/w.rs"), "pub fn render(){}\n").unwrap();
    {
        let store = Store::init(tmp.path(), Some("t"), false).unwrap();
        let intent = store
            .add_node(
                NodeType::Intent,
                "the widget renders",
                "",
                "implemented",
                serde_json::json!({}),
            )
            .unwrap();
        let cf = store
            .add_node(
                NodeType::CodeFile,
                "src/w.rs",
                "",
                "",
                serde_json::json!({}),
            )
            .unwrap();
        loom::sync::run(&store, tmp.path()).unwrap(); // extract → content_hash facet
        store
            .add_edge(
                EdgeKind::Implements,
                &intent.id,
                &cf.id,
                TruthClass::Asserted,
            )
            .unwrap();
    }
    // plan → draft
    run(
        tmp.path(),
        Command::Wiki {
            cmd: WikiCmd::Plan {
                title: "Architecture".into(),
                path: "docs/a.md".into(),
                covers: vec!["the widget renders".into()],
            },
        },
    );
    {
        let store = Store::open_read(tmp.path()).unwrap();
        assert_eq!(
            store
                .resolve_node("Architecture", Some(NodeType::WikiPage))
                .unwrap()
                .status,
            "draft"
        );
    }
    // the brief path (wiki next serves the draft) must not panic
    run(tmp.path(), Command::Wiki { cmd: WikiCmd::Next });
    // record before the prose exists must fail (the freshness gate)
    assert!(
        loom::commands::run(Cli {
            graph: Some(tmp.path().to_path_buf()),
            json: false,
            command: Command::Wiki {
                cmd: WikiCmd::Record {
                    title: "Architecture".into()
                },
            },
        })
        .is_err(),
        "record must fail when the page's prose is not written"
    );
    // write the prose, then record → fresh
    std::fs::create_dir_all(tmp.path().join("docs")).unwrap();
    std::fs::write(tmp.path().join("docs/a.md"), "# Architecture\n\nprose\n").unwrap();
    run(
        tmp.path(),
        Command::Wiki {
            cmd: WikiCmd::Record {
                title: "Architecture".into(),
            },
        },
    );
    {
        let store = Store::open_read(tmp.path()).unwrap();
        assert_eq!(
            store
                .resolve_node("Architecture", Some(NodeType::WikiPage))
                .unwrap()
                .status,
            "fresh"
        );
    }
    // change the documented file → sync marks the page stale
    std::fs::write(
        tmp.path().join("src/w.rs"),
        "pub fn render(){ /* changed */ }\n",
    )
    .unwrap();
    {
        let store = Store::open(tmp.path()).unwrap();
        let report = loom::sync::run(&store, tmp.path()).unwrap();
        assert_eq!(
            report.wiki_staled, 1,
            "a documented file change stales the page"
        );
        assert_eq!(
            store
                .resolve_node("Architecture", Some(NodeType::WikiPage))
                .unwrap()
                .status,
            "stale"
        );
    }
}

#[test]
fn wiki_plan_rejects_empty_covers() {
    let tmp = Tmp::new();
    Store::init(tmp.path(), Some("t"), false).unwrap();
    assert!(
        loom::commands::run(Cli {
            graph: Some(tmp.path().to_path_buf()),
            json: false,
            command: Command::Wiki {
                cmd: WikiCmd::Plan {
                    title: "P".into(),
                    path: "d.md".into(),
                    covers: vec![],
                },
            },
        })
        .is_err(),
        "a wiki page must document at least one intent"
    );
}

// ---- PoC / experiment evidence reaches the build packet --------------------
//
// These defend the wiring that carries hypothesis/task learnings into the
// `loom next --mode build` packet a coding LLM receives. Before it, evidence
// was recorded (notes on the hypothesis, task results) but unreachable at
// coding time: adopt copied only the claim, tasks were inert, and notes never
// appeared in any packet. Each test fails if its wiring is reverted.

#[test]
fn notes_for_filters_by_target_newest_first() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = store
        .add_node(
            NodeType::Intent,
            "intent a",
            "d",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    let b = store
        .add_node(
            NodeType::Intent,
            "intent b",
            "d",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    // 10ms gaps guarantee distinct millisecond timestamps (created_at is %f).
    store.add_note(&a.id, "context", "a-note-1").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    store.add_note(&a.id, "context", "a-note-2").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    store.add_note(&a.id, "decision", "a-note-3").unwrap();
    store.add_note(&b.id, "context", "b-note").unwrap();

    let notes = store.notes_for(&a.id).unwrap();
    assert_eq!(notes.len(), 3, "notes_for returns only notes on the target");
    assert!(
        notes.iter().all(|n| n.description.starts_with("a-note")),
        "no cross-target leak"
    );
    assert_eq!(notes[0].description, "a-note-3", "newest first");
    assert_eq!(notes[2].description, "a-note-1", "oldest last");

    let bn = store.notes_for(&b.id).unwrap();
    assert_eq!(bn.len(), 1, "the other target is isolated");
    assert_eq!(bn[0].description, "b-note");

    // Untargeted listing sees every note across targets.
    assert_eq!(
        store
            .list_nodes(Some(NodeType::Note), usize::MAX)
            .unwrap()
            .len(),
        4,
        "all four notes exist graph-wide"
    );
}

#[test]
fn adopt_copies_experiment_record_to_spawned_intent() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("adopt-rec"));
    loom_json_out(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "checkout",
            "--description",
            "user buys",
            "--level",
            "feature",
            "--lifecycle",
            "planned",
            "--json",
        ],
    );
    loom_json_out(
        tmp.path(),
        &[
            "hypothesis",
            "add",
            "--name",
            "PoC batch",
            "--claim",
            "writes slow",
            "--proposal",
            "BATCHPROP batch per txn",
            "--predicted-outcome",
            "PREDOUT 50pct fewer fsyncs",
            "--target",
            "checkout",
            "--json",
        ],
    );
    loom_json_out(
        tmp.path(),
        &[
            "hypothesis",
            "prove",
            "PoC batch",
            "supported",
            "--evidence",
            "EVIDENCE 1.9ms to 0.8ms bench",
            "--json",
        ],
    );
    loom_json_out(
        tmp.path(),
        &[
            "hypothesis",
            "adopt",
            "PoC batch",
            "--spawned",
            "batch writes",
            "--json",
        ],
    );

    // The spawned intent (not just the hypothesis) carries proposal +
    // prediction + evidence — none of which lived in the intent description.
    let notes = loom_json_out(tmp.path(), &["note", "list", "batch writes", "--json"]);
    let joined: String = notes
        .as_array()
        .expect("note list --json is an array")
        .iter()
        .map(|n| n["text"].as_str().unwrap_or("").to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("BATCHPROP"),
        "proposal reaches spawned intent: {joined}"
    );
    assert!(
        joined.contains("PREDOUT"),
        "prediction reaches spawned intent: {joined}"
    );
    assert!(
        joined.contains("EVIDENCE"),
        "proof evidence reaches spawned intent: {joined}"
    );
}

#[test]
fn build_packet_surfaces_adopted_evidence_note() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("build-note"));
    loom_json_out(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "checkout",
            "--description",
            "user buys",
            "--level",
            "feature",
            "--lifecycle",
            "planned",
            "--json",
        ],
    );
    loom_json_out(
        tmp.path(),
        &[
            "hypothesis",
            "add",
            "--name",
            "PoC batch",
            "--claim",
            "writes slow",
            "--proposal",
            "batch per txn",
            "--predicted-outcome",
            "fewer fsyncs",
            "--target",
            "checkout",
            "--json",
        ],
    );
    loom_json_out(
        tmp.path(),
        &[
            "hypothesis",
            "prove",
            "PoC batch",
            "supported",
            "--evidence",
            "EVIDENCE bench numbers",
            "--json",
        ],
    );
    loom_json_out(
        tmp.path(),
        &[
            "hypothesis",
            "adopt",
            "PoC batch",
            "--spawned",
            "batch writes",
            "--json",
        ],
    );

    let next = loom_json_out(tmp.path(), &["next", "--mode", "build", "--json"]);
    let wi = &next["work_item"];
    assert_eq!(
        wi["target"]["name"], "batch writes",
        "build serves the spawned intent (alphabetically before 'checkout')"
    );
    let les = wi["context"]["linked_entities"]
        .as_array()
        .expect("linked_entities is an array");
    let note = les
        .iter()
        .find(|e| e["role"] == "note")
        .expect("build packet inlines a note linked entity");
    assert!(
        note["description"]
            .as_str()
            .unwrap_or("")
            .contains("EVIDENCE"),
        "the adopted evidence reaches the build packet: {note}"
    );
}

#[test]
fn build_packet_caps_notes_and_flags_overflow() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "cache",
            "cache reads",
            "planned",
            serde_json::json!({ "level": "feature" }),
        )
        .unwrap();
    for i in 0..7 {
        store
            .add_note(&intent.id, "context", &format!("SENTINEL-note-{i}"))
            .unwrap();
    }
    let wi = workitem::next(&store, Some(Mode::Build))
        .unwrap()
        .expect("a build item for the planned intent");
    // Tight-loop insertion means same-millisecond timestamps; id tiebreak is
    // non-deterministic, so we cannot predict which 6 of 7 are inlined.
    // Assert the count cap AND that every inlined description carries its
    // sentinel (not empty, not fabricated).
    let inlined: Vec<&str> = wi
        .context
        .linked_entities
        .iter()
        .filter(|e| e.role == "note")
        .map(|e| e.description.as_deref().unwrap_or(""))
        .collect();
    assert_eq!(inlined.len(), 6, "at most 6 notes inline in a packet");
    let sentinels: Vec<String> = (0..7).map(|i| format!("SENTINEL-note-{i}")).collect();
    for desc in &inlined {
        assert!(
            sentinels.iter().any(|s| desc.contains(s.as_str())),
            "inlined note description must carry sentinel payload, got: {desc}"
        );
    }
    assert!(
        wi.context
            .suggested_reads
            .iter()
            .any(|r| r.command.contains("note list")),
        "overflow adds a `note list` suggested read"
    );
}

#[test]
fn task_target_writes_note_and_targetless_is_diary_only() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("task-note"));
    loom_json_out(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "ranking",
            "--description",
            "rank results",
            "--level",
            "feature",
            "--lifecycle",
            "planned",
            "--json",
        ],
    );
    // Targeted experiment: close lands a note on the intent.
    loom_json_out(
        tmp.path(),
        &[
            "task",
            "add",
            "try BM25",
            "--kind",
            "experiment",
            "--target",
            "ranking",
            "--json",
        ],
    );
    loom_json_out(
        tmp.path(),
        &[
            "task",
            "close",
            "try BM25",
            "--result",
            "RESULTTOK BM25 won",
            "--json",
        ],
    );
    let notes = loom_json_out(tmp.path(), &["note", "list", "ranking", "--json"]);
    let arr = notes.as_array().expect("note list --json is an array");
    assert_eq!(arr.len(), 1, "targeted task close writes exactly one note");
    let txt = arr[0]["text"].as_str().unwrap_or("");
    assert!(txt.contains("RESULTTOK"), "result text in note: {txt}");
    assert!(txt.contains("experiment"), "task kind in note: {txt}");

    // Targetless task: no note anywhere.
    let before = loom_json_out(tmp.path(), &["note", "list", "--json"])
        .as_array()
        .unwrap()
        .len();
    loom_json_out(
        tmp.path(),
        &["task", "add", "poke index", "--kind", "spike", "--json"],
    );
    loom_json_out(
        tmp.path(),
        &[
            "task",
            "close",
            "poke index",
            "--result",
            "nothing conclusive",
            "--json",
        ],
    );
    let after = loom_json_out(tmp.path(), &["note", "list", "--json"])
        .as_array()
        .unwrap()
        .len();
    assert_eq!(
        before, after,
        "a targetless task writes no note (diary-only)"
    );
}

#[test]
fn task_abandon_writes_outcome_note() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("task-abandon"));
    loom_json_out(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "cache",
            "--description",
            "cache reads",
            "--level",
            "feature",
            "--lifecycle",
            "planned",
            "--json",
        ],
    );
    loom_json_out(
        tmp.path(),
        &[
            "task",
            "add",
            "spike LRU",
            "--kind",
            "spike",
            "--target",
            "cache",
            "--json",
        ],
    );
    loom_json_out(
        tmp.path(),
        &[
            "task",
            "abandon",
            "spike LRU",
            "--reason",
            "REASONTOK thrashes on scan",
            "--json",
        ],
    );
    let notes = loom_json_out(tmp.path(), &["note", "list", "cache", "--json"]);
    let arr = notes.as_array().expect("note list --json is an array");
    assert_eq!(arr.len(), 1, "abandon writes one note on the target intent");
    let txt = arr[0]["text"].as_str().unwrap_or("");
    assert!(txt.contains("abandoned"), "outcome marked abandoned: {txt}");
    assert!(txt.contains("REASONTOK"), "reason text in note: {txt}");
}

#[test]
fn adopt_without_proof_note_uses_unavailable_fallback() {
    // Contract 3 fallback: when a hypothesis is force-promoted to "supported"
    // without going through `hypothesis prove` (so no "supported: " decision
    // note exists), `hypothesis adopt` must write "(proof evidence unavailable)"
    // into the spawned intent's note rather than silently omitting it.
    let tmp = Tmp::new();
    // Setup: init store, create hypothesis, force to "supported" WITHOUT going
    // through `hypothesis prove` — so no "supported: " decision note is written.
    // The store is dropped at the end of this block, releasing the exclusive lock
    // before `run()` tries to acquire it.
    let h_name = {
        let store = Store::init(tmp.path(), Some("t"), false).unwrap();
        let h = store
            .add_node(
                NodeType::Hypothesis,
                "no-proof-hyp",
                "the claim",
                "proposed",
                serde_json::json!({ "proposal": "", "predicted_outcome": "" }),
            )
            .unwrap();
        store.set_node_status(&h.id, "supported").unwrap();
        h.name.clone()
    };

    // Adopt through the full in-process dispatcher; the evidence lookup finds
    // no "supported: " note and must fall back to the sentinel string.
    run(
        tmp.path(),
        Command::Hypothesis {
            cmd: HypothesisCmd::Adopt {
                key: h_name.clone(),
                spawned: Some("spawned-no-proof".into()),
            },
        },
    );

    // Reopen (lock was released above) to verify the spawned intent's note.
    let store = Store::open(tmp.path()).unwrap();
    let intents = store
        .list_nodes(Some(NodeType::Intent), usize::MAX)
        .unwrap();
    let spawned = intents
        .iter()
        .find(|n| n.name == "spawned-no-proof")
        .expect("adopt creates the spawned intent");
    let notes = store.notes_for(&spawned.id).unwrap();
    assert_eq!(
        notes.len(),
        1,
        "adopt writes exactly one note on the spawned intent"
    );
    let txt = &notes[0].description;
    assert!(
        txt.contains("(proof evidence unavailable)"),
        "fallback text in note when no 'supported:' proof note exists: {txt}"
    );
}

// ---- self-teaching: guidance strings keep the LLM on the PoC lifecycle ------
//
// These pin the wording of `loom`'s self-teaching `next_step`,
// `allowed_actions`, and `stop_condition` strings. The failure mode they
// guard: after `hypothesis prove <h> supported`, nothing tells the LLM to run
// `hypothesis adopt`, and no queue re-serves the hypothesis — the proven idea
// dies silently. Each test fails if its pinned guidance string is reverted or
// reworded away from teaching the lifecycle.

/// Contract: `hypothesis prove <h> supported --json` `next_step` CONTAINS
/// "hypothesis adopt", directing the LLM to the exact adopt command.
/// Also guards that the string does NOT embed the literal placeholder
/// `<planned intent>` — `--spawned` must be presented as optional, not a
/// required fill-in (a copy/paste of that placeholder would re-strand the LLM).
/// Fails if the supported branch is reworded to drop the adopt pointer.
#[test]
fn prove_supported_next_step_points_at_adopt() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("prove-sup"));
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "speed up checkout",
            "--description",
            "reduce latency",
            "--level",
            "feature",
            "--lifecycle",
            "planned",
        ],
    );
    loom_ok(
        tmp.path(),
        &[
            "hypothesis",
            "add",
            "--name",
            "batch-writes-poc",
            "--claim",
            "batching writes halves fsync count",
            "--proposal",
            "batch per txn",
            "--predicted-outcome",
            "50pct fewer fsyncs",
            "--target",
            "speed up checkout",
        ],
    );
    let v = loom_json_out(
        tmp.path(),
        &[
            "hypothesis",
            "prove",
            "batch-writes-poc",
            "supported",
            "--evidence",
            "bench shows writes improved from 1.9ms to 0.8ms",
            "--json",
        ],
    );
    let next_step = v["next_step"].as_str().expect("next_step is a string");
    assert!(
        next_step.contains("hypothesis adopt"),
        "supported proof must point at `loom hypothesis adopt`: {next_step}"
    );
    assert!(
        !next_step.contains("<planned intent>"),
        "--spawned must be optional; `<planned intent>` placeholder must not appear: {next_step}"
    );
}

/// Contract: `hypothesis prove <h> refuted --json` `next_step` does NOT contain
/// "hypothesis adopt" — the refuted record stands, no adoption step follows.
/// Also asserts the LLM is returned to `loom status` as the next action.
/// Uses a separate graph from the supported test so state is clean.
/// Fails if the refuted branch is accidentally reworded to recommend adopt.
#[test]
fn prove_refuted_next_step_does_not_adopt() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("prove-ref"));
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "reduce memory usage",
            "--description",
            "lower heap footprint",
            "--level",
            "feature",
            "--lifecycle",
            "planned",
        ],
    );
    loom_ok(
        tmp.path(),
        &[
            "hypothesis",
            "add",
            "--name",
            "gc-tuning-poc",
            "--claim",
            "tuning GC knobs reduces heap",
            "--proposal",
            "adjust GC params",
            "--predicted-outcome",
            "20pct heap reduction",
            "--target",
            "reduce memory usage",
        ],
    );
    let v = loom_json_out(
        tmp.path(),
        &[
            "hypothesis",
            "prove",
            "gc-tuning-poc",
            "refuted",
            "--evidence",
            "profiling shows GC tuning had no measurable heap impact",
            "--json",
        ],
    );
    let next_step = v["next_step"].as_str().expect("next_step is a string");
    assert!(
        !next_step.contains("hypothesis adopt"),
        "refuted proof must NOT recommend `hypothesis adopt`: {next_step}"
    );
    assert!(
        next_step.contains("loom status"),
        "refuted proof must return LLM to `loom status`: {next_step}"
    );
}

/// Contract: the prove-queue work item (`workitem::next(Mode::Prove)`) teaches
/// the full PoC lifecycle in three places:
///   • `next_step` names both "adopt" and the concrete hypothesis name
///   • `prompt_contract.allowed_actions` has an entry containing "adopt"
///   • `prompt_contract.stop_condition` contains "adopt"
/// Fails if any wiring point is removed or reworded to drop the adopt pointer,
/// which would leave a supported hypothesis with no queue re-serving it.
#[test]
fn prove_packet_teaches_adopt_lifecycle() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("prove-pkt"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "ship batch writes",
            "reduce fsync overhead",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    let hyp = store
        .add_node(
            NodeType::Hypothesis,
            "batch-poc",
            "batching halves fsyncs",
            "proposed",
            serde_json::json!({ "proposal": "batch per txn", "predicted_outcome": "50pct fewer" }),
        )
        .unwrap();
    store
        .ensure_edge(EdgeKind::Targets, &hyp.id, &intent.id)
        .unwrap();

    let wi = workitem::next(&store, Some(Mode::Prove))
        .unwrap()
        .expect("prove queue must serve the proposed hypothesis");

    assert!(
        wi.next_step.contains("adopt"),
        "prove packet next_step must teach adopt lifecycle: {}",
        wi.next_step
    );
    assert!(
        wi.next_step.contains("batch-poc"),
        "prove packet next_step must name the concrete hypothesis: {}",
        wi.next_step
    );
    assert!(
        wi.prompt_contract
            .allowed_actions
            .iter()
            .any(|a| a.contains("adopt")),
        "prove packet allowed_actions must include an adopt entry: {:?}",
        wi.prompt_contract.allowed_actions
    );
    assert!(
        wi.prompt_contract.stop_condition.contains("adopt"),
        "prove packet stop_condition must reference adopt: {}",
        wi.prompt_contract.stop_condition
    );
}

/// Contract: the build-queue work item (`workitem::next(Mode::Build)`)
/// `context.purpose` names `note` entities as the prior record, so an LLM
/// receiving the packet knows to read them before coding. Substrings checked:
/// "note" AND "prior". No notes need to exist — the purpose string is
/// unconditional. Fails if the build packet's purpose wording is reworded to
/// drop either marker, which would sever the LLM's awareness of prior evidence.
#[test]
fn build_packet_purpose_names_notes_as_prior_record() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("build-pkt"), false).unwrap();
    store
        .add_node(
            NodeType::Intent,
            "implement batch writes",
            "batch db writes to reduce fsyncs",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();

    let wi = workitem::next(&store, Some(Mode::Build))
        .unwrap()
        .expect("build queue must serve the planned intent");

    assert!(
        wi.context.purpose.contains("note"),
        "build packet purpose must name `note` entities as the prior record: {}",
        wi.context.purpose
    );
    assert!(
        wi.context.purpose.contains("prior"),
        "build packet purpose must say 'prior' (prior record of PoC evidence): {}",
        wi.context.purpose
    );
}
