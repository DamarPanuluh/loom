//! Ring 5 tests — quality, validation, hypothesis, saga model, vocab/layer.

use loom::cli::{Cli, CodefileCmd, Command, EdgeCmd, IntentCmd, SagaCmd, ValidationCmd};
use loom::model::{EdgeKind, InspectionStatus, NodeType, TargetKind, TruthClass};
use loom::store::Store;
use loom::workitem::{self, Mode};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Tmp(PathBuf);
impl Tmp {
    fn new() -> Tmp {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("loom-ring5-{}-{nanos}-{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        Tmp(p)
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

fn run(graph: &Path, command: Command) {
    loom::commands::run(Cli {
        graph: Some(graph.to_path_buf()),
        json: false,
        command,
    })
    .unwrap();
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
            },
        },
    );
    run(
        tmp.path(),
        Command::Validate {
            intent: "always passes".into(),
            all: false,
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
                allow_symbol_name: false,
            },
        },
    );
    run(
        tmp.path(),
        Command::Validation {
            cmd: ValidationCmd::Add {
                name: "false-proof".into(),
                r#type: "test".into(),
                command: "false".into(),
                intent: "always fails".into(),
            },
        },
    );
    run(
        tmp.path(),
        Command::Validate {
            intent: "always fails".into(),
            all: false,
        },
    );

    let store = Store::open(tmp.path()).unwrap();
    assert_eq!(
        store
            .resolve_node("false-proof", Some(NodeType::Validation))
            .unwrap()
            .status,
        "failed"
    );
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

// ---- saga model ------------------------------------------------------------

#[test]
fn saga_add_creates_validation_and_step_edges() {
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

    let spec = tmp.path().join("checkout.saga.json");
    std::fs::write(
        &spec,
        r#"{"saga":"checkout-flow","steps":[{"intent":"create cart"},{"intent":"capture payment"}]}"#,
    )
    .unwrap();
    run(
        tmp.path(),
        Command::Saga {
            cmd: SagaCmd::Add { spec },
        },
    );

    let store = Store::open(tmp.path()).unwrap();
    let saga = store
        .resolve_node("checkout-flow", Some(NodeType::Validation))
        .unwrap();
    assert_eq!(saga.body.get("type").and_then(|t| t.as_str()), Some("saga"));
    let validates = store
        .edges_with(Some(EdgeKind::Validates), Some(&saga.id), None)
        .unwrap();
    assert_eq!(validates.len(), 2, "saga validates each step intent");
    // a sequence edge links the two steps
    let cart = store
        .resolve_node("create cart", Some(NodeType::Intent))
        .unwrap();
    let seq = store
        .edges_with(Some(EdgeKind::Sequence), Some(&cart.id), None)
        .unwrap();
    assert_eq!(seq.len(), 1, "consecutive steps are sequence-linked");
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
    run(
        tmp.path(),
        Command::Guide {
            role: Some("monitor".into()),
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
            cmd: ValidationCmd::Mark {
                key: "probe".into(),
                result: "passed".into(),
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
            cmd: IntentCmd::Set {
                key: "f".into(),
                level: Some("system".into()),
                visibility: Some("internal".into()),
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

    let gaps = loom_json_out(tmp.path(), &["interface", "gaps", "--json"]);
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
        &["interface", "gaps", "--json"],
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
        &["saga", "list", "--json"],
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
        &["validate", "--all", "--json"],
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
