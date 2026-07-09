//! Ring 8 tests — durable finding triage.

use loom::maturity::{ladder, RungState};
use loom::model::{EdgeKind, InspectionStatus, NodeType, TargetKind, TruthClass};
use loom::store::Store;
use loom::workitem::{self, Mode};
mod common;
use common::*;

fn derived_finding(store: &Store) -> loom::model::Node {
    store
        .add_derived_node(
            NodeType::Finding,
            "oversized_file:src/x.rs:",
            "src/x.rs is oversized",
            "1200 lines",
            "oversized_file",
            serde_json::json!({ "kind": "oversized_file", "symbol": "" }),
        )
        .unwrap()
}

fn mature_graph_with_codefile(store: &Store) -> loom::model::Node {
    let intent = store
        .add_node(
            NodeType::Intent,
            "behavior holds",
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let codefile = store
        .add_node(
            NodeType::CodeFile,
            "src/x.rs",
            "",
            "active",
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
    store
        .record_verdict(
            &edge.id,
            InspectionStatus::Passing,
            "grounded",
            "src/x.rs",
            0.9,
            "llm",
        )
        .unwrap();
    let validation = store
        .add_node(
            NodeType::Validation,
            "proof",
            "",
            "passed",
            serde_json::json!({}),
        )
        .unwrap();
    let ve = store
        .add_edge(
            EdgeKind::Validates,
            &validation.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &ve.id,
            InspectionStatus::Passing,
            "proof",
            "cargo test proof",
            1.0,
            "llm",
        )
        .unwrap();
    codefile
}

#[test]
fn finding_adjudication_survives_derived_graph_wipe() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let finding = derived_finding(&store);
    store
        .record_finding_verdict(&finding.id, "justified", "cohesive")
        .unwrap();

    store.wipe_derived_graph().unwrap();
    let rebuilt = derived_finding(&store);
    assert_eq!(rebuilt.id, finding.id);

    let raw = store
        .get_facet(&finding.id, TargetKind::Node, "adjudication")
        .unwrap()
        .expect("asserted adjudication facet survives derived wipe");
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(value["verdict"], "justified");
    assert_eq!(value["reason"], "cohesive");
}

#[test]
fn finding_adjudication_goes_stale_when_codefile_hash_changes() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let codefile = mature_graph_with_codefile(&store);
    store
        .set_facet(
            &codefile.id,
            TargetKind::Node,
            "content_hash",
            "h1",
            TruthClass::Derived,
        )
        .unwrap();
    let finding = derived_finding(&store);
    store
        .add_derived_edge(EdgeKind::Flags, &finding.id, &codefile.id)
        .unwrap();
    store
        .record_finding_verdict(&finding.id, "justified", "cohesive")
        .unwrap();

    let fresh = loom::signal::findings_view(&store).unwrap();
    assert_eq!(fresh.len(), 1);
    assert!(!fresh[0].stale);
    assert!(loom::signal::untriaged_findings(&store).unwrap().is_empty());

    store
        .set_facet(
            &codefile.id,
            TargetKind::Node,
            "content_hash",
            "h2",
            TruthClass::Derived,
        )
        .unwrap();
    let stale = loom::signal::findings_view(&store).unwrap();
    assert!(stale[0].stale);
    assert_eq!(loom::signal::untriaged_findings(&store).unwrap().len(), 0);
    assert_eq!(loom::signal::stale_findings(&store).unwrap().len(), 1);
    assert_eq!(loom::signal::triage_findings(&store).unwrap().len(), 1);
}

#[test]
fn triage_mode_serves_findings_until_verdict_is_recorded() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    mature_graph_with_codefile(&store);
    let finding = derived_finding(&store);

    let item = workitem::next(&store, Some(Mode::Triage))
        .unwrap()
        .expect("untriaged finding is served");
    assert_eq!(item.mode, "triage");
    assert_eq!(item.target.id, finding.id);
    // The contract is copy-paste runnable: the concrete short id, not a `<id>`
    // placeholder, so a text-mode agent never needs a separate `finding list`.
    let short = &finding.id[..8];
    assert!(item.prompt_contract.write_back.contains(short));
    assert!(!item.prompt_contract.write_back.contains("<id>"));
    assert!(item
        .prompt_contract
        .allowed_actions
        .iter()
        .all(|a| a.contains(short)));
    assert_eq!(ladder(&store).unwrap().phase, "triage");

    store
        .record_finding_verdict(&finding.id, "justified", "cohesive")
        .unwrap();
    assert!(workitem::next(&store, Some(Mode::Triage))
        .unwrap()
        .is_none());
    let judged = ladder(&store).unwrap();
    assert_ne!(judged.phase, "triage");
}

#[test]
fn triage_item_surfaces_owning_intents_as_cohesion_evidence() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let codefile = mature_graph_with_codefile(&store);
    let finding = derived_finding(&store);
    store
        .add_derived_edge(EdgeKind::Flags, &finding.id, &codefile.id)
        .unwrap();

    let item = workitem::next(&store, Some(Mode::Triage))
        .unwrap()
        .expect("untriaged finding is served");
    // The judgment input comes from the graph, not grep: the flagged file's
    // owning intent is named in the work item so cohesion is judged at a glance.
    assert!(item.reason.contains("owns 1 intent(s)"));
    assert!(item.reason.contains("behavior holds"));
}

#[test]
fn graph_state_counts_needed_findings() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let finding = derived_finding(&store);
    assert_eq!(workitem::graph_state(&store).unwrap().needed, 0);
    store
        .record_finding_verdict(&finding.id, "needed", "split it")
        .unwrap();
    let pulse = workitem::graph_state(&store).unwrap();
    assert_eq!(pulse.needed, 1);
    // A `needed` verdict is a judgment, so it leaves raw untriaged.
    assert_eq!(pulse.untriaged, 0);
    assert_eq!(pulse.stale_findings, 0);
}
#[test]
fn excellent_rung_counts_needed_blocked_but_not_justified() {
    fn excellent_state(store: &Store) -> RungState {
        ladder(store)
            .unwrap()
            .rungs
            .into_iter()
            .find(|r| r.name == "excellent")
            .unwrap()
            .state
    }

    let tmp = Tmp::new();
    tmp.write("src/x.rs", "pub fn x() {}\n");
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let codefile = mature_graph_with_codefile(&store);

    let existing_owner = store
        .realizing_implementers(&codefile.id)
        .unwrap()
        .pop()
        .unwrap()
        .from_id;
    store
        .add_vocab_term("cross_cut", "cross-cut concern")
        .unwrap();
    store
        .set_tag(&existing_owner, TargetKind::Node, "cross_cut")
        .unwrap();

    let second_owner = store
        .add_node(
            NodeType::Intent,
            "cross-cut behavior",
            "",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    store
        .set_tag(&second_owner.id, TargetKind::Node, "cross_cut")
        .unwrap();
    store
        .add_edge(
            EdgeKind::Implements,
            &second_owner.id,
            &codefile.id,
            TruthClass::Asserted,
        )
        .unwrap();

    let smells = loom::signal::smells(&store).unwrap();
    assert_eq!(smells.len(), 1);
    let smell = smells
        .into_iter()
        .find(|s| s.kind == "overlapping_ownership")
        .unwrap();
    let identity = smell.identity;
    let finding_id =
        Store::derived_node_id(NodeType::Finding, &loom::signal::smell_det_key(&identity));

    loom::sync::run(&store, tmp.path()).unwrap();

    assert!(!loom::signal::smell_has_resolving_adjudication(&store, &identity).unwrap());
    assert_eq!(excellent_state(&store), RungState::Unmet);

    // `justified` is a resolving adjudication — excellent rung becomes Met.
    store
        .record_finding_verdict(&finding_id, "justified", "accepted cross-cut")
        .unwrap();
    assert!(loom::signal::smell_has_resolving_adjudication(&store, &identity).unwrap());
    assert_eq!(excellent_state(&store), RungState::Met);

    // `needed` is NOT resolving — excellent rung stays Unmet.
    store
        .record_finding_verdict(&finding_id, "needed", "must split")
        .unwrap();
    assert!(!loom::signal::smell_has_resolving_adjudication(&store, &identity).unwrap());
    assert_eq!(workitem::graph_state(&store).unwrap().untriaged, 0);
    assert_eq!(excellent_state(&store), RungState::Unmet);

    // `blocked` is NOT resolving — excellent rung stays Unmet.
    store
        .record_finding_verdict(&finding_id, "blocked", "upstream")
        .unwrap();
    assert!(!loom::signal::smell_has_resolving_adjudication(&store, &identity).unwrap());
    assert_eq!(workitem::graph_state(&store).unwrap().untriaged, 0);
    assert_eq!(excellent_state(&store), RungState::Unmet);

    // `rejected` is a resolving adjudication — excellent rung becomes Met.
    store
        .record_finding_verdict(&finding_id, "rejected", "false positive after inspection")
        .unwrap();
    assert!(loom::signal::smell_has_resolving_adjudication(&store, &identity).unwrap());
    assert_eq!(excellent_state(&store), RungState::Met);

    // `deferred` is a resolving adjudication — excellent rung becomes Met.
    store
        .record_finding_verdict(&finding_id, "deferred", "revisit after v2 ships")
        .unwrap();
    assert!(loom::signal::smell_has_resolving_adjudication(&store, &identity).unwrap());
    assert_eq!(excellent_state(&store), RungState::Met);

    // `duplicate` is a resolving adjudication — excellent rung becomes Met.
    store
        .record_finding_verdict(
            &finding_id,
            "duplicate",
            "same as overlapping_ownership:other",
        )
        .unwrap();
    assert!(loom::signal::smell_has_resolving_adjudication(&store, &identity).unwrap());
    assert_eq!(excellent_state(&store), RungState::Met);
}

#[test]
fn graph_state_splits_findings_into_open_and_resolved() {
    // Fresh untriaged finding is open; adjudicating it as justified flips it
    // to resolved. The invariant open + resolved == total holds throughout,
    // and a finding that is both `needed` and stale counts once in open.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let finding = derived_finding(&store);

    // (1) Untriaged: open, nothing resolved.
    let pulse = workitem::graph_state(&store).unwrap();
    assert_eq!(pulse.findings, 1);
    assert_eq!(pulse.open_findings, 1);
    assert_eq!(pulse.resolved_findings, 0);
    assert_eq!(
        pulse.open_findings + pulse.resolved_findings,
        pulse.findings
    );

    // (2) Each of the four non-open verdicts resolves the finding.
    // `justified` resolves.
    store
        .record_finding_verdict(&finding.id, "justified", "cohesive")
        .unwrap();
    let pulse = workitem::graph_state(&store).unwrap();
    assert_eq!(pulse.open_findings, 0);
    assert_eq!(pulse.resolved_findings, 1);
    assert_eq!(
        pulse.open_findings + pulse.resolved_findings,
        pulse.findings
    );

    // `rejected` resolves.
    store
        .record_finding_verdict(&finding.id, "rejected", "false positive")
        .unwrap();
    let pulse = workitem::graph_state(&store).unwrap();
    assert_eq!(pulse.open_findings, 0, "rejected is resolved");
    assert_eq!(pulse.resolved_findings, 1);
    assert_eq!(
        pulse.open_findings + pulse.resolved_findings,
        pulse.findings
    );

    // `deferred` resolves.
    store
        .record_finding_verdict(&finding.id, "deferred", "after v2")
        .unwrap();
    let pulse = workitem::graph_state(&store).unwrap();
    assert_eq!(pulse.open_findings, 0, "deferred is resolved");
    assert_eq!(pulse.resolved_findings, 1);
    assert_eq!(
        pulse.open_findings + pulse.resolved_findings,
        pulse.findings
    );

    // `duplicate` resolves.
    store
        .record_finding_verdict(&finding.id, "duplicate", "same as other:finding")
        .unwrap();
    let pulse = workitem::graph_state(&store).unwrap();
    assert_eq!(pulse.open_findings, 0, "duplicate is resolved");
    assert_eq!(pulse.resolved_findings, 1);
    assert_eq!(
        pulse.open_findings + pulse.resolved_findings,
        pulse.findings
    );

    // `needed` remains open.
    store
        .record_finding_verdict(&finding.id, "needed", "schedule it")
        .unwrap();
    let pulse = workitem::graph_state(&store).unwrap();
    assert_eq!(pulse.open_findings, 1, "needed stays open");
    assert_eq!(pulse.resolved_findings, 0);
    assert_eq!(
        pulse.open_findings + pulse.resolved_findings,
        pulse.findings
    );

    // `blocked` remains open.
    store
        .record_finding_verdict(&finding.id, "blocked", "upstream dep")
        .unwrap();
    let pulse = workitem::graph_state(&store).unwrap();
    assert_eq!(pulse.open_findings, 1, "blocked stays open");
    assert_eq!(pulse.resolved_findings, 0);
    assert_eq!(
        pulse.open_findings + pulse.resolved_findings,
        pulse.findings
    );

    // (3) A `needed` finding whose codefile hash later diverges is both needed
    // and stale. Naive untriaged+stale+needed addition would count it twice
    // (as 2), but the contract counts the finding once in open and zero in
    // resolved, preserving open + resolved == total.
    let tmp2 = Tmp::new();
    let store = Store::init(tmp2.path(), Some("t"), false).unwrap();
    let codefile = mature_graph_with_codefile(&store);
    store
        .set_facet(
            &codefile.id,
            TargetKind::Node,
            "content_hash",
            "h1",
            TruthClass::Derived,
        )
        .unwrap();
    let finding = derived_finding(&store);
    store
        .add_derived_edge(EdgeKind::Flags, &finding.id, &codefile.id)
        .unwrap();
    // Stamp `needed` while the hash is h1, so the verdict records hash=h1.
    store
        .record_finding_verdict(&finding.id, "needed", "split it")
        .unwrap();
    let current = workitem::graph_state(&store).unwrap();
    assert_eq!(current.findings, 1);
    assert_eq!(current.open_findings, 1);
    assert_eq!(current.resolved_findings, 0);
    assert_eq!(
        current.open_findings + current.resolved_findings,
        current.findings
    );

    // Diverge the codefile hash: the finding is now needed AND stale.
    store
        .set_facet(
            &codefile.id,
            TargetKind::Node,
            "content_hash",
            "h2",
            TruthClass::Derived,
        )
        .unwrap();
    let stale = workitem::graph_state(&store).unwrap();
    assert_eq!(stale.findings, 1);
    assert_eq!(stale.needed, 1);
    assert_eq!(stale.stale_findings, 1);
    // Counted once, not twice — the regression this defends against.
    assert_eq!(stale.open_findings, 1);
    assert_eq!(stale.resolved_findings, 0);
    assert_eq!(
        stale.open_findings + stale.resolved_findings,
        stale.findings
    );
}

// ---- CLI helpers (compiled binary) -----------------------------------------

fn loom_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_loom"))
}

fn loom_init(tmp: &std::path::Path, name: Option<&str>) {
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
        "loom init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn loom_json(tmp: &std::path::Path, args: &[&str]) -> serde_json::Value {
    let mut cmd = std::process::Command::new(loom_bin());
    cmd.arg("--graph").arg(tmp).args(args).arg("--json");
    let out = cmd.output().unwrap_or_else(|e| panic!("spawn loom: {e}"));
    assert!(
        out.status.success(),
        "loom {:?} --json failed: {}\n{}",
        args,
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "loom {:?} --json did not emit JSON:\n{}\nparse error: {e}",
            args, stdout
        )
    })
}

fn loom_run_ok(tmp: &std::path::Path, args: &[&str]) -> String {
    let mut cmd = std::process::Command::new(loom_bin());
    cmd.arg("--graph").arg(tmp).args(args);
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("spawn loom {:?}: {e}", args));
    assert!(
        out.status.success(),
        "loom {:?} failed: {}\n{}",
        args,
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn finding_add_creates_asserted_finding_without_inbox() {
    // Contract: `loom finding add` creates an asserted Finding node whose body
    // carries source/kind/evidence/impact/confidence, and creates NO inbox item.
    let tmp = Tmp::new();
    tmp.write("src/x.rs", "pub fn x() {}\n");
    loom_init(tmp.path(), Some("t"));
    loom_run_ok(tmp.path(), &["codefile", "add", "src/x.rs"]);

    let out = loom_json(
        tmp.path(),
        &[
            "finding",
            "add",
            "duplicated route-key normalization",
            "--source",
            "code_audit",
            "--kind",
            "duplication",
            "--file",
            "src/x.rs",
            "--evidence",
            "src/x.rs:1 — duplicate normalization observed",
            "--impact",
            "future route-key changes can drift",
            "--confidence",
            "0.8",
        ],
    );

    let finding = out
        .get("finding")
        .expect("FINDING ADD: output must contain 'finding' key");
    assert_eq!(
        finding.get("type").and_then(|v| v.as_str()),
        Some("finding"),
        "FINDING ADD: node type must be 'finding'"
    );
    assert_eq!(
        finding.get("truth_class").and_then(|v| v.as_str()),
        Some("asserted"),
        "FINDING ADD: truth_class must be 'asserted'"
    );
    let body = finding
        .get("body")
        .expect("FINDING ADD: node must have body");
    assert_eq!(
        body.get("source").and_then(|v| v.as_str()),
        Some("code_audit")
    );
    assert_eq!(
        body.get("kind").and_then(|v| v.as_str()),
        Some("duplication")
    );
    assert_eq!(
        body.get("evidence").and_then(|v| v.as_str()),
        Some("src/x.rs:1 — duplicate normalization observed")
    );
    assert_eq!(body.get("confidence").and_then(|v| v.as_f64()), Some(0.8));

    // No inbox item must be created by `finding add`.
    let inbox = loom_json(tmp.path(), &["inbox", "list"]);
    assert_eq!(
        inbox.as_array().map(|a| a.len()),
        Some(0),
        "FINDING ADD: inbox must be empty after finding add, got: {}",
        inbox
    );
}

#[test]
fn finding_verdict_gate_accepts_expanded_vocabulary_via_cli() {
    // Contract: the `loom finding verdict` gate accepts all six vocabulary words
    // and rejects unknown ones. Acceptance is tested through the real CLI binary
    // (which calls adjudicate_finding → validate_finding_verdict), not the raw
    // store method that bypasses the gate.
    // Classification contract: needed/blocked stay open; justified/rejected/
    // deferred/duplicate are resolved when not stale.
    let tmp = Tmp::new();
    tmp.write("src/x.rs", "pub fn x() {}\n");
    loom_init(tmp.path(), Some("t"));
    loom_run_ok(tmp.path(), &["codefile", "add", "src/x.rs"]);

    // Create an asserted finding via CLI so we have a real finding ID.
    let out = loom_json(
        tmp.path(),
        &[
            "finding",
            "add",
            "gate check finding",
            "--source",
            "code_audit",
            "--kind",
            "code_audit",
            "--file",
            "src/x.rs",
            "--evidence",
            "src/x.rs:1 — observed",
            "--impact",
            "matters",
            "--confidence",
            "0.7",
        ],
    );
    let id = out["finding"]["id"]
        .as_str()
        .expect("VERDICT GATE: finding add must emit id")
        .to_string();
    let short = &id[..8.min(id.len())];

    // Unknown verdict must be rejected by the gate (non-zero exit).
    let bad = std::process::Command::new(loom_bin())
        .arg("--graph")
        .arg(tmp.path())
        .args(["finding", "verdict", short, "bogus", "--reason", "test"])
        .output()
        .unwrap();
    assert!(
        !bad.status.success(),
        "VERDICT GATE: unknown verdict 'bogus' must exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&bad.stderr);
    assert!(
        stderr.contains("needed") && stderr.contains("rejected") && stderr.contains("duplicate"),
        "VERDICT GATE: error must name allowed verdicts, got: {stderr}"
    );

    // Each valid verdict must be accepted and produce the correct open/resolved
    // classification. Call through the CLI so the gate is exercised every time.
    for (verdict, expect_open) in [
        ("needed", true),
        ("blocked", true),
        ("justified", false),
        ("rejected", false),
        ("deferred", false),
        ("duplicate", false),
    ] {
        // loom_run_ok asserts success; any gate rejection will panic the test.
        loom_run_ok(
            tmp.path(),
            &[
                "finding",
                "verdict",
                short,
                verdict,
                "--reason",
                "substantive reason",
            ],
        );
        // Read classification back through the in-process store (same disk).
        let store = loom::store::Store::open(tmp.path()).unwrap();
        let pulse = workitem::graph_state(&store).unwrap();
        if expect_open {
            assert_eq!(
                pulse.open_findings, 1,
                "verdict '{verdict}' must keep finding open"
            );
            assert_eq!(pulse.resolved_findings, 0);
        } else {
            assert_eq!(
                pulse.open_findings, 0,
                "verdict '{verdict}' must resolve the finding"
            );
            assert_eq!(pulse.resolved_findings, 1);
        }
    }
}
