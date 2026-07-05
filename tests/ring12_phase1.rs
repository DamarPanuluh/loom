//! Ring 12 phase 1 — churn/concurrency refactor contracts.
//!
//! Each test defends one observable contract of the just-landed refactor:
//! the atomic `loom apply` batch (one call, one transaction, full rollback on
//! any rejected item), idempotent re-record/reapply (no timestamp churn, no
//! duplicate edges), the fail-closed evidence gate that the idempotence no-op
//! cannot bypass, the read-only write boundary, the unlocked CLI scan path
//! (config-read → subprocess with no lock → short write reconcile), and the
//! tracked-export refresh rule. Every test is hermetic (own `Tmp`).

mod common;
use common::*;

use loom::cli::{Cli, Command};
use loom::commands;
use loom::model::{EdgeKind, InspectionStatus, NodeType, TruthClass};
use loom::scan;
use loom::store::Store;
use loom::sync;
use loom::travel;
use serde_json::json;

// ---- shared helpers --------------------------------------------------------

/// Seed a CodeFile node and return its id.
fn seed_codefile(store: &Store, path: &str) -> String {
    store
        .add_node(NodeType::CodeFile, path, "", "", json!({}))
        .unwrap()
        .id
}

/// Count intents in the store.
fn intent_count(store: &Store) -> usize {
    store
        .list_nodes(Some(NodeType::Intent), usize::MAX)
        .unwrap()
        .len()
}

/// Count all edges in the store.
fn edge_count(store: &Store) -> usize {
    store.list_edges(None, usize::MAX).unwrap().len()
}

/// Dispatch a `loom apply` batch file against the graph at `root` via the real
/// in-process write boundary. Caller must NOT be holding a Store on `root`.
fn apply_batch(root: &std::path::Path, file: &std::path::Path) -> anyhow::Result<()> {
    commands::run(Cli {
        graph: Some(root.to_path_buf()),
        json: false,
        command: Some(Command::Apply {
            file: file.to_path_buf(),
        }),
    })
}

/// The single Implements grounding edge between `intent` and `codefile`, if any.
fn implements_edge(store: &Store, intent_id: &str, codefile_id: &str) -> Option<loom::model::Edge> {
    store
        .edges_with(
            Some(EdgeKind::Implements),
            Some(intent_id),
            Some(codefile_id),
        )
        .unwrap()
        .into_iter()
        .next()
}

// =========================================================================
// 1. ONE apply call performs the whole multi-mutation session.
// =========================================================================
#[test]
fn apply_batch_creates_intents_groundings_relationships_and_records_verdicts() {
    // Contract: a single `loom apply` dispatch creates intents, grounds one in
    // a codefile with an inline Passing verdict, and adds a `requires`
    // relationship — all through the real write boundary in one transaction.
    let tmp = Tmp::new();
    let root = tmp.path();

    let store = Store::init(root, Some("t"), false).unwrap();
    let _codefile_id = seed_codefile(&store, "src/a.rs");
    drop(store);

    let batch = tmp.path().join("batch.json");
    tmp.write(
        "batch.json",
        r#"{
  "intents": [
    { "name": "payment can be captured", "description": "capturing a payment settles the charge", "level": "feature", "lifecycle": "planned" },
    { "name": "payment can be refunded",  "description": "refunding a payment returns the charge", "level": "feature", "lifecycle": "planned" }
  ],
  "groundings": [
    { "intent": "payment can be captured", "codefile": "src/a.rs", "locator": "capture", "role": "realizes",
      "verdict": { "verdict": "ground", "criterion": "capture() settles the charge", "evidence": "test capture_settles passes", "confidence": 0.9 } }
  ],
  "relationships": [
    { "kind": "requires", "from": "payment can be refunded", "to": "payment can be captured",
      "verdict": { "verdict": "ground", "criterion": "refund needs a prior capture", "evidence": "refund path reads capture record" } }
  ]
}
"#,
    );

    apply_batch(root, &batch).expect("contract 1: a well-formed batch applies");

    let store = Store::open(root).unwrap();
    assert_eq!(
        intent_count(&store),
        2,
        "contract 1: the two declared intents persisted"
    );

    let capture = store
        .resolve_node("payment can be captured", Some(NodeType::Intent))
        .expect("contract 1: capture intent resolves");
    let codefile = store
        .resolve_node("src/a.rs", Some(NodeType::CodeFile))
        .expect("contract 1: codefile resolves");
    let edge = implements_edge(&store, &capture.id, &codefile.id)
        .expect("contract 1: the implements grounding edge exists");
    assert_eq!(
        edge.kind,
        EdgeKind::Implements,
        "contract 1: grounding edge is Implements"
    );
    assert_eq!(
        edge.status,
        InspectionStatus::Passing,
        "contract 1: inline ground verdict set the edge to Passing"
    );
    assert_eq!(
        edge.criterion, "capture() settles the charge",
        "contract 1: the recorded criterion persisted on the edge"
    );
    assert_eq!(
        edge.evidence, "test capture_settles passes",
        "contract 1: the recorded evidence persisted on the edge"
    );

    // The `requires` relationship edge between the two intents.
    let refund = store
        .resolve_node("payment can be refunded", Some(NodeType::Intent))
        .expect("contract 1: refund intent resolves");
    let rels = store
        .edges_with(
            Some(EdgeKind::Requires),
            Some(&refund.id),
            Some(&capture.id),
        )
        .unwrap();
    assert_eq!(
        rels.len(),
        1,
        "contract 1: exactly one requires edge between the two intents"
    );
}

// =========================================================================
// 2. Atomicity: a rejected item rolls the whole batch back.
// =========================================================================
#[test]
fn apply_is_atomic_a_rejected_item_rolls_back_the_whole_batch() {
    // Contract: the entire batch is one transaction. A relationship carrying a
    // `ground` verdict with EMPTY evidence violates the evidence gate inside
    // record_verdict, so the whole batch rolls back — even the valid intents
    // declared earlier in the same batch must NOT persist.
    let tmp = Tmp::new();
    let root = tmp.path();

    let store = Store::init(root, Some("t"), false).unwrap();
    drop(store);

    let batch = tmp.path().join("bad.json");
    tmp.write(
        "bad.json",
        r#"{
  "intents": [
    { "name": "alpha works", "description": "alpha behavior", "level": "feature", "lifecycle": "planned" },
    { "name": "beta works",  "description": "beta behavior",  "level": "feature", "lifecycle": "planned" }
  ],
  "relationships": [
    { "kind": "requires", "from": "beta works", "to": "alpha works",
      "verdict": { "verdict": "ground", "criterion": "real reason", "evidence": "" } }
  ]
}
"#,
    );

    let res = apply_batch(root, &batch);
    assert!(
        res.is_err(),
        "contract 2: a batch with empty-evidence verdict must be rejected, got Ok"
    );

    let store = Store::open(root).unwrap();
    assert_eq!(
        intent_count(&store),
        0,
        "contract 2: atomic rollback left zero intents (valid intents from the same batch did NOT persist)"
    );
    assert_eq!(
        edge_count(&store),
        0,
        "contract 2: atomic rollback left zero edges"
    );
}

// =========================================================================
// 3. Edges and verdicts are idempotent on reapply.
// =========================================================================
#[test]
fn apply_edges_and_verdicts_are_idempotent_on_reapply() {
    // Contract: groundings are find-or-create and verdicts are idempotent, so
    // re-applying the same grounding with an identical verdict adds neither a
    // duplicate edge nor a duplicate intent.
    let tmp = Tmp::new();
    let root = tmp.path();

    let store = Store::init(root, Some("t"), false).unwrap();
    let _codefile_id = seed_codefile(&store, "src/a.rs");
    drop(store);

    let first = tmp.path().join("first.json");
    tmp.write(
        "first.json",
        r#"{
  "intents": [
    { "name": "gamma works", "description": "gamma behavior", "level": "feature", "lifecycle": "planned" }
  ],
  "groundings": [
    { "intent": "gamma works", "codefile": "src/a.rs", "locator": "g", "role": "realizes",
      "verdict": { "verdict": "ground", "criterion": "gamma criterion", "evidence": "gamma evidence", "confidence": 0.9 } }
  ]
}
"#,
    );
    apply_batch(root, &first).expect("contract 3: first apply succeeds");

    let store = Store::open(root).unwrap();
    let intents_before = intent_count(&store);
    let edges_before = edge_count(&store);
    drop(store);
    assert_eq!(
        intents_before, 1,
        "contract 3: first apply created one intent"
    );
    assert!(
        edges_before >= 1,
        "contract 3: first apply created the grounding edge"
    );

    // Edge-only reapply: same grounding + identical verdict, NO intents section.
    let second = tmp.path().join("second.json");
    tmp.write(
        "second.json",
        r#"{
  "groundings": [
    { "intent": "gamma works", "codefile": "src/a.rs", "locator": "g", "role": "realizes",
      "verdict": { "verdict": "ground", "criterion": "gamma criterion", "evidence": "gamma evidence", "confidence": 0.9 } }
  ]
}
"#,
    );
    apply_batch(root, &second).expect("contract 3: idempotent reapply succeeds");

    let store = Store::open(root).unwrap();
    assert_eq!(
        intent_count(&store),
        intents_before,
        "contract 3: reapplying a grounding added no duplicate intent"
    );
    assert_eq!(
        edge_count(&store),
        edges_before,
        "contract 3: find-or-create grounding added no duplicate edge"
    );
}

// =========================================================================
// 4. Reapplying intent declarations is rejected (ambiguous resolve) and leaves the graph unchanged.
// =========================================================================
#[test]
fn apply_reapplying_intent_declarations_is_rejected_and_leaves_graph_unchanged() {
    // Contract: intent creation is create-only (no upsert). Reapplying the same
    // batch creates a second intent with the same name inside the transaction;
    // the grounding's later resolve_node then hits an ambiguous-name error,
    // which rolls the whole batch back — so the graph is left unchanged.
    let tmp = Tmp::new();
    let root = tmp.path();

    let store = Store::init(root, Some("t"), false).unwrap();
    let _codefile_id = seed_codefile(&store, "src/a.rs");
    drop(store);

    let batch = tmp.path().join("dup.json");
    tmp.write(
        "dup.json",
        r#"{
  "intents": [
    { "name": "delta works", "description": "delta behavior", "level": "feature", "lifecycle": "planned" }
  ],
  "groundings": [
    { "intent": "delta works", "codefile": "src/a.rs", "locator": "d", "role": "realizes",
      "verdict": { "verdict": "ground", "criterion": "delta criterion", "evidence": "delta evidence", "confidence": 0.9 } }
  ]
}
"#,
    );
    apply_batch(root, &batch).expect("contract 4: first apply succeeds");

    let store = Store::open(root).unwrap();
    let intents_before = intent_count(&store);
    let edges_before = edge_count(&store);
    drop(store);
    assert_eq!(
        intents_before, 1,
        "contract 4: first apply created one intent"
    );
    assert_eq!(
        edges_before, 1,
        "contract 4: first apply created one grounding edge"
    );

    // Reapply the SAME batch. The duplicate intent is created inside the txn,
    // then the grounding's resolve_node fails on the ambiguous name, rolling
    // the whole batch back.
    let res = apply_batch(root, &batch);
    assert!(
        res.is_err(),
        "contract 4: reapplying intent declarations must be rejected (ambiguous resolve), got Ok"
    );

    let store = Store::open(root).unwrap();
    assert_eq!(
        intent_count(&store),
        intents_before,
        "contract 4: atomic rollback left the intent count unchanged (no duplicate created)"
    );
    assert_eq!(
        edge_count(&store),
        edges_before,
        "contract 4: atomic rollback left the edge count unchanged"
    );
}

// =========================================================================
// 5. Identical re-record is a no-op preserving updated_at.
// =========================================================================
#[test]
fn record_verdict_identical_rerecord_is_a_noop_preserving_updated_at() {
    // Contract: record_verdict no-ops on an identical verdict (same status,
    // criterion, evidence, confidence, inspected_by), so re-recording does
    // NOT bump updated_at — the exported timestamp cannot churn on a re-apply.
    let tmp = Tmp::new();
    let root = tmp.path();
    let store = Store::init(root, Some("t"), false).unwrap();

    let intent = store
        .add_node(
            NodeType::Intent,
            "epsilon works",
            "epsilon behavior",
            "planned",
            json!({}),
        )
        .unwrap();
    let codefile_id = seed_codefile(&store, "src/e.rs");
    let edge = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &codefile_id,
            TruthClass::Asserted,
        )
        .unwrap();

    store
        .record_verdict(
            &edge.id,
            InspectionStatus::Passing,
            "epsilon criterion",
            "epsilon evidence",
            0.9,
            "llm",
        )
        .unwrap();
    let after_first = store.get_edge(&edge.id).unwrap().unwrap();
    let ts = after_first.updated_at.clone();
    assert_eq!(
        after_first.status,
        InspectionStatus::Passing,
        "contract 5: first record set Passing"
    );

    // Advance past the millisecond timestamp tick so a non-noop update would
    // produce a DIFFERENT updated_at and redden this test (the no-op must hold).
    std::thread::sleep(std::time::Duration::from_millis(20));
    // Re-record the IDENTICAL verdict.
    store
        .record_verdict(
            &edge.id,
            InspectionStatus::Passing,
            "epsilon criterion",
            "epsilon evidence",
            0.9,
            "llm",
        )
        .unwrap();
    let after_second = store.get_edge(&edge.id).unwrap().unwrap();
    assert_eq!(
        after_second.updated_at, ts,
        "contract 5: identical re-record did not bump updated_at"
    );
    assert_eq!(
        after_second.status,
        InspectionStatus::Passing,
        "contract 5: status unchanged after identical re-record"
    );
    assert_eq!(
        after_second.criterion, "epsilon criterion",
        "contract 5: criterion unchanged after identical re-record"
    );
    assert_eq!(
        after_second.evidence, "epsilon evidence",
        "contract 5: evidence unchanged after identical re-record"
    );
}

// =========================================================================
// 6. The evidence gate still rejects placeholder evidence after a settled verdict.
// =========================================================================
#[test]
fn record_verdict_still_rejects_placeholder_evidence_after_a_settled_verdict() {
    // Contract: the idempotence no-op runs AFTER validation, so a settled edge
    // still fails closed when re-recorded with EMPTY evidence — the no-op
    // never bypasses the gate.
    let tmp = Tmp::new();
    let root = tmp.path();
    let store = Store::init(root, Some("t"), false).unwrap();

    let intent = store
        .add_node(
            NodeType::Intent,
            "zeta works",
            "zeta behavior",
            "planned",
            json!({}),
        )
        .unwrap();
    let codefile = store
        .add_node(NodeType::CodeFile, "src/z.rs", "", "", json!({}))
        .unwrap();
    let edge = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &codefile.id,
            TruthClass::Asserted,
        )
        .unwrap();

    // Settle the edge with a valid Passing verdict.
    store
        .record_verdict(
            &edge.id,
            InspectionStatus::Passing,
            "zeta criterion",
            "zeta evidence",
            0.9,
            "llm",
        )
        .unwrap();
    assert_eq!(
        store.get_edge(&edge.id).unwrap().unwrap().status,
        InspectionStatus::Passing,
        "contract 6: edge settled to Passing before the bad re-record"
    );

    // Attempt to re-record a Passing verdict with EMPTY evidence.
    let res = store.record_verdict(
        &edge.id,
        InspectionStatus::Passing,
        "zeta criterion",
        "",
        0.9,
        "llm",
    );
    assert!(
        res.is_err(),
        "contract 6: re-recording with empty evidence must be rejected even on a settled edge"
    );

    // The settled state is untouched.
    let after = store.get_edge(&edge.id).unwrap().unwrap();
    assert_eq!(
        after.status,
        InspectionStatus::Passing,
        "contract 6: settled edge state survived the rejected re-record"
    );
    assert_eq!(
        after.evidence, "zeta evidence",
        "contract 6: settled edge evidence survived the rejected re-record"
    );
}

// =========================================================================
// 7. open_read forbids writes but allows reads.
// =========================================================================
#[test]
fn open_read_forbids_writes_but_allows_reads() {
    // Contract: Store::open_read takes a SHARED lock and sets query_only, so a
    // read succeeds but any write method returns Err.
    let tmp = Tmp::new();
    let root = tmp.path();

    // Seed under a write store, then drop it so the read open can take the lock.
    let store = Store::init(root, Some("t"), false).unwrap();
    store
        .add_node(
            NodeType::Intent,
            "seed intent",
            "seed",
            "planned",
            json!({}),
        )
        .unwrap();
    drop(store);

    let ro = Store::open_read(root).expect("contract 7: open_read succeeds on an existing graph");

    // A read works.
    let read_res = ro.list_nodes(Some(NodeType::Intent), usize::MAX);
    assert!(
        read_res.is_ok(),
        "contract 7: a read under open_read succeeds"
    );
    assert_eq!(
        read_res.unwrap().len(),
        1,
        "contract 7: the seeded intent is visible to the read-only store"
    );

    // A write fails (query_only).
    let write_res = ro.add_node(NodeType::Intent, "x", "d", "planned", json!({}));
    assert!(
        write_res.is_err(),
        "contract 7: a write under open_read must be rejected (query_only)"
    );
}

// =========================================================================
// 8. scan::run_unlocked creates then resolves findings (the CLI path).
// =========================================================================
#[test]
fn scan_run_unlocked_creates_then_resolves_findings() {
    // Contract: the unlocked scan path (config read under a shared lock,
    // subprocess with NO lock, then a short exclusive write to reconcile)
    // creates a finding from a diagnostic and resolves it on a clean re-run.
    // The existing ring10 tests cover scan::run (locked); this covers the CLI
    // path that the refactor introduced.
    let tmp = Tmp::new();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "pub fn a() {}\n").unwrap();

    // Seed the codefile registry + adapter under a write store, then DROP it —
    // run_unlocked opens its own stores and a same-process lock conflict would
    // deadlock if we kept ours alive.
    let store = Store::init(root, Some("t"), false).unwrap();
    seed_codefile(&store, "src/a.rs");
    scan::add_adapter(
        &store,
        "lint",
        "printf 'src/a.rs:1: boom\\n'",
        None,
        scan::ScanFormat::Lines,
    )
    .unwrap();
    drop(store);

    // First unlocked run: one diagnostic creates one new finding.
    let first = scan::run_unlocked(root, Some("lint")).expect("contract 8: first run_unlocked");
    assert_eq!(
        first.new_findings, 1,
        "contract 8: first run_unlocked reports one new finding"
    );
    let store = Store::open(root).unwrap();
    let findings = store
        .list_nodes(Some(NodeType::Finding), usize::MAX)
        .unwrap();
    assert_eq!(
        findings.len(),
        1,
        "contract 8: a derived Finding node exists after the first run"
    );
    assert!(
        findings[0].name.contains("src/a.rs:1 boom"),
        "contract 8: the finding name carries the path, line, and message"
    );
    drop(store);

    // Reconfigure the adapter to emit nothing (a healthy run), then re-run.
    let store = Store::open(root).unwrap();
    scan::update_adapter(&store, "lint", Some("printf ''"), None, None).unwrap();
    drop(store);

    let second = scan::run_unlocked(root, Some("lint")).expect("contract 8: second run_unlocked");
    assert!(
        second.resolved_findings >= 1,
        "contract 8: clean re-run resolves the prior finding (got {})",
        second.resolved_findings
    );
    let store = Store::open(root).unwrap();
    let after = store
        .list_nodes(Some(NodeType::Finding), usize::MAX)
        .unwrap();
    assert!(
        after.is_empty(),
        "contract 8: no findings remain after the clean re-run"
    );
}

// =========================================================================
// 9. refresh_export_if_tracked only rewrites a tracked, drifted export.
// =========================================================================
#[test]
fn refresh_export_if_tracked_only_rewrites_a_tracked_drifted_export() {
    // Contract: refresh_export_if_tracked rewrites loom.graph.json ONLY when it
    // already exists AND has drifted. No file → false and nothing created;
    // drifted tracked file → true and the file becomes fresh; already fresh →
    // false with no rewrite.
    let tmp = Tmp::new();
    let root = tmp.path();
    let store = Store::init(root, Some("t"), false).unwrap();
    store
        .add_node(
            NodeType::Intent,
            "eta works",
            "eta behavior",
            "planned",
            json!({}),
        )
        .unwrap();

    let export_path = root.join(loom::GRAPH_EXPORT);

    // (a) No export present: returns false and creates nothing.
    assert!(
        !export_path.exists(),
        "contract 9a: no export file present before any refresh"
    );
    let a = travel::refresh_export_if_tracked(&store).unwrap();
    assert!(
        !a,
        "contract 9a: refresh with no tracked export returns false"
    );
    assert!(
        !export_path.exists(),
        "contract 9a: refresh with no tracked export created no file"
    );

    // (b) Export, then drift (add an intent), then refresh: true and now fresh.
    travel::export_to_file(&store).unwrap();
    assert!(export_path.exists(), "contract 9b: export file written");
    store
        .add_node(
            NodeType::Intent,
            "theta works",
            "theta behavior",
            "planned",
            json!({}),
        )
        .unwrap();
    assert!(
        !travel::export_is_fresh(&store).unwrap(),
        "contract 9b: adding an intent drifted the export"
    );
    let b = travel::refresh_export_if_tracked(&store).unwrap();
    assert!(b, "contract 9b: refresh rewrites a tracked, drifted export");
    assert!(
        travel::export_is_fresh(&store).unwrap(),
        "contract 9b: on-disk export equals a fresh export after refresh"
    );

    // (c) Immediately again: already fresh → false, no rewrite.
    let c = travel::refresh_export_if_tracked(&store).unwrap();
    assert!(
        !c,
        "contract 9c: refresh on an already-fresh export returns false"
    );
}

// =========================================================================
// 10. Symbol-level findings fire on a production callable that exceeds the
//     default thresholds, and thresholds provably drive derivation.
// =========================================================================
#[test]
fn symbol_findings_fire_and_are_gated_by_thresholds() {
    // Contract: a source callable with 7 args (>6), nesting depth 6 (>5), and a
    // span of 121 lines (>120) yields exactly the excess_args, deep_nesting,
    // and large_symbol findings; complexity stays under 20 so complex_symbol
    // does NOT fire (asserting absence guards against a flipped condition
    // silently adding noise). Saving looser thresholds that absorb the same
    // symbol and re-syncing removes all three — the thresholds, not the code,
    // drive the derivation.
    let tmp = Tmp::new();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/big.rs"), offending_fn()).unwrap();

    let store = Store::init(root, Some("t"), false).unwrap();
    seed_codefile(&store, "src/big.rs");
    sync::run(&store, root).expect("contract 10: first sync");

    let kinds = finding_kinds(&store);
    assert!(
        kinds.contains(&"excess_args".to_string()),
        "contract 10: 7 args fires excess_args, got {kinds:?}"
    );
    assert!(
        kinds.contains(&"deep_nesting".to_string()),
        "contract 10: nesting 6 fires deep_nesting, got {kinds:?}"
    );
    assert!(
        kinds.contains(&"large_symbol".to_string()),
        "contract 10: 121-line span fires large_symbol, got {kinds:?}"
    );
    assert!(
        !kinds.contains(&"complex_symbol".to_string()),
        "contract 10: low complexity must NOT fire complex_symbol, got {kinds:?}"
    );

    // Loosen the three offending gates so the same code clears them, then
    // re-sync: the findings must disappear — proving thresholds drive the
    // derivation rather than the code alone.
    let loose = loom::thresholds::Thresholds {
        max_file_loc: 600,
        max_symbol_complexity: 20,
        max_symbol_loc: 500,
        max_nesting: 10,
        max_args: 10,
        max_file_owners: 2,
    };
    loom::thresholds::save(&store, &loose).unwrap();
    sync::run(&store, root).expect("contract 10: second sync under loose thresholds");
    let after = finding_kinds(&store);
    assert!(
        !after.contains(&"excess_args".to_string()),
        "contract 10: max_args 10 removes excess_args, got {after:?}"
    );
    assert!(
        !after.contains(&"deep_nesting".to_string()),
        "contract 10: max_nesting 10 removes deep_nesting, got {after:?}"
    );
    assert!(
        !after.contains(&"large_symbol".to_string()),
        "contract 10: max_symbol_loc 500 removes large_symbol, got {after:?}"
    );
}

// =========================================================================
// 11. calibrate proposes repo-fitted gates >= floors and errors on no codefiles.
// =========================================================================
#[test]
fn calibrate_proposes_above_floors_and_errors_without_codefiles() {
    // Contract: calibrate reads the registered codefiles on disk, samples
    // their metrics, and proposes per-floor clamped gates. The offending
    // fixture yields files_sampled/symbols_sampled > 0 and every proposed
    // gate >= its floor (file 200, complexity 10, symbol loc 60, nesting 4,
    // args 5). A store with no readable codefiles errors rather than proposing
    // from nothing.
    let tmp = Tmp::new();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/big.rs"), offending_fn()).unwrap();

    let store = Store::init(root, Some("t"), false).unwrap();
    seed_codefile(&store, "src/big.rs");
    let cal = loom::thresholds::calibrate(&store, root)
        .expect("contract 11: calibrate on a populated repo");
    assert!(
        cal.files_sampled > 0,
        "contract 11: at least one file sampled, got {}",
        cal.files_sampled
    );
    assert!(
        cal.symbols_sampled > 0,
        "contract 11: at least one callable sampled, got {}",
        cal.symbols_sampled
    );
    assert!(
        cal.proposed.max_file_loc >= 200,
        "contract 11: proposed max_file_loc clamps to floor 200, got {}",
        cal.proposed.max_file_loc
    );
    assert!(
        cal.proposed.max_symbol_complexity >= 10,
        "contract 11: proposed max_symbol_complexity clamps to floor 10, got {}",
        cal.proposed.max_symbol_complexity
    );
    assert!(
        cal.proposed.max_symbol_loc >= 60,
        "contract 11: proposed max_symbol_loc clamps to floor 60, got {}",
        cal.proposed.max_symbol_loc
    );
    assert!(
        cal.proposed.max_nesting >= 4,
        "contract 11: proposed max_nesting clamps to floor 4, got {}",
        cal.proposed.max_nesting
    );
    assert!(
        cal.proposed.max_args >= 5,
        "contract 11: proposed max_args clamps to floor 5, got {}",
        cal.proposed.max_args
    );
    drop(store);

    // Registered but unreadable: a codefile pointing at a path that does not
    // exist on disk is "no readable codefile" — calibrate errors rather than
    // proposing from nothing. (This is the stronger case: the registry is
    // non-empty, so the error is specifically the read guard, not an empty
    // store.)
    let empty = Tmp::new();
    let empty_store = Store::init(empty.path(), Some("t"), false).unwrap();
    seed_codefile(&empty_store, "src/missing.rs");
    let err = loom::thresholds::calibrate(&empty_store, empty.path())
        .expect_err("contract 11: unreadable codefile must error");
    let msg = format!("{err}");
    assert!(
        msg.contains("codefile"),
        "contract 11: error explains the missing codefile, got: {msg}"
    );
}

// =========================================================================
// 12. Symbol-level findings are suppressed for test-role paths.
// =========================================================================
#[test]
fn test_role_path_yields_no_symbol_findings() {
    // Contract: symbol-level detectors apply to production callables only
    // (Role::Source). The same offending function placed under tests/ has
    // Role::Test, so no excess_args/deep_nesting/large_symbol findings appear
    // — even though every metric still exceeds the default gates.
    let tmp = Tmp::new();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(root.join("tests/x.rs"), offending_fn()).unwrap();

    let store = Store::init(root, Some("t"), false).unwrap();
    seed_codefile(&store, "tests/x.rs");
    sync::run(&store, root).expect("contract 12: sync over test-role file");
    let kinds = finding_kinds(&store);
    assert!(
        !kinds.contains(&"excess_args".to_string()),
        "contract 12: test-role path suppresses excess_args, got {kinds:?}"
    );
    assert!(
        !kinds.contains(&"deep_nesting".to_string()),
        "contract 12: test-role path suppresses deep_nesting, got {kinds:?}"
    );
    assert!(
        !kinds.contains(&"large_symbol".to_string()),
        "contract 12: test-role path suppresses large_symbol, got {kinds:?}"
    );
    assert!(
        !kinds.contains(&"complex_symbol".to_string()),
        "contract 12: test-role path suppresses complex_symbol, got {kinds:?}"
    );
}

// ---- helpers for the threshold contracts ----------------------------------

/// The `kind` strings of every derived Finding node in the store. A finding's
/// kind is carried on the node's `status` field (see sync::rebuild_findings),
/// so collecting it is a pure read of the public node surface.
fn finding_kinds(store: &Store) -> Vec<String> {
    store
        .list_nodes(Some(NodeType::Finding), usize::MAX)
        .unwrap()
        .into_iter()
        .map(|n| n.status)
        .collect()
}

/// A Rust source callable that trips the three symbol-level default gates and
/// nothing else: 7 declared args (>6), 6 levels of nested `if` (>5), and a
/// 121-line body span (>120). Complexity stays low (only the 6 branches, ~7)
/// so `complex_symbol` does not fire. The file is well under the 600-line file
/// gate and contains no `unwrap()`/`panic!`, so no oversized_file or
/// panic_marker finding pollutes the assertions.
fn offending_fn() -> String {
    let mut s = String::new();
    s.push_str("pub fn heavy(\n");
    // 7 declared parameters (no receiver in a free fn), each on its own line.
    for i in 1..=7 {
        s.push_str(&format!("    a{i}: u32,\n"));
    }
    s.push_str(") -> u32 {\n");
    // 6 nested `if` blocks → max_nesting 6. Each level pads with simple
    // statements so the function spans 121+ lines without adding branches.
    let pad = "    let _z = 0u32;\n".repeat(18);
    for depth in 0..6 {
        let indent = "    ".repeat(depth + 1);
        s.push_str(&format!("{indent}if a1 > {depth} {{\n"));
        s.push_str(
            &pad.lines()
                .map(|l| format!("{indent}{l}\n"))
                .collect::<String>(),
        );
    }
    s.push_str("        let _r = a1 + a2 + a3 + a4 + a5 + a6 + a7;\n");
    // Close the 6 opened blocks.
    for depth in (0..6).rev() {
        let indent = "    ".repeat(depth + 1);
        s.push_str(&format!("{indent}}}\n"));
    }
    s.push_str("    0\n");
    s.push_str("}\n");
    s
}

#[test]
fn apply_batch_adjudicates_findings() {
    // Contract: a single apply batch can adjudicate multiple durable Findings.
    // Two unrelated pairs co-own two different files, yielding two independent
    // overlapping_ownership findings that are both triaged by one dispatch.
    let tmp = Tmp::new();
    let root = tmp.path();
    tmp.write("src/one.rs", "pub fn one() {}\n");
    tmp.write("src/two.rs", "pub fn two() {}\n");

    let store = Store::init(root, Some("t"), false).unwrap();
    let one = seed_codefile(&store, "src/one.rs");
    let two = seed_codefile(&store, "src/two.rs");
    let one_a = store
        .add_node(NodeType::Intent, "one alpha", "", "implemented", json!({}))
        .unwrap();
    let one_b = store
        .add_node(NodeType::Intent, "one beta", "", "implemented", json!({}))
        .unwrap();
    let two_a = store
        .add_node(NodeType::Intent, "two alpha", "", "implemented", json!({}))
        .unwrap();
    let two_b = store
        .add_node(NodeType::Intent, "two beta", "", "implemented", json!({}))
        .unwrap();
    store
        .add_edge(EdgeKind::Implements, &one_a.id, &one, TruthClass::Asserted)
        .unwrap();
    store
        .add_edge(EdgeKind::Implements, &one_b.id, &one, TruthClass::Asserted)
        .unwrap();
    store
        .add_edge(EdgeKind::Implements, &two_a.id, &two, TruthClass::Asserted)
        .unwrap();
    store
        .add_edge(EdgeKind::Implements, &two_b.id, &two, TruthClass::Asserted)
        .unwrap();

    let mut finding_ids = loom::signal::smells(&store)
        .unwrap()
        .into_iter()
        .filter(|smell| smell.kind == "overlapping_ownership")
        .map(|smell| {
            Store::derived_node_id(
                NodeType::Finding,
                &loom::signal::smell_det_key(&smell.identity),
            )
        })
        .collect::<Vec<_>>();
    finding_ids.sort();
    assert_eq!(
        finding_ids.len(),
        2,
        "apply adjudication batch setup: expected two overlapping ownership smells"
    );

    loom::sync::run(&store, root)
        .expect("apply adjudication batch setup: sync materializes findings");
    assert!(store.get_node(&finding_ids[0]).unwrap().is_some());
    assert!(store.get_node(&finding_ids[1]).unwrap().is_some());
    drop(store);

    let batch = root.join("adjudications.json");
    std::fs::write(
        &batch,
        serde_json::to_string(&json!({
            "adjudications": [
                {
                    "finding": finding_ids[0],
                    "verdict": "justified",
                    "reason": "cohesive co-owners"
                },
                {
                    "finding": finding_ids[1],
                    "verdict": "needed",
                    "reason": "split these"
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    apply_batch(root, &batch).expect("apply adjudication batch: both adjudications apply");

    let store = Store::open(root).unwrap();
    assert_eq!(
        loom::signal::adjudication_of(&store, &finding_ids[0]).unwrap(),
        Some(("justified".into(), "cohesive co-owners".into())),
        "apply adjudication batch: first finding was adjudicated"
    );
    assert_eq!(
        loom::signal::adjudication_of(&store, &finding_ids[1]).unwrap(),
        Some(("needed".into(), "split these".into())),
        "apply adjudication batch: second finding was adjudicated"
    );
}

#[test]
fn apply_adjudication_batch_is_atomic_and_gated() {
    // Contract: adjudications use the same gate as `loom finding verdict` and
    // stay inside the batch transaction. A later invalid verdict vocabulary
    // rolls back an earlier valid adjudication from the same file.
    let tmp = Tmp::new();
    let root = tmp.path();
    tmp.write("src/pair.rs", "pub fn pair() {}\n");

    let store = Store::init(root, Some("t"), false).unwrap();
    let codefile = seed_codefile(&store, "src/pair.rs");
    let alpha = store
        .add_node(NodeType::Intent, "alpha pair", "", "implemented", json!({}))
        .unwrap();
    let beta = store
        .add_node(NodeType::Intent, "beta pair", "", "implemented", json!({}))
        .unwrap();
    store
        .add_edge(
            EdgeKind::Implements,
            &alpha.id,
            &codefile,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Implements,
            &beta.id,
            &codefile,
            TruthClass::Asserted,
        )
        .unwrap();

    let smell = loom::signal::smells(&store)
        .unwrap()
        .into_iter()
        .find(|smell| smell.kind == "overlapping_ownership")
        .expect("apply adjudication atomic setup: overlapping ownership smell exists");
    let finding_id = Store::derived_node_id(
        NodeType::Finding,
        &loom::signal::smell_det_key(&smell.identity),
    );
    loom::sync::run(&store, root)
        .expect("apply adjudication atomic setup: sync materializes finding");
    assert_eq!(
        loom::signal::adjudication_of(&store, &finding_id).unwrap(),
        None,
        "apply adjudication atomic setup: finding starts unadjudicated"
    );
    drop(store);

    let batch = root.join("bad_adjudications.json");
    std::fs::write(
        &batch,
        serde_json::to_string(&json!({
            "adjudications": [
                {
                    "finding": finding_id,
                    "verdict": "justified",
                    "reason": "shared implementation seam"
                },
                {
                    "finding": finding_id,
                    "verdict": "bogus",
                    "reason": "bad verdict vocabulary"
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let res = apply_batch(root, &batch);
    assert!(
        res.is_err(),
        "apply adjudication atomic: bogus finding verdict must reject the batch"
    );

    let store = Store::open(root).unwrap();
    assert_eq!(
        loom::signal::adjudication_of(&store, &finding_id).unwrap(),
        None,
        "apply adjudication atomic: valid earlier adjudication rolled back"
    );
}

#[test]
fn apply_batch_registers_vocab_and_tags() {
    // Contract: vocab registration runs before tag application inside the same
    // batch, so a newly registered term can immediately tag an intent.
    let tmp = Tmp::new();
    let root = tmp.path();

    let store = Store::init(root, Some("t"), false).unwrap();
    store
        .add_node(NodeType::Intent, "checkout", "", "implemented", json!({}))
        .unwrap();
    drop(store);

    let batch = root.join("vocab_tags.json");
    std::fs::write(
        &batch,
        serde_json::to_string(&json!({
            "vocab": [
                {
                    "term": "payments",
                    "why": "payment domain"
                }
            ],
            "tags": [
                {
                    "intent": "checkout",
                    "terms": ["payments"]
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    apply_batch(root, &batch).expect("apply vocab/tag batch: vocab then tag apply");

    let store = Store::open(root).unwrap();
    assert!(
        store.vocab_has("payments").unwrap(),
        "apply vocab/tag batch: vocab term persisted"
    );
    let checkout = store
        .resolve_node("checkout", Some(NodeType::Intent))
        .expect("apply vocab/tag batch: checkout intent resolves");
    let tags = store.snapshot().unwrap().tags;
    assert!(
        tags.iter().any(|tag| {
            tag.target_id == checkout.id
                && tag.term == "payments"
                && tag.target_kind == loom::model::TargetKind::Node
        }),
        "apply vocab/tag batch: checkout carries the newly registered payments tag"
    );
}

#[test]
fn apply_tags_batch_is_atomic_on_unregistered_term() {
    // Contract: tag application is gated on registered vocab terms, and a tag
    // failure rolls back earlier vocab registration from the same batch.
    let tmp = Tmp::new();
    let root = tmp.path();

    let store = Store::init(root, Some("t"), false).unwrap();
    store
        .add_node(NodeType::Intent, "refund", "", "implemented", json!({}))
        .unwrap();
    drop(store);

    let batch = root.join("bad_tags.json");
    std::fs::write(
        &batch,
        serde_json::to_string(&json!({
            "vocab": [
                {
                    "term": "money",
                    "why": ""
                }
            ],
            "tags": [
                {
                    "intent": "refund",
                    "terms": ["ghost_term"]
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let res = apply_batch(root, &batch);
    assert!(
        res.is_err(),
        "apply tag atomic: unregistered tag term must reject the batch"
    );

    let store = Store::open(root).unwrap();
    assert!(
        !store.vocab_has("money").unwrap(),
        "apply tag atomic: earlier vocab registration rolled back"
    );
    let refund = store
        .resolve_node("refund", Some(NodeType::Intent))
        .expect("apply tag atomic: refund intent resolves");
    let tags = store.snapshot().unwrap().tags;
    assert!(
        !tags.iter().any(|tag| tag.target_id == refund.id),
        "apply tag atomic: failing tag batch left refund untagged"
    );
}
