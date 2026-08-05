use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use loom::journey::{Expect, JourneySpec, Step};
use loom::model::{EdgeKind, InspectionStatus, NodeType, TargetKind, TruthClass};
use loom::store::Store;

static NEXT_TMP: AtomicU64 = AtomicU64::new(0);

struct Tmp {
    root: PathBuf,
}

impl Tmp {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "loom-post-update-{label}-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn path(&self) -> &Path {
        &self.root
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn loom_output(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_loom"))
        .arg("--graph")
        .arg(root)
        .args(args)
        .output()
        .expect("spawn loom")
}

fn add_intent(store: &Store, name: &str, description: &str) -> String {
    store
        .add_node(
            NodeType::Intent,
            name,
            description,
            "implemented",
            serde_json::json!({}),
        )
        .unwrap()
        .id
}

fn add_codefile(store: &Store, root: &Path, path: &str, source: &str) -> String {
    let absolute = root.join(path);
    std::fs::create_dir_all(absolute.parent().unwrap()).unwrap();
    std::fs::write(&absolute, source).unwrap();
    store
        .add_node(
            NodeType::CodeFile,
            path,
            "",
            "present",
            serde_json::json!({}),
        )
        .unwrap()
        .id
}

#[test]
fn export_const_locator_stays_resolved_across_syncs() {
    let tmp = Tmp::new("export-const");
    let store = Store::init(tmp.path(), Some("export const"), false).unwrap();
    let intent = add_intent(&store, "page data loads", "returns the page data");
    let file = add_codefile(
        &store,
        tmp.path(),
        "src/routes/+page.ts",
        "export const load = async () => ({ ok: true });\n",
    );
    loom::sync::run(&store, tmp.path()).unwrap();

    let edge = store
        .add_edge(EdgeKind::Implements, &intent, &file, TruthClass::Asserted)
        .unwrap();
    store
        .set_facet(
            &edge.id,
            TargetKind::Edge,
            "locator",
            "export const load",
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &edge.id,
            InspectionStatus::Passing,
            "load returns page data",
            "src/routes/+page.ts:1",
            0.95,
            "llm",
        )
        .unwrap();

    for _ in 0..2 {
        loom::sync::run(&store, tmp.path()).unwrap();
        assert_eq!(
            store.get_edge(&edge.id).unwrap().unwrap().status,
            InspectionStatus::Passing,
            "sync must not stale a live `export const` locator"
        );
    }
}

#[test]
fn edge_implement_refuses_role_or_locator_collision_without_mutation() {
    let tmp = Tmp::new("edge-collision");
    let store = Store::init(tmp.path(), Some("edge collision"), false).unwrap();
    let intent = add_intent(&store, "request is handled", "handles one request");
    let file = add_codefile(
        &store,
        tmp.path(),
        "src/handler.ts",
        "export const live = () => true;\nexport const witness = () => true;\n",
    );
    loom::sync::run(&store, tmp.path()).unwrap();
    let edge = store
        .add_edge(EdgeKind::Implements, &intent, &file, TruthClass::Asserted)
        .unwrap();
    store
        .set_facet(
            &edge.id,
            TargetKind::Edge,
            "locator",
            "live",
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &edge.id,
            InspectionStatus::Passing,
            "live handles the request",
            "src/handler.ts:1",
            0.95,
            "llm",
        )
        .unwrap();
    drop(store);

    let output = loom_output(
        tmp.path(),
        &[
            "edge",
            "implement",
            &intent,
            "src/handler.ts",
            "--role",
            "verifies",
            "--locator",
            "witness",
        ],
    );
    assert!(!output.status.success(), "collision must be refused");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("edge exists"),
        "actionable refusal: {stderr}"
    );
    assert!(stderr.contains("set-role") && stderr.contains("set-locator"));

    let store = Store::open(tmp.path()).unwrap();
    let unchanged = store.get_edge(&edge.id).unwrap().unwrap();
    assert_eq!(unchanged.status, InspectionStatus::Passing);
    assert_eq!(
        store.grounding_role(&edge.id).unwrap(),
        loom::model::GroundingRole::Realizes
    );
    assert_eq!(
        store
            .get_facet(&edge.id, TargetKind::Edge, "locator")
            .unwrap()
            .as_deref(),
        Some("live")
    );
}

#[test]
fn duplicate_clear_survives_unrelated_writes_and_expires_on_reword() {
    let tmp = Tmp::new("duplicate-clear");
    let store = Store::init(tmp.path(), Some("duplicate clear"), false).unwrap();
    let a = add_intent(
        &store,
        "checkout may be retried",
        "retries only transport failures",
    );
    let b = add_intent(
        &store,
        "checkout may be resumed",
        "resumes only user-interrupted checkout",
    );
    let file = add_codefile(
        &store,
        tmp.path(),
        "src/checkout.rs",
        "pub fn checkout() {}\n",
    );
    for intent in [&a, &b] {
        let edge = store
            .add_edge(EdgeKind::Implements, intent, &file, TruthClass::Asserted)
            .unwrap();
        store
            .set_facet(
                &edge.id,
                TargetKind::Edge,
                "locator",
                "checkout",
                TruthClass::Asserted,
            )
            .unwrap();
    }
    assert_eq!(loom::divergence::rectifiable_count(&store).unwrap(), 1);
    drop(store);

    let output = loom_output(
        tmp.path(),
        &[
            "intent",
            "update",
            &b,
            "--rectify",
            "clear",
            "--reason",
            "retry is automatic after transport failure; resume is user-directed",
        ],
    );
    assert!(
        output.status.success(),
        "clear failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let store = Store::open(tmp.path()).unwrap();
    assert_eq!(loom::divergence::rectifiable_count(&store).unwrap(), 0);
    let unrelated = add_intent(&store, "receipt is emailed", "sends one receipt");
    let other_file = add_codefile(
        &store,
        tmp.path(),
        "src/receipt.rs",
        "pub fn email_receipt() {}\n",
    );
    store
        .add_edge(
            EdgeKind::Implements,
            &unrelated,
            &other_file,
            TruthClass::Asserted,
        )
        .unwrap();
    assert_eq!(
        loom::divergence::rectifiable_count(&store).unwrap(),
        0,
        "unrelated writes must not resurrect the decided pair"
    );

    store
        .update_node(&a, None, Some("retries all checkout failures"), None)
        .unwrap();
    assert_eq!(
        loom::divergence::rectifiable_count(&store).unwrap(),
        1,
        "changing either description invalidates the pair decision"
    );
}

#[test]
fn failing_cli_steps_include_command_stream_tails_and_exit_classification() {
    let tmp = Tmp::new("journey-output");
    let exited = JourneySpec {
        journey: "step exit evidence".into(),
        base: String::new(),
        steps: vec![Step {
            name: "broken command".into(),
            intent: "observable behavior".into(),
            run: "printf 'stdout-evidence\\n'; printf 'stderr-evidence\\n' >&2; exit 7".into(),
            expect: Expect::default(),
            ..Step::default()
        }],
    };
    let outcomes = loom::journey::execute_steps(&exited, Some(tmp.path()), false).unwrap();
    let detail = &outcomes[0].detail;
    assert!(detail.contains("classification: step_exit"), "{detail}");
    assert!(detail.contains("command: `printf"), "{detail}");
    assert!(detail.contains("stdout-evidence"), "{detail}");
    assert!(detail.contains("stderr-evidence"), "{detail}");

    let killed = JourneySpec {
        journey: "runner kill evidence".into(),
        base: String::new(),
        steps: vec![Step {
            name: "timed out command".into(),
            intent: "observable behavior".into(),
            run: "printf 'before-timeout\\n'; printf 'timeout-stderr\\n' >&2; sleep 2".into(),
            timeout_secs: Some(1),
            ..Step::default()
        }],
    };
    let outcomes = loom::journey::execute_steps(&killed, Some(tmp.path()), false).unwrap();
    let detail = &outcomes[0].detail;
    assert!(detail.contains("classification: runner_kill"), "{detail}");
    assert!(detail.contains("before-timeout"), "{detail}");
    assert!(detail.contains("timeout-stderr"), "{detail}");
}
