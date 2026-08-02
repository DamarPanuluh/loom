//! Ring 31 — ratified, live Pattern guidance and coding-packet delivery.

mod common;
use common::Tmp;
use loom::lane::Lane;
use loom::model::{EdgeKind, InspectionStatus, NodeType, TargetKind, TruthClass};
use loom::pattern::{Applicability, PatternBody};
use loom::registry::OwnerRole;
use loom::store::{Agent, Store};

fn body(paths: &[&str], tags: &[&str]) -> serde_json::Value {
    serde_json::to_value(PatternBody {
        rationale: "One error envelope keeps callers and logs comparable.".into(),
        when_to_use: "Use for repository HTTP handlers.".into(),
        when_not_to_use: "Do not use for internal batch jobs.".into(),
        applicability: Applicability {
            path_globs: paths.iter().map(|s| s.to_string()).collect(),
            intent_tags: tags.iter().map(|s| s.to_string()).collect(),
        },
    })
    .unwrap()
}

fn codefile(store: &Store, source: &str) -> loom::model::Node {
    if !store.root().join("src").is_dir() {
        std::fs::create_dir_all(store.root().join("src")).unwrap();
    }
    std::fs::write(store.root().join("src/example.rs"), source).unwrap();
    store
        .add_node(
            NodeType::CodeFile,
            "src/example.rs",
            "",
            "",
            serde_json::json!({}),
        )
        .unwrap()
}

fn pattern(store: &Store) -> loom::model::Node {
    store
        .add_node(
            NodeType::Pattern,
            "HTTP error envelope",
            "",
            "draft",
            body(&["src/**"], &["api"]),
        )
        .unwrap()
}

fn exemplar(
    store: &Store,
    pattern: &loom::model::Node,
    file: &loom::model::Node,
) -> loom::model::Edge {
    let edge = store
        .add_edge(
            EdgeKind::Exemplar,
            &pattern.id,
            &file.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &edge.id,
            TargetKind::Edge,
            "locator",
            "exemplar",
            TruthClass::Asserted,
        )
        .unwrap();
    edge
}

fn trust(store: &Store, pattern: &loom::model::Node, edge: &loom::model::Edge) {
    store
        .ratify_pattern(
            &pattern.id,
            "The maintainer chose this repository convention.",
            "mint",
        )
        .unwrap();
    store
        .record_verdict(
            &edge.id,
            InspectionStatus::Passing,
            "the located handler demonstrates the complete convention",
            "src/example.rs:1-3 — reviewed live handler",
            0.95,
            "analyzer",
        )
        .unwrap();
}

fn fixture() -> (
    Tmp,
    Store,
    loom::model::Node,
    loom::model::Node,
    loom::model::Edge,
) {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("patterns"), false).unwrap();
    let file = codefile(
        &store,
        "pub fn exemplar() {\n println!(\"actual source\");\n}\n\npub fn other() {}\n",
    );
    let pattern = pattern(&store);
    let edge = exemplar(&store, &pattern, &file);
    trust(&store, &pattern, &edge);
    (tmp, store, pattern, file, edge)
}

#[test]
fn body_authority_and_edge_types_fail_closed() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("patterns"), false).unwrap();
    let mut invalid = body(&[], &[]);
    invalid["snippet"] = serde_json::json!("copied code");
    assert!(store
        .add_node(NodeType::Pattern, "bad", "", "draft", invalid)
        .is_err());
    let pattern = pattern(&store);
    store.set_agent(Agent::Lane(OwnerRole::Builder));
    assert!(store
        .ratify_pattern(&pattern.id, "agent choice", "forged")
        .unwrap_err()
        .to_string()
        .contains("INV-8"));
    store.set_agent(Agent::Solo);
    let intent = store
        .add_node(
            NodeType::Intent,
            "handle requests",
            "requests return a response",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    assert!(store
        .add_edge(
            EdgeKind::Exemplar,
            &pattern.id,
            &intent.id,
            TruthClass::Asserted
        )
        .is_err());
}

#[test]
fn ambiguous_exemplar_locator_is_rejected() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("patterns"), false).unwrap();
    let file = codefile(
        &store,
        "mod first { fn duplicate() {} }\nmod second { fn duplicate() {} }\n",
    );
    let pattern = pattern(&store);
    let edge = store
        .add_edge(
            EdgeKind::Exemplar,
            &pattern.id,
            &file.id,
            TruthClass::Asserted,
        )
        .unwrap();
    assert!(store
        .set_facet(
            &edge.id,
            TargetKind::Edge,
            "locator",
            "duplicate",
            TruthClass::Asserted,
        )
        .unwrap_err()
        .to_string()
        .contains("exactly one"));
}

#[test]
fn trusted_guidance_contains_source_and_requires_path_and_tag() {
    let (_tmp, store, pattern, _file, _edge) = fixture();
    let view = loom::pattern::inspect(&store, &pattern).unwrap();
    assert_eq!(view.health, "routable");
    assert!(view.exemplars[0].source_excerpt.contains("actual source"));
    assert!(!view.exemplars[0].source_excerpt.contains("(1 match)"));
    assert_eq!(
        loom::pattern::guidance(&store, &["src/new.rs".into()], &["api".into()])
            .unwrap()
            .matched,
        1
    );
    assert_eq!(
        loom::pattern::guidance(&store, &["tests/new.rs".into()], &["api".into()])
            .unwrap()
            .matched,
        0
    );
    assert_eq!(
        loom::pattern::guidance(&store, &["src/new.rs".into()], &["worker".into()])
            .unwrap()
            .matched,
        0
    );
}

#[test]
fn unrelated_edit_is_spared_but_exemplar_edit_is_not() {
    let (_tmp, store, pattern, _file, _edge) = fixture();
    std::fs::write(store.root().join("src/example.rs"), "pub fn exemplar() {\n println!(\"actual source\");\n}\n\npub fn other() { println!(\"changed\"); }\n").unwrap();
    assert_eq!(
        loom::pattern::inspect(&store, &pattern).unwrap().health,
        "routable"
    );
    std::fs::write(
        store.root().join("src/example.rs"),
        "pub fn exemplar() {\n println!(\"changed convention\");\n}\n\npub fn other() {}\n",
    )
    .unwrap();
    assert_eq!(
        loom::pattern::inspect(&store, &pattern).unwrap().health,
        "stale"
    );
    assert_eq!(
        loom::pattern::guidance(&store, &["src/new.rs".into()], &["api".into()])
            .unwrap()
            .matched,
        0
    );
}

#[test]
fn normative_change_reopens_but_name_change_does_not() {
    let (_tmp, store, pattern, _file, edge) = fixture();
    store
        .update_node(&pattern.id, Some("Renamed convention"), None, None)
        .unwrap();
    assert_eq!(store.ratification(&pattern.id).unwrap(), "ratified");
    assert_eq!(
        store.get_edge(&edge.id).unwrap().unwrap().status,
        InspectionStatus::Passing
    );
    let mut changed = PatternBody::parse(&pattern.body).unwrap();
    changed.when_not_to_use = "Do not use for streams or jobs.".into();
    store
        .set_node_body(&pattern.id, &serde_json::to_value(changed).unwrap())
        .unwrap();
    assert_eq!(
        store.ratification(&pattern.id).unwrap(),
        "needs_reconfirmation"
    );
    assert_eq!(
        store.get_edge(&edge.id).unwrap().unwrap().status,
        InspectionStatus::NeedsReverification
    );
}

#[test]
fn failing_exemplar_is_analysis_not_code_repair() {
    let (_tmp, store, _pattern, _file, edge) = fixture();
    store
        .record_verdict(
            &edge.id,
            InspectionStatus::Failing,
            "this code does not demonstrate the full convention",
            "src/example.rs:1-3 — reviewed live handler",
            0.95,
            "analyzer",
        )
        .unwrap();

    let fix = loom::workitem::queue_items(&store, Lane::Fix).unwrap();
    assert!(fix.iter().all(|item| item.target.id != edge.id));
    let analyze = loom::workitem::queue_items(&store, Lane::Analyze).unwrap();
    assert!(analyze.iter().any(|item| item.target.id == edge.id));
    let depths = loom::maturity::depths(&store).unwrap();
    assert_eq!(depths.get(Lane::Fix), fix.len());
    assert_eq!(depths.get(Lane::Analyze), analyze.len());
}

#[test]
fn exemplar_packet_reads_source_and_build_packet_gets_guidance() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("patterns"), false).unwrap();
    let file = codefile(
        &store,
        "pub fn exemplar() {\n println!(\"house style\");\n}\n",
    );
    let pattern = pattern(&store);
    let edge = exemplar(&store, &pattern, &file);
    let analyze = loom::workitem::next(&store, Some(Lane::Analyze))
        .unwrap()
        .unwrap();
    assert_eq!(
        analyze.context.read_set[0].locator.as_deref(),
        Some("exemplar")
    );
    assert!(analyze
        .prompt_contract
        .write_back
        .contains("pattern exemplar verdict"));
    assert!(analyze.pattern_guidance.is_none());
    trust(&store, &pattern, &edge);
    let intent = store
        .add_node(
            NodeType::Intent,
            "create API response",
            "a request receives a stable response",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    store.set_tag(&intent.id, TargetKind::Node, "api").unwrap();
    let ground = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &file.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &ground.id,
            TargetKind::Edge,
            "locator",
            "exemplar",
            TruthClass::Asserted,
        )
        .unwrap();
    let build = loom::workitem::next(&store, Some(Lane::Build))
        .unwrap()
        .unwrap();
    let guidance = build.pattern_guidance.unwrap();
    assert_eq!((guidance.matched, guidance.included), (1, 1));
    assert!(guidance.items[0].source_excerpt.contains("house style"));
}

#[test]
fn fix_packet_receives_the_same_guidance_as_lookup() {
    let (_tmp, store, pattern, file, _exemplar) = fixture();
    let intent = store
        .add_node(
            NodeType::Intent,
            "repair API response",
            "an API response follows the contract",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    store.set_tag(&intent.id, TargetKind::Node, "api").unwrap();
    let grounding = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &file.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &grounding.id,
            TargetKind::Edge,
            "locator",
            "exemplar",
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &grounding.id,
            InspectionStatus::Failing,
            "the implementation does not meet the intent",
            "src/example.rs:1-3 — observed mismatch",
            0.95,
            "builder",
        )
        .unwrap();

    let fix = loom::workitem::next(&store, Some(Lane::Fix))
        .unwrap()
        .unwrap();
    let delivered = fix.pattern_guidance.expect("fix guidance");
    let looked_up =
        loom::pattern::guidance(&store, &["src/example.rs".into()], &["api".into()]).unwrap();
    assert_eq!(delivered.items, looked_up.items);
    assert_eq!(delivered.items[0].name, pattern.name);
}

#[test]
fn guidance_budget_reports_omitted_matches() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("patterns"), false).unwrap();
    let file = codefile(
        &store,
        "pub fn exemplar() {\n println!(\"bounded source\");\n}\n",
    );
    for index in 0..=loom::pattern::MAX_GUIDANCE_ITEMS {
        let pattern = store
            .add_node(
                NodeType::Pattern,
                &format!("pattern {index}"),
                "",
                "draft",
                body(&["src/**"], &["api"]),
            )
            .unwrap();
        let edge = exemplar(&store, &pattern, &file);
        trust(&store, &pattern, &edge);
    }
    let guidance =
        loom::pattern::guidance(&store, &["src/new.rs".into()], &["api".into()]).unwrap();
    assert_eq!(guidance.matched, loom::pattern::MAX_GUIDANCE_ITEMS + 1);
    assert_eq!(guidance.included, loom::pattern::MAX_GUIDANCE_ITEMS);
    assert_eq!(guidance.omitted, 1);
    assert!(guidance.lookup_command.contains("--offset 5"));
    let next = loom::pattern::guidance_page(
        &store,
        &["src/new.rs".into()],
        &["api".into()],
        guidance.included,
    )
    .unwrap();
    assert_eq!(next.included, 1);
    assert!(guidance
        .items
        .iter()
        .all(|first| next.items.iter().all(|second| first != second)));
}

#[test]
fn exemplar_does_not_own_or_prove_and_export_has_no_excerpt() {
    let (_tmp, store, _pattern, file, _edge) = fixture();
    assert!(store
        .edges_with(Some(EdgeKind::Implements), None, Some(&file.id))
        .unwrap()
        .is_empty());
    assert!(store
        .edges_with(Some(EdgeKind::Validates), None, None)
        .unwrap()
        .is_empty());
    let first = loom::travel::Export::from_snapshot(store.snapshot().unwrap())
        .to_json()
        .unwrap();
    let second = loom::travel::Export::from_json(&first)
        .unwrap()
        .to_json()
        .unwrap();
    assert_eq!(first, second);
    assert!(!first.contains("source_excerpt"));
    assert!(!first.contains("actual source"));
}
