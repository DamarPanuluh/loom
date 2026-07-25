//! Ring 11 — grounding-roles contracts and audit-fix regressions.
//!
//! Real SQLite, no mocks. Each test defends one observable contract:
//! coverage ownership by role, set-role re-open semantics, consumes
//! seam-drift staleness (the load-bearing sync acceptance), export/import
//! role round-trip + doctor green, the consumer_owned_file smell, rehome
//! supersede-not-delete, consumes_without_seam doctor issue, and the
//! listed H-*/M-* regressions. CLI is exercised only for the coverage
//! `--json` surface (the `pub(crate)` summary helpers have no other
//! externally observable proxy).

use loom::lane::Lane;
use loom::model::{EdgeKind, GroundingRole, InspectionStatus, NodeType, TargetKind, TruthClass};
use loom::registry::OwnerRole;
use loom::store::{Agent, Store};
use loom::travel::Export;
mod common;
use common::*;

fn seed_intent(store: &Store, name: &str) -> String {
    store
        .add_node(
            NodeType::Intent,
            name,
            "a behavior",
            "planned",
            serde_json::json!({}),
        )
        .unwrap()
        .id
}

/// Register a codefile AND put it on disk.
///
/// A registered path with no file behind it is a fiction: any `file:line`
/// citation into it is silently skipped, so a verdict "citing" it was never
/// anchored to anything. Once the grounding floor demands `cited`, these
/// fixtures stop compiling green — correctly, because they never proved what
/// they claimed to.
fn seed_codefile(store: &Store, path: &str) -> String {
    let full = store.root().join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    // Only create what is missing: a fixture that deliberately wrote content
    // here is testing that content, and a helper must never clobber it.
    if !full.exists() {
        let body: String = std::iter::once("pub fn behavior() {}\n".to_string())
            .chain((2..=40).map(|n| format!("// line {n}\n")))
            .collect();
        std::fs::write(&full, body).unwrap();
    }
    codefile(store, path)
        .id
}

/// The evidence a grounding fixture cites: a real span in the file it grounds.
fn cite(path: &str) -> String {
    format!("{path}:1")
}

/// A citation that resolves, for fixtures whose subject is not the evidence.
///
/// The floor demands a grounding POINT at code. These tests are about roles,
/// rehoming and staleness, not about what makes a citation good — so they get a
/// real file and a real span, rather than a placeholder that only looked like
/// one.
fn anchored(store: &Store) -> String {
    let p = store.root().join("fixture.rs");
    if !p.exists() {
        std::fs::write(&p, "pub fn fixture() {}\n").unwrap();
    }
    "fixture.rs:1".to_string()
}

/// Write `content` to `root/<rel>` creating parent dirs.
fn write_file(root: &std::path::Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, content).unwrap();
}

fn facet_value(store: &Store, edge_id: &str, key: &str) -> Option<String> {
    store.get_facet(edge_id, TargetKind::Edge, key).unwrap()
}

// =========================================================================
// 1. Coverage ownership: realizes owns, consumes does not.
// =========================================================================
#[test]
fn realizes_owns_consumes_does_not() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = seed_intent(&store, "render page");
    let backend = seed_codefile(&store, "src/backend.rs");
    let page = seed_codefile(&store, "routes/page.svelte");

    // backend grounded by a realizes edge; page grounded only by consumes.
    let real_edge = store
        .add_edge(
            EdgeKind::Implements,
            &intent,
            &backend,
            TruthClass::Asserted,
        )
        .unwrap();
    let cons_edge = store
        .add_edge(EdgeKind::Implements, &intent, &page, TruthClass::Asserted)
        .unwrap();
    store
        .set_grounding_role(&cons_edge.id, GroundingRole::Consumes)
        .unwrap();

    // realizes confers ownership: backend has a live realizing implementer.
    assert_eq!(
        store.realizing_implementers(&backend).unwrap().len(),
        1,
        "a realizes grounding owns the codefile"
    );
    // consumes NEVER owns: page has no realizing implementer -> coverage-unowned.
    assert!(
        store.realizing_implementers(&page).unwrap().is_empty(),
        "a consumes-only grounding leaves the file unowned"
    );
    // the realizing edge itself is the one that owns.
    assert_eq!(
        store.realizing_implementers(&backend).unwrap()[0].id,
        real_edge.id
    );
}

/// The same ownership contract observed end-to-end through `loom coverage --json`,
/// the only externally observable proxy for the `pub(crate)` summary helpers.
#[test]
fn coverage_json_reports_consumes_file_as_unowned() {
    let tmp = Tmp::new();
    // Seed via the binary so the graph is consistent with CLI assumptions.
    loom_init(tmp.path(), Some("t"));
    // codefile add requires the file to exist on disk.
    write_file(tmp.path(), "src/backend.rs", "fn ship() {}");
    write_file(
        tmp.path(),
        "routes/page.svelte",
        "<script>loadProfile();</script>",
    );
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "render page",
            "--description",
            "d",
        ],
    );
    loom_ok(tmp.path(), &["codefile", "add", "src/backend.rs"]);
    loom_ok(tmp.path(), &["codefile", "add", "routes/page.svelte"]);
    // realizing grounding owns backend.
    loom_ok(
        tmp.path(),
        &[
            "edge",
            "implement",
            "render page",
            "src/backend.rs",
            "--role",
            "realizes",
        ],
    );
    // consumes grounding does NOT own the page.
    loom_ok(
        tmp.path(),
        &[
            "edge",
            "implement",
            "render page",
            "routes/page.svelte",
            "--role",
            "consumes",
            "--locator",
            "loadProfile",
        ],
    );

    let cov = loom_json(tmp.path(), &["coverage"]);
    let unowned = cov["codefiles"]["unowned_files"].as_array().unwrap();
    assert!(
        unowned
            .iter()
            .any(|v| v.as_str() == Some("routes/page.svelte")),
        "consumes-only file must appear in unowned_files: {cov}"
    );
    assert_eq!(
        cov["codefiles"]["owned"].as_i64().unwrap(),
        1,
        "exactly one owned file (the realizes grounding): {cov}"
    );
    assert!(
        !unowned.iter().any(|v| v.as_str() == Some("src/backend.rs")),
        "realizes-owned file must NOT appear in unowned_files: {cov}"
    );
}

// =========================================================================
// 2. set-role re-open: changed role re-opens a settled claim; same-role does not.
// =========================================================================
#[test]
fn reclassify_changed_role_reopens_settled_claim() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = seed_intent(&store, "ship order");
    let file = seed_codefile(&store, "src/order.rs");
    let edge = store
        .add_edge(EdgeKind::Implements, &intent, &file, TruthClass::Asserted)
        .unwrap();
    // settle the claim with a passing verdict (criterion+evidence preserved).
    store
        .record_verdict(
            &edge.id,
            InspectionStatus::Passing,
            "criterion: routes order",
            "src/order.rs:12",
            0.9,
            "llm",
        )
        .unwrap();
    assert_eq!(
        store.get_edge(&edge.id).unwrap().unwrap().status,
        InspectionStatus::Passing
    );

    // reclassify realizes -> consumes: role CHANGES, claim re-opens.
    let (new_edge, old_role, reopened) = store
        .reclassify_grounding(
            &edge.id,
            GroundingRole::Consumes,
            "page only calls the route",
        )
        .unwrap();
    assert_eq!(old_role, GroundingRole::Realizes);
    assert!(reopened, "a role change on a settled claim must re-open");
    assert_eq!(new_edge.status, InspectionStatus::NeedsReverification);
    // stale_cause leads with role_changed.
    let cause = facet_value(&store, &edge.id, "stale_cause").unwrap();
    assert!(
        cause.starts_with("role_changed"),
        "stale_cause must start with role_changed, got {cause:?}"
    );
    // ORIGINAL criterion/evidence preserved (history kept, not wiped).
    let after = store.get_edge(&edge.id).unwrap().unwrap();
    assert_eq!(after.criterion, "criterion: routes order");
    assert_eq!(store.verdict_prose(&after.id).unwrap(), "src/order.rs:12");

    // same-role reclassify on a SETTLED claim does NOT re-open: use a fresh
    // consumes edge that is still passing (not the one just re-opened above,
    // which is now needs_reverification and would trivially return false).
    let file2 = seed_codefile(&store, "src/other.rs");
    let edge2 = store
        .add_edge(EdgeKind::Implements, &intent, &file2, TruthClass::Asserted)
        .unwrap();
    store
        .set_grounding_role(&edge2.id, GroundingRole::Consumes)
        .unwrap();
    store
        .record_verdict(
            &edge2.id,
            InspectionStatus::Passing,
            "c",
            &anchored(&store),
            0.9,
            "llm",
        )
        .unwrap();
    let (e2_after, _, reopened2) = store
        .reclassify_grounding(&edge2.id, GroundingRole::Consumes, "no-op rename")
        .unwrap();
    assert!(
        !reopened2,
        "a same-role reclassify on a settled claim must not re-open"
    );
    assert_eq!(
        e2_after.status,
        InspectionStatus::Passing,
        "settled claim stays settled on same-role reclassify"
    );
}

// =========================================================================
// 3. consumes seam-drift staleness (the load-bearing sync acceptance).
// =========================================================================
#[test]
fn consumes_seam_drift_stales_only_when_seam_gone() {
    let tmp = Tmp::new();
    let root = tmp.path().to_path_buf();
    let store = Store::init(&root, Some("t"), false).unwrap();
    let intent = seed_intent(&store, "render page");

    // backend file is realized; page file is consumed (seam = "loadProfile").
    let backend = seed_codefile(&store, "src/backend.rs");
    let page = seed_codefile(&store, "routes/page.svelte");
    write_file(&root, "src/backend.rs", "fn ship() {}");
    write_file(
        &root,
        "routes/page.svelte",
        "<script>loadProfile();</script>",
    );

    let real_edge = store
        .add_edge(
            EdgeKind::Implements,
            &intent,
            &backend,
            TruthClass::Asserted,
        )
        .unwrap();
    let cons_edge = store
        .add_edge(EdgeKind::Implements, &intent, &page, TruthClass::Asserted)
        .unwrap();
    store
        .set_grounding_role(&cons_edge.id, GroundingRole::Consumes)
        .unwrap();
    store
        .set_facet(
            &cons_edge.id,
            TargetKind::Edge,
            "locator",
            "loadProfile",
            TruthClass::Asserted,
        )
        .unwrap();

    // verdict both edges passing.
    store
        .record_verdict(
            &real_edge.id,
            InspectionStatus::Passing,
            "c",
            &anchored(&store),
            0.9,
            "llm",
        )
        .unwrap();
    store
        .record_verdict(
            &cons_edge.id,
            InspectionStatus::Passing,
            "calls route",
            &anchored(&store),
            0.9,
            "llm",
        )
        .unwrap();

    // first sync: seeds content hashes (no prior hash -> no ripple).
    loom::sync::run(&store, &root).unwrap();
    assert_eq!(
        store.get_edge(&real_edge.id).unwrap().unwrap().status,
        InspectionStatus::Passing,
        "realizing edge still passing after seed sync"
    );
    assert_eq!(
        store.get_edge(&cons_edge.id).unwrap().unwrap().status,
        InspectionStatus::Passing,
        "consumes edge still passing after seed sync"
    );

    // (a) edit page KEEPING the seam string -> consumes STAYS passing, realizes stays passing.
    write_file(
        &root,
        "routes/page.svelte",
        "<script>loadProfile();\nconsole.log('v2');</script>",
    );
    loom::sync::run(&store, &root).unwrap();
    assert_eq!(
        store.get_edge(&real_edge.id).unwrap().unwrap().status,
        InspectionStatus::Passing,
        "realizing edge stays passing when only the page changed (no ripple to backend)"
    );
    assert_eq!(
        store.get_edge(&cons_edge.id).unwrap().unwrap().status,
        InspectionStatus::Passing,
        "consumes edge stays passing when the seam string is preserved"
    );

    // (b) edit page REMOVING the seam string -> consumes -> needs_reverification, realizes stays passing.
    write_file(
        &root,
        "routes/page.svelte",
        "<script>renderOther();</script>",
    );
    loom::sync::run(&store, &root).unwrap();
    assert_eq!(
        store.get_edge(&real_edge.id).unwrap().unwrap().status,
        InspectionStatus::Passing,
        "realizing edge stays passing: the seam drift does not ripple to the consumed intent"
    );
    assert_eq!(
        store.get_edge(&cons_edge.id).unwrap().unwrap().status,
        InspectionStatus::NeedsReverification,
        "consumes edge re-opens when the seam locator is gone from the file"
    );
    let cause = facet_value(&store, &cons_edge.id, "stale_cause").unwrap();
    assert!(
        cause.starts_with("seam drift"),
        "consumes stale_cause must lead with seam drift, got {cause:?}"
    );
}

// =========================================================================
// 4. export->import round-trips roles + doctor green.
// =========================================================================
#[test]
fn export_import_roundtrips_role_and_doctor_green() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = seed_intent(&store, "render page");
    let page = seed_codefile(&store, "routes/page.svelte");
    let edge = store
        .add_edge(EdgeKind::Implements, &intent, &page, TruthClass::Asserted)
        .unwrap();
    store
        .set_grounding_role(&edge.id, GroundingRole::Consumes)
        .unwrap();
    // settle with a seam-naming criterion so doctor does not flag it.
    store
        .record_verdict(
            &edge.id,
            InspectionStatus::Passing,
            "calls /api/load",
            &anchored(&store),
            0.9,
            "llm",
        )
        .unwrap();

    let json = Export::from_snapshot(store.snapshot().unwrap())
        .to_json()
        .unwrap();

    // restore into a fresh store.
    let tmp2 = Tmp::new();
    let mut store2 = Store::init(tmp2.path(), None, false).unwrap();
    let export = Export::from_json(&json).unwrap();
    store2.restore(&export.into_snapshot()).unwrap();

    // the role facet survived: coverage still treats the file as unowned.
    let restored_edge = store2
        .edges_with(Some(EdgeKind::Implements), Some(&intent), Some(&page))
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(
        store2.grounding_role(&restored_edge.id).unwrap(),
        GroundingRole::Consumes,
        "role facet must survive export/import"
    );
    assert!(
        store2.realizing_implementers(&page).unwrap().is_empty(),
        "consumes-only file stays coverage-unowned after round-trip"
    );
    // doctor returns no issues on the restored graph.
    let issues = loom::signal::doctor(&store2).unwrap();
    assert!(
        issues.is_empty(),
        "doctor must be green on the restored graph: {issues:?}"
    );
}

// =========================================================================
// 5. consumer_owned_file smell.
// =========================================================================
#[test]
fn consumer_owned_file_smell_flags_cross_cluster_realizing() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = seed_intent(&store, "render page");
    let backend = seed_codefile(&store, "src/backend.rs");
    let page = seed_codefile(&store, "routes/page.svelte");
    // both realized by the same intent (different top-level clusters).
    store
        .add_edge(
            EdgeKind::Implements,
            &intent,
            &backend,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .add_edge(EdgeKind::Implements, &intent, &page, TruthClass::Asserted)
        .unwrap();

    let smells = loom::signal::smells(&store).unwrap();
    let matching: Vec<_> = smells
        .iter()
        .filter(|s| s.kind == "consumer_owned_file" && s.message.contains("routes/page.svelte"))
        .collect();
    assert!(
        !matching.is_empty(),
        "a page in a different cluster than the intent's other realizing files must smell consumer_owned_file: {smells:?}"
    );
}

// =========================================================================
// 6. rehome supersede-not-delete.
// =========================================================================
#[test]
fn rehome_supersedes_old_and_creates_new_carrying_role() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let old_intent = seed_intent(&store, "old owner");
    let new_intent = seed_intent(&store, "new owner");
    let file = seed_codefile(&store, "src/widget.rs");
    let edge = store
        .add_edge(
            EdgeKind::Implements,
            &old_intent,
            &file,
            TruthClass::Asserted,
        )
        .unwrap();
    // Realizes (default) — so the file is owned by the old intent before rehome.
    assert_eq!(
        store.grounding_role(&edge.id).unwrap(),
        GroundingRole::Realizes
    );
    assert_eq!(
        store.realizing_implementers(&file).unwrap().len(),
        1,
        "before rehome the file is owned by the old intent"
    );
    // settle it so we can observe the re-open on the successor.
    store
        .record_verdict(
            &edge.id,
            InspectionStatus::Passing,
            "c",
            &cite("src/widget.rs"),
            0.9,
            "llm",
        )
        .unwrap();

    let (old, new) = store
        .rehome_grounding(&edge.id, &new_intent, "wrong intent")
        .unwrap();

    // old edge superseded: excluded from realizing_implementers.
    assert!(
        store.edge_superseded(&old.id).unwrap(),
        "old edge must carry a superseded_by facet"
    );
    let live = store.realizing_implementers(&file).unwrap();
    assert!(
        !live.iter().any(|e| e.id == old.id),
        "the superseded realizing edge must be excluded from realizing_implementers"
    );
    // new edge exists on the successor, carries the old Realizes role, so it
    // is now the file's realizing implementer (ownership moved with the claim).
    assert_eq!(new.from_id, new_intent);
    assert_eq!(new.to_id, file);
    assert_eq!(
        store.grounding_role(&new.id).unwrap(),
        GroundingRole::Realizes,
        "the new edge carries the old role"
    );
    assert_eq!(
        live.iter().find(|e| e.id == new.id).map(|e| e.id.clone()),
        Some(new.id.clone()),
        "the new realizing edge is the file's live owner"
    );
    let cause = facet_value(&store, &new.id, "stale_cause").unwrap();
    assert!(
        cause.starts_with("rehomed"),
        "new edge stale_cause must start with rehomed, got {cause:?}"
    );
    // the successor edge is freshly created (uninspected); rehome stamps the
    // rehomed stale_cause on it regardless, and carries the old role/ownership.
    assert_eq!(new.status, InspectionStatus::Uninspected);
}

// =========================================================================
// 7. doctor consumes_without_seam.
// =========================================================================
#[test]
fn doctor_flags_consumes_without_seam_and_clears_with_route() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();

    // (a) vague criterion, no locator -> consumes_without_seam.
    let intent_a = seed_intent(&store, "vague");
    let file_a = seed_codefile(&store, "src/a.rs");
    let e_a = store
        .add_edge(
            EdgeKind::Implements,
            &intent_a,
            &file_a,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_grounding_role(&e_a.id, GroundingRole::Consumes)
        .unwrap();
    store
        .record_verdict(
            &e_a.id,
            InspectionStatus::Passing,
            "works fine",
            &anchored(&store),
            0.9,
            "llm",
        )
        .unwrap();

    // (b) criterion names a route (contains '/') -> no consumes_without_seam.
    let intent_b = seed_intent(&store, "routed");
    let file_b = seed_codefile(&store, "src/b.rs");
    let e_b = store
        .add_edge(
            EdgeKind::Implements,
            &intent_b,
            &file_b,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_grounding_role(&e_b.id, GroundingRole::Consumes)
        .unwrap();
    store
        .record_verdict(
            &e_b.id,
            InspectionStatus::Passing,
            "calls /api/widgets",
            &anchored(&store),
            0.9,
            "llm",
        )
        .unwrap();

    let issues = loom::signal::doctor(&store).unwrap();
    let seam_issues: Vec<_> = issues
        .iter()
        .filter(|i| i.kind == "consumes_without_seam")
        .collect();
    assert_eq!(
        seam_issues.len(),
        1,
        "exactly one consumes_without_seam issue"
    );
    assert!(
        seam_issues[0].message.contains(&e_a.id),
        "the flagged edge must be the vague one"
    );
    assert!(
        !seam_issues.iter().any(|i| i.message.contains(&e_b.id)),
        "the route-naming criterion must NOT be flagged"
    );
}

// =========================================================================
// Regressions
// =========================================================================

// 8. H-1: redefine_intent re-opens a settled realizing grounding.
#[test]
fn h1_redefine_intent_reopens_realizing_grounding() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = seed_intent(&store, "ship order");
    let file = seed_codefile(&store, "src/order.rs");
    let edge = store
        .add_edge(EdgeKind::Implements, &intent, &file, TruthClass::Asserted)
        .unwrap();
    store
        .record_verdict(
            &edge.id,
            InspectionStatus::Passing,
            "c",
            &cite("src/order.rs"),
            0.9,
            "llm",
        )
        .unwrap();
    assert_eq!(
        store.get_edge(&edge.id).unwrap().unwrap().status,
        InspectionStatus::Passing
    );

    store.redefine_intent(&intent, "ship order v2").unwrap();
    assert_eq!(
        store.get_edge(&edge.id).unwrap().unwrap().status,
        InspectionStatus::NeedsReverification,
        "redefining an intent must re-open its settled realizing grounding (H-1)"
    );
}

// 9. H-2: independent verdict requires BOTH non-empty criterion and evidence.
#[test]
fn h2_independent_verdict_requires_criterion_and_evidence() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = seed_intent(&store, "x");
    let file = seed_codefile(&store, "src/x.rs");
    let edge = store
        .add_edge(EdgeKind::Implements, &intent, &file, TruthClass::Asserted)
        .unwrap();

    // empty criterion, non-empty evidence -> error.
    assert!(
        store
            .record_verdict(
                &edge.id,
                InspectionStatus::Independent,
                "",
                &anchored(&store),
                0.9,
                "llm"
            )
            .is_err(),
        "independent verdict with empty criterion must error (H-2)"
    );
    // both non-empty -> ok.
    store
        .record_verdict(
            &edge.id,
            InspectionStatus::Independent,
            "c",
            &anchored(&store),
            0.9,
            "llm",
        )
        .unwrap();
    assert_eq!(
        store.get_edge(&edge.id).unwrap().unwrap().status,
        InspectionStatus::Independent
    );
}

// 10. H-4: Agent::parse fail-closed on unknown lane.
#[test]
fn h4_agent_parse_fail_closed_on_unknown_lane() {
    assert!(
        Agent::parse("llm:qualtiy").is_err(),
        "typo lane must be Err (H-4)"
    );
    assert!(
        Agent::parse("nonsense").is_err(),
        "bare nonsense must be Err"
    );
    assert_eq!(Agent::parse("llm").unwrap(), Agent::Solo);
    assert_eq!(Agent::parse("").unwrap(), Agent::Solo);
    assert_eq!(
        Agent::parse("llm:builder").unwrap(),
        Agent::Lane(OwnerRole::Builder)
    );
}

// 11. M-12: add_edge rejects Derived truth class.
#[test]
fn m12_add_edge_rejects_derived_truth_class() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let surface = store
        .add_node(
            NodeType::InterfaceSurface,
            "GET /x",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap()
        .id;
    let file = seed_codefile(&store, "src/a.rs");
    // exposes is asserted-only; a Derived add_edge must error.
    assert!(
        store
            .add_edge(EdgeKind::Exposes, &surface, &file, TruthClass::Derived)
            .is_err(),
        "add_edge with Derived must error (M-12); use add_derived_edge"
    );
    // Isolate the add_edge boundary from the registry truth-class check: use
    // a kind that DOES allow derived (Flags: Finding->CodeFile) and assert the
    // same add_edge path still refuses Derived — the error is the boundary, not
    // the kind's truth-class list.
    let finding = store
        .add_derived_node(
            NodeType::Finding,
            "finding:1",
            "finding:1",
            "todo found",
            "open",
            serde_json::json!({}),
        )
        .unwrap()
        .id;
    let err = store
        .add_edge(EdgeKind::Flags, &finding, &file, TruthClass::Derived)
        .expect_err("add_edge must refuse Derived even for a derived-allowing kind (M-12)");
    assert!(
        err.to_string().contains("add_edge is for asserted edges"),
        "the error must be the boundary guard, not a truth-class mismatch: {err}"
    );
}

// 12. M-13: set_derived_status rejects anything but Current.
#[test]
fn m13_set_derived_status_only_allows_current() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let codefile = seed_codefile(&store, "src/a.rs");
    // create a derived Flags edge (Finding -> CodeFile) via the derived path.
    let finding = store
        .add_derived_node(
            NodeType::Finding,
            "f1",
            "finding:1",
            "todo found",
            "open",
            serde_json::json!({}),
        )
        .unwrap()
        .id;
    let derived = store
        .add_derived_edge(EdgeKind::Flags, &finding, &codefile)
        .unwrap();

    // Passing is NOT Current -> error.
    assert!(
        store
            .set_derived_status(&derived.id, InspectionStatus::Passing)
            .is_err(),
        "set_derived_status with Passing must error (M-13)"
    );
    // Current is the only allowed derived status -> ok.
    store
        .set_derived_status(&derived.id, InspectionStatus::Current)
        .unwrap();
    assert_eq!(
        store.get_edge(&derived.id).unwrap().unwrap().status,
        InspectionStatus::Current
    );
}

// 13. H-12: observed graph zeroes build/coverage/fix/elaborate queue counts.
#[test]
fn h12_observed_graph_zeroes_active_lane_queues() {
    // Identical content seeded into both an observed and a non-observed graph,
    // populating all four disabled lanes (build/coverage/fix/elaborate) so the
    // observed zero is a real suppression, not a vacuous empty queue.
    fn build(root: &std::path::Path, observed: bool) -> Store {
        let store = Store::init(root, Some("t"), observed).unwrap();
        // build queue: a planned intent.
        store
            .add_node(
                NodeType::Intent,
                "planned feature",
                "d",
                "planned",
                serde_json::json!({}),
            )
            .unwrap();
        // an implemented intent that owns a file (so coverage/fix are real).
        let intent = store
            .add_node(
                NodeType::Intent,
                "ship",
                "d",
                "implemented",
                serde_json::json!({}),
            )
            .unwrap()
            .id;
        // coverage queue: an unowned codefile (no realizing implementer).
        seed_codefile(&store, "src/alone.rs");
        // fix queue: a failing grounding.
        let file = seed_codefile(&store, "src/owned.rs");
        let edge = store
            .add_edge(EdgeKind::Implements, &intent, &file, TruthClass::Asserted)
            .unwrap();
        store
            .record_verdict(
                &edge.id,
                InspectionStatus::Failing,
                "c",
                &anchored(&store),
                0.9,
                "llm",
            )
            .unwrap();
        // elaborate queue: a user_visible feature intent with open completeness
        // axes (no scenarios/prerequisites/boundary recorded -> open > 0).
        let feat = store
            .add_node(
                NodeType::Intent,
                "elaborate me",
                "d",
                "implemented",
                serde_json::json!({}),
            )
            .unwrap()
            .id;
        store
            .set_facet(
                &feat,
                TargetKind::Node,
                "level",
                "feature",
                TruthClass::Asserted,
            )
            .unwrap();
        store
            .set_facet(
                &feat,
                TargetKind::Node,
                "visibility",
                "user_visible",
                TruthClass::Asserted,
            )
            .unwrap();
        store
    }

    let tmp_obs = Tmp::new();
    let obs = build(tmp_obs.path(), true);
    let obs_q = loom::maturity::depths(&obs).unwrap();
    assert_eq!(obs_q.get(Lane::Build), 0, "observed: build queue zeroed");
    assert_eq!(
        obs_q.get(Lane::Coverage),
        0,
        "observed: coverage queue zeroed"
    );
    assert_eq!(obs_q.get(Lane::Fix), 0, "observed: fix queue zeroed");
    assert_eq!(
        obs_q.get(Lane::Elaborate),
        0,
        "observed: elaborate queue zeroed"
    );

    let tmp_act = Tmp::new();
    let act = build(tmp_act.path(), false);
    let act_q = loom::maturity::depths(&act).unwrap();
    assert!(
        act_q.get(Lane::Build) > 0,
        "non-observed: build queue populated (planned intent)"
    );
    assert!(
        act_q.get(Lane::Coverage) > 0,
        "non-observed: coverage queue populated (unowned file)"
    );
    assert!(
        act_q.get(Lane::Fix) > 0,
        "non-observed: fix queue populated (failing edge)"
    );
    assert!(
        act_q.get(Lane::Elaborate) > 0,
        "non-observed: elaborate queue populated (user_visible feature with open axes)"
    );
}

// 14. M-7: an export with an unsupported format version fails from_json.
#[test]
fn m7_unsupported_export_format_is_rejected() {
    // a minimal valid-shaped export with a future format version.
    let bad = r#"{"format":999,"graph_id":"g","name":"n","observed":false,
                  "nodes":[],"edges":[],"facets":[],"tags":[]}"#;
    assert!(
        Export::from_json(bad).is_err(),
        "an export with format 999 must be rejected (M-7)"
    );
    // sanity: the current format still parses. Format 1 is deliberately NOT
    // accepted — it predates the evidence spine, and silently importing its
    // verdicts would reintroduce exactly the unanchored facts the spine exists
    // to refuse. `loom carry-forward` is the explicit path for those.
    let ok = r#"{"format":2,"graph_id":"g","name":"n","observed":false,
                  "nodes":[],"edges":[],"facets":[],"tags":[]}"#;
    assert!(Export::from_json(ok).is_ok());
    let legacy = r#"{"format":1,"graph_id":"g","name":"n","observed":false,
                  "nodes":[],"edges":[],"facets":[],"tags":[]}"#;
    assert!(
        Export::from_json(legacy).is_err(),
        "a pre-evidence-spine export must not import silently"
    );
}

// =========================================================================
// 15. Per-codefile observed flag: exempt from ownership/coverage/build
// obligations, still in the sync/surface/contract plane.
// =========================================================================
#[test]
fn observed_codefile_exempt_from_coverage_but_still_in_contract_plane() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));
    write_file(tmp.path(), "src/app.rs", "fn mount() {}");
    write_file(
        tmp.path(),
        "vendor/tilde/module.rs",
        "pub trait Module { fn prepare(&self); }",
    );
    loom_ok(
        tmp.path(),
        &[
            "intent",
            "add",
            "--name",
            "engine mounts",
            "--description",
            "d",
        ],
    );
    loom_ok(tmp.path(), &["codefile", "add", "src/app.rs"]);
    loom_ok(
        tmp.path(),
        &["codefile", "add", "vendor/tilde/module.rs", "--observed"],
    );
    loom_ok(
        tmp.path(),
        &[
            "edge",
            "implement",
            "engine mounts",
            "src/app.rs",
            "--role",
            "realizes",
        ],
    );

    // Exempt: the observed file is not a coverage gap and feeds no queue.
    let cov = loom_json(tmp.path(), &["coverage"]);
    let unowned = cov["codefiles"]["unowned_files"].as_array().unwrap();
    assert!(
        !unowned
            .iter()
            .any(|v| v.as_str() == Some("vendor/tilde/module.rs")),
        "observed file must not be a coverage gap: {cov}"
    );
    assert_eq!(
        cov["codefiles"]["observed"].as_i64().unwrap(),
        1,
        "observed bucket counts the monitored file: {cov}"
    );
    let status = loom_json(tmp.path(), &["status"]);
    assert_eq!(
        status["queues"]["coverage"].as_i64().unwrap(),
        0,
        "coverage queue must not serve the observed file: {status}"
    );

    // Still in the sync plane: a surface backed by the observed file resets
    // its contract validation when the vendored content changes.
    loom_ok(tmp.path(), &["sync"]);
    loom_ok(
        tmp.path(),
        &[
            "surface",
            "add",
            "--name",
            "Module::prepare",
            "--kind",
            "sdk_method",
            "--codefile",
            "vendor/tilde/module.rs",
        ],
    );
    loom_ok(
        tmp.path(),
        &[
            "validation",
            "add",
            "--name",
            "boots through prepare",
            "--type",
            "contract",
            "--intent",
            "engine mounts",
        ],
    );
    loom_ok(
        tmp.path(),
        &["edge", "call", "boots through prepare", "Module::prepare"],
    );
    write_file(
        tmp.path(),
        "vendor/tilde/module.rs",
        "pub trait Module { fn prepare(&self, ctx: &Ctx); }",
    );
    // A contract proof loom cannot run is ATTESTED, not reported: the
    // attestation must point at something re-checkable, so the citation is what
    // makes it count. Prose alone leaves it visibly weaker than a run.
    loom_ok(
        tmp.path(),
        &[
            "validation",
            "verdict",
            "boots through prepare",
            "passed",
            "--evidence",
            "booted against vendored tilde: vendor/tilde/module.rs:1",
        ],
    );
    loom_ok(tmp.path(), &["sync"]);
    let val = loom_json(tmp.path(), &["validation", "show", "boots through prepare"]);
    assert_eq!(
        val["status"].as_str().unwrap(),
        "not_run",
        "upstream change must reset the contract validation: {val}"
    );
}

/// Observed globs propagate: re-adding with --observed marks an existing file,
/// and `rescan` registers files that appear under an observed glob later as
/// observed too — never as coverage gaps.
#[test]
fn observed_glob_propagates_through_readd_and_rescan() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));
    write_file(tmp.path(), "vendor/tilde/a.rs", "fn a() {}");
    // Registered owned first, then upgraded by the observed glob re-add.
    loom_ok(tmp.path(), &["codefile", "add", "vendor/tilde/a.rs"]);
    loom_ok(
        tmp.path(),
        &["codefile", "add", "vendor/tilde/**/*.rs", "--observed"],
    );
    // Upstream adds a file later; rescan registers it as observed.
    write_file(tmp.path(), "vendor/tilde/b.rs", "fn b() {}");
    loom_ok(tmp.path(), &["codefile", "rescan"]);

    let cov = loom_json(tmp.path(), &["coverage"]);
    assert_eq!(
        cov["codefiles"]["observed"].as_i64().unwrap(),
        2,
        "both the upgraded and the rescanned file are observed: {cov}"
    );
    assert!(
        cov["codefiles"]["unowned_files"]
            .as_array()
            .unwrap()
            .is_empty(),
        "no observed file is a coverage gap: {cov}"
    );
}

// =========================================================================
// CLI helpers (coverage --json is the only pub(crate)-gated surface here).
// =========================================================================

fn loom_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_loom"))
}

fn loom_init(tmp: &std::path::Path, name: Option<&str>) {
    let mut cmd = std::process::Command::new(loom_bin());
    cmd.arg("init").arg(tmp);
    if let Some(n) = name {
        cmd.args(["--name", n]);
    }
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "loom init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn loom_ok(tmp: &std::path::Path, args: &[&str]) {
    let mut cmd = std::process::Command::new(loom_bin());
    cmd.arg("--graph").arg(tmp).args(args);
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("spawn loom {:?}: {e}", args));
    assert!(
        out.status.success(),
        "loom {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn loom_json(tmp: &std::path::Path, args: &[&str]) -> serde_json::Value {
    let mut cmd = std::process::Command::new(loom_bin());
    cmd.arg("--graph").arg(tmp).args(args).arg("--json");
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("spawn loom {:?}: {e}", args));
    assert!(
        out.status.success(),
        "loom {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "loom {:?} stdout not JSON: {e}\n{}",
            args,
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

// =========================================================================
// 16. Rescan (and sync) skip files matched by `loom ignore` globs.
// =========================================================================

/// A file matched by an ignore glob must not be registered by `codefile rescan`
/// even when it matches a remembered codefile glob.  Files already registered
/// before the ignore was added stay (ignore never deletes nodes).
#[test]
fn rescan_skips_ignored_glob_matches() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));
    write_file(tmp.path(), "vendor/up/a.rs", "fn a() {}");
    // Register a glob — captures `a.rs`.
    loom_ok(tmp.path(), &["codefile", "add", "vendor/up/**/*.rs"]);
    // Ignore a subset that will match `b.rs` but not `a.rs`.
    loom_ok(
        tmp.path(),
        &[
            "ignore",
            "add",
            "vendor/up/ignored/**/*.rs",
            "--reason",
            "third-party",
        ],
    );
    // A new file appears under the ignored subtree.
    write_file(tmp.path(), "vendor/up/ignored/b.rs", "fn b() {}");
    // And one outside it.
    write_file(tmp.path(), "vendor/up/c.rs", "fn c() {}");
    loom_ok(tmp.path(), &["codefile", "rescan"]);

    let store = Store::open(tmp.path()).unwrap();
    let files: Vec<String> = store
        .list_nodes(Some(NodeType::CodeFile), usize::MAX)
        .unwrap()
        .into_iter()
        .map(|n| n.name)
        .collect();
    assert!(
        files.contains(&"vendor/up/a.rs".to_string()),
        "pre-existing file stays: {files:?}"
    );
    assert!(
        files.contains(&"vendor/up/c.rs".to_string()),
        "non-ignored new file is registered: {files:?}"
    );
    assert!(
        !files.contains(&"vendor/up/ignored/b.rs".to_string()),
        "ignored file must NOT be registered by rescan: {files:?}"
    );
}

/// `codefile add '<glob>'` must skip paths matched by ignore globs for new
/// glob-discovered files, but explicit literal adds override the ignore.
#[test]
fn codefile_add_glob_skips_ignored_but_literal_overrides() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));
    write_file(tmp.path(), "gen/a.rs", "fn a() {}");
    write_file(tmp.path(), "gen/b.rs", "fn b() {}");
    loom_ok(
        tmp.path(),
        &["ignore", "add", "gen/b.rs", "--reason", "generated"],
    );
    // Glob add — b.rs should be skipped.
    let j = loom_json(tmp.path(), &["codefile", "add", "gen/**/*.rs"]);
    assert_eq!(j["registered"].as_i64().unwrap(), 1, "only a.rs: {j}");
    assert_eq!(j["ignored"].as_i64().unwrap(), 1, "b.rs ignored: {j}");
    // Explicit literal add — overrides the ignore.
    loom_ok(tmp.path(), &["codefile", "add", "gen/b.rs"]);
    let store = Store::open(tmp.path()).unwrap();
    let files: Vec<String> = store
        .list_nodes(Some(NodeType::CodeFile), usize::MAX)
        .unwrap()
        .into_iter()
        .map(|n| n.name)
        .collect();
    assert!(
        files.contains(&"gen/b.rs".to_string()),
        "literal add overrides ignore: {files:?}"
    );
}

// =========================================================================
// 17. Sync discovers new files under remembered globs and reports them.
// =========================================================================

/// `loom sync` must run the discovery pass (rescan remembered globs) before the
/// deriver loop.  New files appear in the store AND in the sync JSON output
/// (`new_files` list, `new_observed` count).
#[test]
fn sync_discovers_new_files_and_reports_them() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));
    write_file(tmp.path(), "src/alpha.rs", "fn alpha() {}");
    // Register an owned glob + an observed glob.
    loom_ok(tmp.path(), &["codefile", "add", "src/**/*.rs"]);
    loom_ok(
        tmp.path(),
        &["codefile", "add", "vendor/**/*.rs", "--observed"],
    );
    // Initial sync to baseline.
    loom_ok(tmp.path(), &["sync"]);
    // New files appear after the baseline sync.
    write_file(tmp.path(), "src/beta.rs", "fn beta() {}");
    write_file(tmp.path(), "vendor/lib.rs", "fn lib() {}");
    let j = loom_json(tmp.path(), &["sync"]);

    // The new_files list must contain the discovered paths.
    let new_files: Vec<String> = j["new_files"]
        .as_array()
        .expect("new_files is an array")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(
        new_files.contains(&"src/beta.rs".to_string()),
        "owned discovery: {new_files:?}"
    );
    assert!(
        new_files.contains(&"vendor/lib.rs".to_string()),
        "observed discovery: {new_files:?}"
    );
    assert_eq!(
        j["new_observed"].as_i64().unwrap(),
        1,
        "one observed file: {j}"
    );

    // The store must have registered both.
    let store = Store::open(tmp.path()).unwrap();
    let files: Vec<String> = store
        .list_nodes(Some(NodeType::CodeFile), usize::MAX)
        .unwrap()
        .into_iter()
        .map(|n| n.name)
        .collect();
    assert!(
        files.contains(&"src/beta.rs".to_string()),
        "beta.rs in store: {files:?}"
    );
    assert!(
        files.contains(&"vendor/lib.rs".to_string()),
        "vendor/lib.rs in store: {files:?}"
    );
}

/// Sync's discovery pass also respects ignore globs — an ignored file that
/// matches a remembered glob must not appear in the sync JSON `new_files`.
#[test]
fn sync_discovery_respects_ignore_globs() {
    let tmp = Tmp::new();
    loom_init(tmp.path(), Some("t"));
    write_file(tmp.path(), "src/keep.rs", "fn keep() {}");
    loom_ok(tmp.path(), &["codefile", "add", "src/**/*.rs"]);
    loom_ok(
        tmp.path(),
        &["ignore", "add", "src/gen/**/*.rs", "--reason", "generated"],
    );
    loom_ok(tmp.path(), &["sync"]);

    // New files: one kept, one ignored.
    write_file(tmp.path(), "src/new.rs", "fn new() {}");
    write_file(tmp.path(), "src/gen/auto.rs", "fn auto() {}");
    let j = loom_json(tmp.path(), &["sync"]);
    let new_files: Vec<String> = j["new_files"]
        .as_array()
        .expect("new_files is an array")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(
        new_files.contains(&"src/new.rs".to_string()),
        "non-ignored discovered: {new_files:?}"
    );
    assert!(
        !new_files.contains(&"src/gen/auto.rs".to_string()),
        "ignored file must NOT be discovered: {new_files:?}"
    );
}
