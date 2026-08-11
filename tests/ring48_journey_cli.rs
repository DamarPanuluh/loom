//! Ring 48 — the public v12 CLI teaches and enforces Journey-root intake.

use clap::{CommandFactory, Parser};
use loom::cli::{Cli, Command, JourneyCmd};
use loom::model::{EdgeKind, NodeType, TargetKind};
use loom::store::Store;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "loom-ring48-{}-{nanos}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn invoke(root: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_loom"))
        .env("LOOM_AGENT", "llm:builder")
        .env("LOOM_NON_INTERACTIVE", "1")
        .arg("--graph")
        .arg(root)
        .args(args)
        .output()
        .expect("spawn loom")
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "command failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn journey_parser_exposes_only_the_v12_contract() {
    for argv in [
        vec!["loom", "journey", "add", "journeys/checkout.yaml"],
        vec!["loom", "journey", "show", "checkout"],
        vec!["loom", "journey", "list"],
        vec!["loom", "journey", "map"],
        vec!["loom", "journey", "remove", "checkout"],
        vec!["loom", "journey", "derive", "checkout"],
        vec![
            "loom",
            "journey",
            "derive-accept",
            "checkout",
            "--manifest",
            "derive.json",
            "--human-decision",
            "Accept this exact mapping",
        ],
        vec!["loom", "journey", "surface", "checkout"],
        vec![
            "loom",
            "journey",
            "surface-accept",
            "checkout",
            "--manifest",
            "surface.json",
        ],
        vec!["loom", "journey", "compile", "checkout"],
        vec!["loom", "journey", "run", "checkout", "--profile", "smoke"],
        vec![
            "loom",
            "journey",
            "diagnose",
            "checkout",
            "--input",
            "sku=\"sku-1\"",
            "--input",
            "quantity=2",
        ],
        vec!["loom", "journey", "freeze", "checkout"],
        vec!["loom", "journey", "drift"],
        vec!["loom", "journey", "drift", "checkout"],
    ] {
        assert!(Cli::try_parse_from(&argv).is_ok(), "must parse: {argv:?}");
    }

    for argv in [
        vec!["loom", "journey", "coverage", "discover"],
        vec!["loom", "journey", "invariant", "list"],
        vec!["loom", "journey", "prompt", "checkout"],
        vec![
            "loom",
            "journey",
            "run",
            "checkout",
            "--base-url",
            "http://x",
        ],
        vec!["loom", "journey", "compile", "checkout", "--input", "sku=1"],
        vec![
            "loom",
            "journey",
            "derive-accept",
            "checkout",
            "--manifest",
            "derive.json",
            "--evidence",
            "legacy",
            "--human-decision",
            "accept",
        ],
    ] {
        assert!(Cli::try_parse_from(&argv).is_err(), "must reject: {argv:?}");
    }
}

#[test]
fn execution_profiles_default_to_proof_and_only_diagnose_accepts_inputs() {
    let cli = Cli::try_parse_from(["loom", "journey", "compile", "checkout"]).unwrap();
    match cli.command.unwrap() {
        Command::Journey {
            cmd: JourneyCmd::Compile { journey, profile },
        } => {
            assert_eq!(journey, "checkout");
            assert_eq!(profile, "proof");
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let cli = Cli::try_parse_from([
        "loom",
        "journey",
        "diagnose",
        "checkout",
        "--input",
        "sku=\"sku-1\"",
    ])
    .unwrap();
    match cli.command.unwrap() {
        Command::Journey {
            cmd:
                JourneyCmd::Diagnose {
                    journey,
                    profile,
                    input,
                },
        } => {
            assert_eq!(journey, "checkout");
            assert_eq!(profile, "proof");
            assert_eq!(input, ["sku=\"sku-1\""]);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn validation_add_has_no_legacy_journey_metadata_door() {
    let base = [
        "loom",
        "validation",
        "add",
        "--name",
        "proof",
        "--intent",
        "behavior",
    ];
    assert!(Cli::try_parse_from(base).is_ok());
    for retired in [
        "--proof-kind",
        "--journey-id",
        "--repo-native-kind",
        "--artifact",
    ] {
        let mut argv = base.to_vec();
        argv.extend([retired, "journey"]);
        assert!(Cli::try_parse_from(argv).is_err(), "accepted {retired}");
    }
}

#[test]
fn question_add_requires_exactly_one_intent_or_journey_target() {
    assert!(Cli::try_parse_from([
        "loom",
        "question",
        "add",
        "Which behavior is wanted?",
        "--intent",
        "checkout",
    ])
    .is_ok());
    assert!(Cli::try_parse_from([
        "loom",
        "question",
        "add",
        "Which flow is wanted?",
        "--journey",
        "checkout",
    ])
    .is_ok());
    assert!(Cli::try_parse_from(["loom", "question", "add", "Where does this belong?",]).is_err());
    assert!(Cli::try_parse_from([
        "loom",
        "question",
        "add",
        "Where does this belong?",
        "--intent",
        "intent",
        "--journey",
        "journey",
    ])
    .is_err());
}

#[test]
fn questions_can_target_intents_or_journeys_and_report_the_target_kind() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("ring48-questions"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "checkout calculates totals",
            "the technical total is deterministic",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    let journey = store
        .add_node(
            NodeType::Journey,
            "checkout.happy",
            "a shopper completes checkout",
            "authored",
            serde_json::json!({}),
        )
        .unwrap();
    drop(store);

    assert_success(&invoke(
        tmp.path(),
        &[
            "question",
            "add",
            "Which rounding rule applies?",
            "--intent",
            &intent.id,
        ],
    ));
    assert_success(&invoke(
        tmp.path(),
        &[
            "question",
            "add",
            "May a guest complete this Journey?",
            "--journey",
            &journey.id,
        ],
    ));

    let store = Store::open_read(tmp.path()).unwrap();
    let questions = store
        .list_nodes(Some(NodeType::Question), usize::MAX)
        .unwrap();
    assert_eq!(questions.len(), 2);
    let targets: std::collections::BTreeSet<_> = questions
        .iter()
        .map(|question| {
            store
                .edges_with(Some(EdgeKind::Questions), Some(&question.id), None)
                .unwrap()
                .into_iter()
                .next()
                .unwrap()
                .to_id
        })
        .collect();
    assert_eq!(targets, [intent.id.clone(), journey.id.clone()].into());
    drop(store);

    let output = invoke(tmp.path(), &["--json", "question", "list"]);
    assert_success(&output);
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let rows = value["items"].as_array().unwrap();
    let kinds: std::collections::BTreeSet<_> = rows
        .iter()
        .filter_map(|row| row["target_kind"].as_str())
        .collect();
    assert_eq!(kinds, ["intent", "journey"].into());
    assert!(rows.iter().any(|row| row["intent"].is_object()));
    assert!(rows.iter().any(|row| row["journey"].is_object()));

    let context = invoke(tmp.path(), &["--json", "context", &journey.id]);
    assert_success(&context);
    let context: Value = serde_json::from_slice(&context.stdout).unwrap();
    assert_eq!(context["target"]["kind"], "journey");
    assert!(context["context"]["linked_entities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entity| entity["kind"] == "question"));
}

#[test]
fn journey_exemption_is_canonical_human_gated_and_semantically_invalidated() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("ring48"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "repository cache stays coherent",
            "cache metadata remains internally coherent",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    drop(store);

    let denied = invoke(
        tmp.path(),
        &[
            "intent",
            "journey-exempt",
            &intent.id,
            "--kind",
            "infrastructure",
            "--reason",
            "not user-reachable",
        ],
    );
    assert!(!denied.status.success(), "non-human exemption was accepted");

    assert_success(&invoke(
        tmp.path(),
        &[
            "intent",
            "journey-exempt",
            &intent.id,
            "--kind",
            "infrastructure",
            "--reason",
            "not user-reachable",
            "--human-decision",
            "Keep this repository-only behavior outside user Journeys",
        ],
    ));
    let store = Store::open_read(tmp.path()).unwrap();
    let raw = store
        .get_facet(&intent.id, TargetKind::Node, "journey_exemption")
        .unwrap()
        .expect("canonical exemption facet");
    let parsed: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(raw, serde_json::to_string(&parsed).unwrap());
    assert_eq!(parsed.as_object().unwrap().len(), 3);
    assert_eq!(parsed["kind"], "infrastructure");
    assert_eq!(parsed["reason"], "not user-reachable");
    assert!(parsed["human_decision_digest"]
        .as_str()
        .is_some_and(|digest| !digest.is_empty()));
    assert!(!raw.contains("Keep this repository-only behavior"));
    assert!(loom::completeness::intent_journey_exempt(&store, &intent.id).unwrap());
    drop(store);

    assert_success(&invoke(
        tmp.path(),
        &[
            "intent",
            "update",
            &intent.id,
            "--description",
            "cache metadata remains coherent across repository operations",
            "--reason",
            "clearer wording",
            "--reword",
        ],
    ));
    let store = Store::open_read(tmp.path()).unwrap();
    assert!(store
        .get_facet(&intent.id, TargetKind::Node, "journey_exemption")
        .unwrap()
        .is_some());
    drop(store);

    assert_success(&invoke(
        tmp.path(),
        &[
            "intent",
            "update",
            &intent.id,
            "--description",
            "cache entries are rebuilt from authoritative repository state",
            "--reason",
            "the semantic criterion changed",
        ],
    ));
    let store = Store::open_read(tmp.path()).unwrap();
    assert!(store
        .get_facet(&intent.id, TargetKind::Node, "journey_exemption")
        .unwrap()
        .is_none());
}

#[test]
fn journey_require_is_human_gated_and_withdraws_the_facet() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("ring48-require"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "operator can inspect cache state",
            "cache state is available to operators",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    drop(store);
    assert_success(&invoke(
        tmp.path(),
        &[
            "intent",
            "journey-exempt",
            &intent.id,
            "--kind",
            "infrastructure",
            "--reason",
            "initially operator-only",
            "--human-decision",
            "Approve the repository-only exemption",
        ],
    ));
    assert_success(&invoke(
        tmp.path(),
        &[
            "intent",
            "journey-require",
            &intent.id,
            "--reason",
            "operators now reach it through an authored flow",
            "--human-decision",
            "Require Journey ancestry again",
        ],
    ));
    let store = Store::open_read(tmp.path()).unwrap();
    assert!(store
        .get_facet(&intent.id, TargetKind::Node, "journey_exemption")
        .unwrap()
        .is_none());
}

#[test]
fn door_and_mcp_teach_journey_root_derive_and_surface() {
    let tmp = Tmp::new();
    Store::init(tmp.path(), Some("ring48-door"), false).unwrap();
    let output = invoke(
        tmp.path(),
        &["--json", "door", "a shopper completes checkout"],
    );
    assert_success(&output);
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let menu = value["landing_menu"].as_array().unwrap();
    let new_journey = menu
        .iter()
        .find(|entry| entry["landing"] == "new_journey")
        .expect("new Journey landing");
    assert!(new_journey["command"]
        .as_str()
        .unwrap()
        .contains("loom journey add"));
    assert!(new_journey["after"]
        .as_str()
        .unwrap()
        .contains("--mode derive"));
    assert!(!menu.iter().any(|entry| entry["landing"] == "new_intent"));

    let initialized = loom::mcp::handle(
        None,
        &serde_json::json!({"jsonrpc":"2.0", "id":1, "method":"initialize"}),
    )
    .unwrap();
    let instructions = initialized["result"]["instructions"].as_str().unwrap();
    for term in [
        "authored Journeys",
        "technical Intents",
        "surfaces",
        "derive/surface",
    ] {
        assert!(
            instructions.contains(term),
            "missing {term}: {instructions}"
        );
    }
}

#[test]
fn public_help_contains_new_modes_and_no_retired_journey_families() {
    let mut command = Cli::command();
    let journey = command.find_subcommand_mut("journey").unwrap();
    let names: std::collections::BTreeSet<_> = journey
        .get_subcommands()
        .map(|command| command.get_name().to_string())
        .collect();
    for required in [
        "add",
        "show",
        "list",
        "map",
        "remove",
        "derive",
        "derive-accept",
        "surface",
        "surface-accept",
        "compile",
        "run",
        "diagnose",
        "freeze",
        "drift",
    ] {
        assert!(names.contains(required), "missing journey {required}");
    }
    for retired in ["coverage", "invariant", "prompt"] {
        assert!(
            !names.contains(retired),
            "retired journey {retired} remains"
        );
    }
}
