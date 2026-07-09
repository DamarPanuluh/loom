//! Ring 13 precision tests — symbol-scoped staleness, evidence anchoring,
//! and the vague_intent smell.
//!
//! Setup discipline: register file → baseline `sync::run` (seeds fingerprints,
//! no prior hash → no staleness) → record verdicts → edit → second `sync::run`.

use loom::evidence::{SpanStamp, EVIDENCE_SPANS_KEY};
use loom::model::{EdgeKind, InspectionStatus, NodeType, TargetKind, TruthClass};
use loom::signal::{smell_det_key, smell_has_resolving_adjudication, smells};
use loom::store::Store;
use loom::sync;

mod common;
use common::Tmp;

// ---- helpers ---------------------------------------------------------------

fn add_intent(store: &Store, name: &str, desc: &str) -> String {
    store
        .add_node(
            NodeType::Intent,
            name,
            desc,
            "planned",
            serde_json::json!({}),
        )
        .unwrap()
        .id
}

fn add_codefile(store: &Store, path: &str) -> String {
    store
        .add_node(NodeType::CodeFile, path, "", "", serde_json::json!({}))
        .unwrap()
        .id
}

/// Add an Implements edge; optionally stamp a `locator` facet.
fn ground(store: &Store, intent_id: &str, cf_id: &str, locator: Option<&str>) -> String {
    let e = store
        .add_edge(EdgeKind::Implements, intent_id, cf_id, TruthClass::Asserted)
        .unwrap();
    if let Some(loc) = locator {
        store
            .set_facet(
                &e.id,
                TargetKind::Edge,
                "locator",
                loc,
                TruthClass::Asserted,
            )
            .unwrap();
    }
    e.id
}

/// Record a Passing verdict (criterion="criterion", confidence=0.9).
fn pass(store: &Store, edge_id: &str, evidence: &str) {
    store
        .record_verdict(
            edge_id,
            InspectionStatus::Passing,
            "criterion",
            evidence,
            0.9,
            "test",
        )
        .unwrap();
}

fn edge_status(store: &Store, edge_id: &str) -> InspectionStatus {
    store.get_edge(edge_id).unwrap().unwrap().status
}

fn stale_cause(store: &Store, edge_id: &str) -> Option<String> {
    store
        .get_facet(edge_id, TargetKind::Edge, "stale_cause")
        .unwrap()
}

// ============================================================
// 1. symbol_scoped_spare
// ============================================================

/// When only `fn beta`'s body changes, the grounding whose locator resolves to
/// the UNCHANGED symbol `alpha` is spared (stays Passing; edges_spared == 1).
/// Beta's grounding is staled with a cause naming "beta". Alpha's intent is NOT
/// added to changed_intents so its Requires edge stays Passing; beta's intent IS
/// added so its Requires edge is rippled to NeedsReverification.
#[test]
fn symbol_scoped_spare() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();

    tmp.write(
        "src/lib.rs",
        "pub fn alpha() { let _ = 1; }\npub fn beta() { let _ = 2; }\n",
    );

    let cf = add_codefile(&store, "src/lib.rs");
    let i_alpha = add_intent(&store, "alpha behavior", "alpha does something");
    let i_beta = add_intent(&store, "beta behavior", "beta does something");
    let i_dep = add_intent(&store, "shared dependent", "something both require");

    let e_alpha = ground(&store, &i_alpha, &cf, Some("fn alpha"));
    let e_beta = ground(&store, &i_beta, &cf, Some("fn beta"));
    // Requires edge out of alpha — must stay Passing when alpha is spared.
    let e_alpha_req = store
        .add_edge(EdgeKind::Requires, &i_alpha, &i_dep, TruthClass::Asserted)
        .unwrap()
        .id;
    // Requires edge out of beta — must be staled when beta is added to changed_intents.
    let e_beta_req = store
        .add_edge(EdgeKind::Requires, &i_beta, &i_dep, TruthClass::Asserted)
        .unwrap()
        .id;

    // Baseline: seeds symbol_fingerprints; no prior hash → nothing staled.
    let r0 = sync::run(&store, tmp.path()).unwrap();
    assert_eq!(r0.edges_staled, 0);
    assert_eq!(r0.edges_spared, 0);

    // Record passing verdicts AFTER baseline.
    pass(&store, &e_alpha, "alpha works fine");
    pass(&store, &e_beta, "beta works fine");
    pass(
        &store,
        &e_alpha_req,
        "alpha requires dep — confirmed independent",
    );
    pass(
        &store,
        &e_beta_req,
        "beta requires dep — confirmed independent",
    );

    // Edit only beta's body; alpha is untouched.
    tmp.write(
        "src/lib.rs",
        "pub fn alpha() { let _ = 1; }\npub fn beta() { let _ = 99; }\n",
    );

    let r1 = sync::run(&store, tmp.path()).unwrap();

    assert_eq!(r1.edges_spared, 1, "alpha grounding must be spared");

    assert_eq!(
        edge_status(&store, &e_alpha),
        InspectionStatus::Passing,
        "alpha grounding must stay Passing"
    );
    assert_eq!(
        edge_status(&store, &e_beta),
        InspectionStatus::NeedsReverification,
        "beta grounding must be re-opened"
    );
    let cause = stale_cause(&store, &e_beta).expect("beta must have a stale_cause");
    assert!(
        cause.contains("beta"),
        "stale_cause must name the changed symbol: {cause}"
    );

    // Alpha was spared → NOT in changed_intents → its Requires edge is not rippled.
    assert_eq!(
        edge_status(&store, &e_alpha_req),
        InspectionStatus::Passing,
        "alpha's Requires dependent must not be staled when alpha is spared"
    );
    // Beta was staled → IS in changed_intents → its Requires edge is rippled.
    assert_eq!(
        edge_status(&store, &e_beta_req),
        InspectionStatus::NeedsReverification,
        "beta's Requires dependent must be staled when beta enters changed_intents"
    );
}

// ============================================================
// 2. no_locator_stays_file_scoped
// ============================================================

/// A grounding with no locator falls back to file-scoped staling on any content
/// change and is never spared.
#[test]
fn no_locator_stays_file_scoped() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();

    tmp.write("src/lib.rs", "pub fn alpha() { 1 }\npub fn beta() { 2 }\n");

    let cf = add_codefile(&store, "src/lib.rs");
    let i = add_intent(&store, "noloc behavior", "something");
    let e = ground(&store, &i, &cf, None); // no locator

    sync::run(&store, tmp.path()).unwrap();
    pass(&store, &e, "works fine");

    tmp.write("src/lib.rs", "pub fn alpha() { 1 }\npub fn beta() { 99 }\n");

    let r = sync::run(&store, tmp.path()).unwrap();

    assert_eq!(
        r.edges_spared, 0,
        "no-locator grounding must never be spared"
    );
    assert_eq!(
        edge_status(&store, &e),
        InspectionStatus::NeedsReverification,
    );
    let cause = stale_cause(&store, &e).expect("stale_cause must be set");
    assert!(
        cause.contains("content hash"),
        "file-scoped cause must reference the content hash: {cause}"
    );
}

// ============================================================
// 3. removed_symbol_stales
// ============================================================

/// Removing the located symbol from the file stales its grounding with a cause
/// naming that symbol (a removed symbol is in the `changed` diff set).
#[test]
fn removed_symbol_stales() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();

    tmp.write("src/lib.rs", "pub fn alpha() { 1 }\npub fn beta() { 2 }\n");

    let cf = add_codefile(&store, "src/lib.rs");
    let i = add_intent(&store, "alpha behavior", "something");
    let e = ground(&store, &i, &cf, Some("fn alpha"));

    sync::run(&store, tmp.path()).unwrap();
    pass(&store, &e, "alpha works");

    // Remove fn alpha entirely — only beta remains.
    tmp.write("src/lib.rs", "pub fn beta() { 2 }\n");

    sync::run(&store, tmp.path()).unwrap();

    assert_eq!(
        edge_status(&store, &e),
        InspectionStatus::NeedsReverification,
    );
    let cause = stale_cause(&store, &e).expect("stale_cause must be set");
    assert!(
        cause.contains("alpha"),
        "stale_cause must name the removed symbol: {cause}"
    );
}

// ============================================================
// 4. duplicate_names_fold
// ============================================================

/// Two functions with the same name fold into one fingerprint. Editing either
/// one changes that combined fingerprint so the grounding is always staled,
/// never spared.
#[test]
fn duplicate_names_fold() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();

    tmp.write(
        "src/lib.rs",
        "struct A;\nimpl A { pub fn new() -> Self { A } }\n\
         struct B;\nimpl B { pub fn new() -> Self { B } }\n",
    );

    let cf = add_codefile(&store, "src/lib.rs");
    let i = add_intent(&store, "new behavior", "something");
    let e = ground(&store, &i, &cf, Some("new"));

    sync::run(&store, tmp.path()).unwrap();
    pass(&store, &e, "new works");

    // Edit only B::new; A::new is untouched.
    tmp.write(
        "src/lib.rs",
        "struct A;\nimpl A { pub fn new() -> Self { A } }\n\
         struct B;\nimpl B { pub fn new() -> Self { B /* v2 */ } }\n",
    );

    let r = sync::run(&store, tmp.path()).unwrap();

    assert_eq!(
        r.edges_spared, 0,
        "a duplicate-named symbol must never be spared — folded fingerprint always changes"
    );
    assert_eq!(
        edge_status(&store, &e),
        InspectionStatus::NeedsReverification,
    );
    let cause = stale_cause(&store, &e).expect("stale_cause must be set");
    assert!(
        cause.contains("new"),
        "stale_cause must name the changed symbol: {cause}"
    );
}

// ============================================================
// 5. evidence_stamped_and_integrity
// ============================================================

/// `record_verdict` stamps `evidence_spans` for in-bounds file citations with
/// correct start/end; fails closed (Err, edge Uninspected) for an existing-file
/// citation beyond EOF; silently ignores nonexistent paths with no facet written.
#[test]
fn evidence_stamped_and_integrity() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();

    // Ten-line file — makes out-of-bounds obvious.
    tmp.write(
        "src/core.rs",
        "// line 1\n// line 2\n// line 3\n// line 4\n// line 5\n\
         // line 6\n// line 7\n// line 8\n// line 9\n// line 10\n",
    );

    let cf = add_codefile(&store, "src/core.rs");
    let i1 = add_intent(&store, "behavior a", "something a");
    let i2 = add_intent(&store, "behavior b", "something b");
    let i3 = add_intent(&store, "behavior c", "something c");
    let e1 = ground(&store, &i1, &cf, None);
    let e2 = ground(&store, &i2, &cf, None);
    let e3 = ground(&store, &i3, &cf, None);

    // A: valid in-bounds citation → evidence_spans stamped with correct start/end.
    pass(&store, &e1, "see src/core.rs:2-4 for proof");
    let raw = store
        .get_facet(&e1, TargetKind::Edge, EVIDENCE_SPANS_KEY)
        .unwrap()
        .expect("evidence_spans must be stamped for a valid in-bounds citation");
    let spans: Vec<SpanStamp> = serde_json::from_str(&raw).unwrap();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].file, "src/core.rs");
    assert_eq!(spans[0].start, 2);
    assert_eq!(spans[0].end, 4);

    // B: end beyond EOF → integrity gate fires → Err naming the file; edge stays Uninspected.
    let err = store
        .record_verdict(
            &e2,
            InspectionStatus::Passing,
            "criterion",
            "see src/core.rs:999",
            0.9,
            "test",
        )
        .unwrap_err();
    assert!(
        err.to_string().contains("src/core.rs"),
        "integrity error must name the cited file: {err}"
    );
    assert_eq!(
        edge_status(&store, &e2),
        InspectionStatus::Uninspected,
        "a failed verdict must not advance the edge status"
    );

    // C: nonexistent path → Ok; no evidence_spans facet written (silently ignored).
    pass(&store, &e3, "see nonexistent.rs:1 for details");
    assert!(
        store
            .get_facet(&e3, TargetKind::Edge, EVIDENCE_SPANS_KEY)
            .unwrap()
            .is_none(),
        "a nonexistent-path citation must not produce an evidence_spans facet"
    );
}

// ============================================================
// 6. evidence_refines_cause
// ============================================================

/// After a file change stales a no-locator grounding, the `stale_cause` is
/// refined with "cited evidence intact, cheap re-confirm" when all stamped spans
/// survived, or "cited evidence rewritten" when at least one was overwritten.
/// Both groundings are staled (Scope::Unknown always stales); the span status
/// only determines what text is appended to the cause.
#[test]
fn evidence_refines_cause() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();

    // 4-line file: header on line 1, function body on lines 2-4.
    tmp.write(
        "src/lib.rs",
        "// stable header\npub fn alpha() {\n    let _ = 1;\n}\n",
    );

    let cf = add_codefile(&store, "src/lib.rs");
    let i_a = add_intent(&store, "intact span behavior", "something");
    let i_b = add_intent(&store, "rewritten span behavior", "something else");
    let e_a = ground(&store, &i_a, &cf, None); // will cite line 1 (stable)
    let e_b = ground(&store, &i_b, &cf, None); // will cite lines 2-4 (body, will change)

    // Baseline sync first — seeds content hash.
    sync::run(&store, tmp.path()).unwrap();

    // Record verdicts with span citations AFTER baseline.
    pass(&store, &e_a, "see src/lib.rs:1 for proof");
    pass(&store, &e_b, "see src/lib.rs:2-4 for proof");

    // Edit the function body (line 3 changes); line 1 is untouched.
    tmp.write(
        "src/lib.rs",
        "// stable header\npub fn alpha() {\n    let _ = 99;\n}\n",
    );

    sync::run(&store, tmp.path()).unwrap();

    // Both are staled (no locator → Scope::Unknown always stales).
    assert_eq!(
        edge_status(&store, &e_a),
        InspectionStatus::NeedsReverification,
        "no-locator grounding must be staled even when its cited span is intact"
    );
    assert_eq!(
        edge_status(&store, &e_b),
        InspectionStatus::NeedsReverification,
    );

    // The difference is in the cause refinement only.
    let cause_a = stale_cause(&store, &e_a).expect("e_a must have a stale_cause");
    assert!(
        cause_a.contains("cited evidence intact"),
        "intact span must append 'cited evidence intact' to the cause: {cause_a}"
    );

    let cause_b = stale_cause(&store, &e_b).expect("e_b must have a stale_cause");
    assert!(
        cause_b.contains("cited evidence rewritten"),
        "rewritten span must append 'cited evidence rewritten' to the cause: {cause_b}"
    );
}

// ============================================================
// 7. unchanged_symbol_rewritten_evidence
// ============================================================

/// When the locator symbol is unchanged but the cited evidence spans in the same
/// file were rewritten, the grounding is staled — NOT spared — with a cause that
/// contains "unchanged" and names the locator symbol.
#[test]
fn unchanged_symbol_rewritten_evidence() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();

    // alpha on lines 1-3, beta on lines 4-6.
    tmp.write(
        "src/lib.rs",
        "pub fn alpha() {\n    let _ = 1;\n}\npub fn beta() {\n    let _ = 2;\n}\n",
    );

    let cf = add_codefile(&store, "src/lib.rs");
    let i = add_intent(&store, "alpha behavior", "something");
    // Locator resolves to alpha — alpha body is UNCHANGED in the edit below.
    let e = ground(&store, &i, &cf, Some("fn alpha"));

    sync::run(&store, tmp.path()).unwrap();

    // Evidence cites beta's body (lines 4-6), which WILL be rewritten.
    pass(&store, &e, "see src/lib.rs:4-6 for proof");

    // Edit only beta's body; alpha is untouched.
    tmp.write(
        "src/lib.rs",
        "pub fn alpha() {\n    let _ = 1;\n}\npub fn beta() {\n    let _ = 99;\n}\n",
    );

    sync::run(&store, tmp.path()).unwrap();

    assert_eq!(
        edge_status(&store, &e),
        InspectionStatus::NeedsReverification,
        "grounding must be staled when cited evidence is rewritten even if locator symbol is unchanged"
    );
    let cause = stale_cause(&store, &e).expect("stale_cause must be set");
    assert!(
        cause.contains("alpha"),
        "cause must name the unchanged locator symbol: {cause}"
    );
    assert!(
        cause.contains("unchanged"),
        "cause must note the locator symbol was unchanged: {cause}"
    );
}

// ============================================================
// 8. vague_intent_smell
// ============================================================

/// `vague_intent` fires when a description hedges without an observable outcome.
/// It does NOT fire when digits, a "by <gerund>" clause, or an outcome-verb stem
/// defuse the hedge. After `sync`, the Finding is materialized; a "justified"
/// adjudication makes `smell_has_resolving_adjudication` return true.
#[test]
fn vague_intent_smell() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();

    // Fires: "handles" hedge + "correctly" hedge; nothing observable in the description.
    let i_vague = store
        .add_node(
            NodeType::Intent,
            "handle auth errors",
            "handles errors correctly",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();

    // Does NOT fire: digit "429" defuses the hedge immediately.
    store
        .add_node(
            NodeType::Intent,
            "handle rate limits",
            "handles 429 responses by retrying with exponential backoff, max 5 attempts",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();

    // Does NOT fire: no hedge term; "returns" is an outcome stem.
    store
        .add_node(
            NodeType::Intent,
            "reject expired token",
            "returns an error when the token is expired",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();

    // Does NOT fire: "by sending" (by + gerund) defuses the hedge.
    store
        .add_node(
            NodeType::Intent,
            "reset password",
            "handles password reset by sending a recovery email",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();

    // Only the first intent produces a vague_intent smell.
    let vague_smells: Vec<_> = smells(&store)
        .unwrap()
        .into_iter()
        .filter(|s| s.kind == "vague_intent")
        .collect();
    assert_eq!(
        vague_smells.len(),
        1,
        "exactly one vague_intent smell expected"
    );
    let identity = vague_smells[0].identity.clone();
    assert!(
        identity.contains(&i_vague.id),
        "smell identity must reference the vague intent id: {identity}"
    );

    // Sync materializes the Finding node under its deterministic id.
    sync::run(&store, tmp.path()).unwrap();

    let finding_id = Store::derived_node_id(NodeType::Finding, &smell_det_key(&identity));
    assert!(
        store.get_node(&finding_id).unwrap().is_some(),
        "sync must materialize a Finding node for the vague_intent smell"
    );

    // Before adjudication: not justified.
    assert!(
        !smell_has_resolving_adjudication(&store, &identity).unwrap(),
        "smell must not have a resolving adjudication before any verdict"
    );

    // Adjudicate as justified.
    store
        .record_finding_verdict(
            &finding_id,
            "justified",
            "accepted as a summary-level intent",
        )
        .unwrap();

    // After adjudication: smell_has_resolving_adjudication reads the durable asserted facet.
    assert!(
        smell_has_resolving_adjudication(&store, &identity).unwrap(),
        "smell_has_resolving_adjudication must return true after a 'justified' adjudication"
    );
}
