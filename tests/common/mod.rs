use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A unique temp dir that removes itself on drop.
pub struct Tmp(PathBuf);

impl Tmp {
    pub fn new() -> Tmp {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("loom-test-{}-{nanos}-{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    #[allow(dead_code)]
    pub fn write(&self, rel: &str, content: &str) {
        let p = self.0.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Give an intent a REAL passing proof: register a trivial command and let loom
/// run it.
///
/// Fixtures used to hand-record a `passing` verdict on a `validates` edge. That
/// is precisely the move the evidence spine refuses — a proof is `verified` only
/// when loom watched it happen — so a fixture that fabricates one no longer
/// compiles a green graph. Going through the real command path means the test
/// graph is proven for the same reason a production graph would be.
#[allow(dead_code)]
pub fn prove(root: &Path, intent_name: &str, proof_name: &str) {
    use loom::cli::{Cli, Command, ValidationCmd};
    let call = |cmd: Command| {
        loom::commands::run(Cli {
            graph: Some(root.to_path_buf()),
            json: true,
            command: Some(cmd),
        })
        .unwrap_or_else(|e| panic!("fixture proof step failed: {e}"));
    };
    call(Command::Validation {
        cmd: ValidationCmd::Add {
            name: proof_name.into(),
            r#type: "test".into(),
            command: "true".into(),
            intent: intent_name.into(),
            proof_level: None,
            proof_kind: None,
            journey_id: None,
            repo_native_kind: None,
            artifact: None,
        },
    });
    call(Command::Validation {
        cmd: ValidationCmd::Run {
            key: proof_name.into(),
            all: false,
        },
    });
}

/// Put an existing validation into a genuinely-passing state: point it at a
/// trivial command and let loom run it.
///
/// For fixtures whose subject is drift detection or coverage, not proof
/// semantics. The command is irrelevant to what they assert; what matters is
/// that the passing state was EARNED by a run loom observed, because a
/// hand-written passing verdict is no longer a state the graph can hold.
#[allow(dead_code)]
pub fn observe_passing(store: &loom::store::Store, val_name: &str) {
    use loom::model::NodeType;
    let val = store
        .resolve_node(val_name, Some(NodeType::Validation))
        .unwrap_or_else(|e| panic!("resolve validation {val_name}: {e}"));
    let mut body = val.body.clone();
    body["command"] = serde_json::json!("true");
    store.set_node_body(&val.id, &body).unwrap();
    let val = store.get_node(&val.id).unwrap().unwrap();
    let outcome = loom::commands::observe_validation(store, &val)
        .unwrap_or_else(|e| panic!("observe {val_name}: {e}"));
    // A JOURNEY proof is not runnable by the validation runner — it is proven by
    // `loom journey run`. loom says so by returning `Manual` rather than
    // pretending, so the fixture attests it instead, citing the artifact the
    // proof is about. Attested is visibly weaker than observed, which is the
    // honest reading of a proof loom did not watch.
    if let loom::proof::ProofOutcome::Manual { .. } = outcome {
        let artifact = val
            .body
            .get("artifact")
            .and_then(|a| a.as_str())
            .unwrap_or_default()
            .to_string();
        for e in store
            .edges_with(Some(loom::model::EdgeKind::Validates), Some(&val.id), None)
            .unwrap()
        {
            store
                .record_verdict(
                    &e.id,
                    loom::model::InspectionStatus::Passing,
                    "attested proof",
                    &format!("attested against {artifact}:1"),
                    0.9,
                    "test",
                )
                .unwrap_or_else(|err| panic!("attest {val_name}: {err}"));
        }
        store.set_node_status(&val.id, "passed").unwrap();
    }
}
