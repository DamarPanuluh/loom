//! Ring 32 — governed host-performed internet research.

mod common;
use common::Tmp;
use loom::cli::{Cli, Command, TaskCmd};
use loom::lane::Lane;
use loom::model::{NodeType, TargetKind};
use loom::research::{ResearchBody, ResearchSource, SourceKind};
use loom::store::Store;

fn source(fresh_until: Option<&str>) -> ResearchSource {
    let quote = "This specification defines the overall architecture of HTTP and its terminology.";
    ResearchSource {
        url: "https://www.rfc-editor.org/rfc/rfc9110.html".into(),
        title: "HTTP Semantics".into(),
        publisher: "RFC Editor".into(),
        source_kind: SourceKind::Standard,
        retrieved_at: "2026-08-02T12:00:00Z".into(),
        quote: quote.into(),
        quote_fingerprint: loom::research::quote_fingerprint(quote),
        published_at: Some("2022-06-01T00:00:00Z".into()),
        fresh_until: fresh_until.map(str::to_string),
    }
}

fn research_body(sources: Vec<ResearchSource>) -> serde_json::Value {
    serde_json::to_value(ResearchBody {
        kind: "research".into(),
        research_schema: 1,
        why_external:
            "The applicable external standard can change independently of this repository.".into(),
        preferred_sources: vec!["RFC Editor standards".into()],
        sources,
        target_id: None,
        conclusion_fresh_until: None,
    })
    .unwrap()
}

#[test]
fn strict_body_source_and_close_validation() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    // Unmarked legacy tasks remain generic and keep their historical routing.
    assert!(store
        .add_node(
            NodeType::TaskRecord,
            "bad",
            "",
            "proposed",
            serde_json::json!({"kind":"research"})
        )
        .is_ok());
    let mut bad = source(None);
    bad.url = "https://www.google.com/search?q=http".into();
    assert!(bad
        .validate()
        .unwrap_err()
        .to_string()
        .contains("discovery only"));
    let empty = ResearchBody::parse(&research_body(vec![])).unwrap();
    assert!(empty.validate_close(chrono::Utc::now()).is_err());
    let mut stale_source = source(Some("2020-01-01T00:00:00Z"));
    stale_source.retrieved_at = "2019-01-01T00:00:00Z".into();
    stale_source.published_at = None;
    let stale = ResearchBody::parse(&research_body(vec![stale_source])).unwrap();
    assert!(stale
        .validate_close(chrono::Utc::now())
        .unwrap_err()
        .to_string()
        .contains("currently usable"));
}

#[test]
fn research_routes_in_analyze_and_depth_equals_roster() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let task = store
        .add_node(
            NodeType::TaskRecord,
            "Current HTTP requirement",
            "",
            "proposed",
            research_body(vec![]),
        )
        .unwrap();
    let roster = loom::workitem::queue_items(&store, Lane::Analyze).unwrap();
    assert!(roster.iter().any(|r| r.target.id == task.id));
    assert_eq!(
        loom::lane::LadderInputs::gather(&store)
            .unwrap()
            .open_research,
        1
    );
    assert_eq!(
        Lane::Analyze.depth(&loom::lane::LadderInputs::gather(&store).unwrap()),
        roster.len()
    );
    let item = loom::workitem::next(&store, Some(Lane::Analyze))
        .unwrap()
        .unwrap();
    let packet = serde_json::to_string(&item).unwrap();
    for required in [
        "host web search/browser",
        "actual pages",
        "Search snippets",
        "source-add",
        "edit code",
        "professional certification",
    ] {
        assert!(packet.contains(required), "missing {required}");
    }
}

#[test]
fn source_dedup_close_note_and_export_remain_advisory() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "HTTP behavior",
            "criterion",
            "planned",
            serde_json::json!({}),
        )
        .unwrap();
    drop(store);
    let run = |cmd| {
        loom::commands::run(Cli {
            graph: Some(tmp.path().to_path_buf()),
            json: true,
            command: Some(Command::Task { cmd }),
        })
        .unwrap()
    };
    run(TaskCmd::Add {
        title: "Current HTTP rule".into(),
        kind: "research".into(),
        target: Some(intent.id.clone()),
        why_external: Some(
            "The standard is maintained outside this repository and may change.".into(),
        ),
        preferred_sources: vec!["RFC Editor".into()],
    });
    let add = || TaskCmd::SourceAdd {
        task: "Current HTTP rule".into(),
        url: "https://www.rfc-editor.org/rfc/rfc9110.html".into(),
        title: "HTTP Semantics".into(),
        publisher: "RFC Editor".into(),
        source_kind: "standard".into(),
        quote: "This specification defines the overall architecture of HTTP and its terminology."
            .into(),
        published_at: None,
        fresh_until: Some("2030-01-01T00:00:00Z".into()),
    };
    run(add());
    run(add());
    run(TaskCmd::Close {
        key: "Current HTTP rule".into(),
        result:
            "The standard supports the behavior, but implementation remains a product decision."
                .into(),
    });
    let store = Store::open(tmp.path()).unwrap();
    let task = store
        .resolve_node("Current HTTP rule", Some(NodeType::TaskRecord))
        .unwrap();
    assert_eq!(ResearchBody::parse(&task.body).unwrap().sources.len(), 1);
    let notes = store.notes_for(&intent.id).unwrap();
    assert_eq!(notes[0].body["research_task_id"], task.id);
    assert!(!store
        .all_facts()
        .unwrap()
        .iter()
        .any(|f| f.subject_id == task.id
            || (f.subject_kind == TargetKind::Node && f.criterion.contains("rfc9110"))));
    let export = loom::travel::Export::from_snapshot(store.snapshot().unwrap())
        .to_json()
        .unwrap();
    assert!(export.contains("quote_fingerprint") && export.contains("preferred_sources"));
}

#[test]
fn door_offers_external_research() {
    let tmp = Tmp::new();
    Store::init(tmp.path(), Some("t"), false).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_loom"))
        .args([
            "--graph",
            tmp.path().to_str().unwrap(),
            "--json",
            "door",
            "Which current regulation applies?",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("external_research") && text.contains("--why-external"));
}

/// **A non-expiring source makes the conclusion non-expiring.**
///
/// Freshness is bound at close from the sources still usable then, and the
/// binding takes the EARLIEST expiry among them — except that a source with no
/// expiry at all cannot be outlived, so it makes the whole conclusion
/// non-expiring. Getting this backwards (taking the earliest and ignoring the
/// non-expiring source) would retire a conclusion that nothing has actually
/// invalidated.
#[test]
fn a_non_expiring_source_makes_the_conclusion_non_expiring() {
    let now = chrono::DateTime::parse_from_rfc3339("2026-08-02T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    // Two sources that expire, one that never does.
    let mut body = ResearchBody::parse(&research_body(vec![
        source(Some("2027-01-01T00:00:00Z")),
        source(Some("2030-01-01T00:00:00Z")),
        source(None),
    ]))
    .unwrap();
    body.stamp_conclusion_freshness(now).unwrap();
    assert_eq!(
        body.conclusion_fresh_until, None,
        "a source that never expires cannot be outlived by one that does"
    );

    // With every source dated, the conclusion inherits the EARLIEST of them:
    // the first fact to go stale is the one that bounds the synthesis.
    let mut dated = ResearchBody::parse(&research_body(vec![
        source(Some("2030-01-01T00:00:00Z")),
        source(Some("2027-01-01T00:00:00Z")),
    ]))
    .unwrap();
    dated.stamp_conclusion_freshness(now).unwrap();
    assert_eq!(
        dated.conclusion_fresh_until.as_deref(),
        Some("2027-01-01T00:00:00Z"),
        "the earliest usable expiry bounds the conclusion"
    );
}

/// **An already-stale source does not shorten a conclusion its replacement
/// supports.**
///
/// Historical sources stay in the record as provenance. If a fresher source
/// covers the same ground, the conclusion is bound by the fresh one — otherwise
/// keeping honest history would silently expire the answer.
#[test]
fn a_stale_source_does_not_bound_a_conclusion_a_current_one_supports() {
    let now = chrono::DateTime::parse_from_rfc3339("2026-08-02T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let mut stale = source(Some("2020-01-01T00:00:00Z"));
    stale.retrieved_at = "2019-01-01T00:00:00Z".into();
    stale.published_at = None;

    let mut body = ResearchBody::parse(&research_body(vec![
        stale,
        source(Some("2030-01-01T00:00:00Z")),
    ]))
    .unwrap();
    body.stamp_conclusion_freshness(now).unwrap();
    assert_eq!(
        body.conclusion_fresh_until.as_deref(),
        Some("2030-01-01T00:00:00Z"),
        "the expired source is provenance, not a bound on the current answer"
    );
}

/// **An expired conclusion is still served — labelled stale, and never
/// re-queued.**
///
/// The product decision behind this (question f06e8b72): when a conclusion
/// passes its freshness date, loom keeps showing it with a stale marker where a
/// reader will see it, and does NOT route it back for a refresh. Surfacing
/// staleness at the point of use is how loom treats staleness everywhere;
/// turning every expiring source into queue work would manufacture an
/// obligation only a human could clear.
///
/// Both halves are asserted, because the second is the one a future change
/// would break silently.
#[test]
fn an_expired_conclusion_is_served_labelled_stale_and_never_queued() {
    let tmp = Tmp::new();
    std::fs::write(tmp.path().join("cart.rs"), "pub fn add_item() {}\n").unwrap();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();

    let intent = store
        .add_node(
            NodeType::Intent,
            "items can be added to a cart",
            "an item lands in the cart",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = store
        .add_node(NodeType::CodeFile, "cart.rs", "", "", serde_json::json!({}))
        .unwrap();
    store
        .add_edge(
            loom::model::EdgeKind::Implements,
            &intent.id,
            &cf.id,
            loom::model::TruthClass::Asserted,
        )
        .unwrap();

    // A completed research task whose conclusion has lapsed. Conclusion
    // freshness is immutable once stamped, so it can expire while a
    // later-appended source is still current — exactly the state that must
    // still reach the reader, marked.
    let mut body =
        ResearchBody::parse(&research_body(vec![source(Some("2030-01-01T00:00:00Z"))])).unwrap();
    body.conclusion_fresh_until = Some("2020-01-01T00:00:00Z".into());
    let task = store
        .add_node(
            NodeType::TaskRecord,
            "which cart semantics does the spec require",
            "the spec requires idempotent add",
            "completed",
            serde_json::to_value(&body).unwrap(),
        )
        .unwrap();
    store
        .add_node(
            NodeType::Note,
            &format!("Research reference: {}", task.name),
            "Governed research advisory",
            "context",
            serde_json::json!({
                "kind": "research_reference",
                "target_id": intent.id,
                "research_task_id": task.id
            }),
        )
        .unwrap();

    // Served: the work packet for this behavior carries the conclusion, marked,
    // with the recommendation suppressed rather than the entry dropped.
    let packet = loom::workitem::next(&store, Some(Lane::Validate))
        .unwrap()
        .expect("an implemented intent with no proof is validate work");
    let rendered = serde_json::to_string(&packet).unwrap();
    assert!(
        rendered.contains("STALE research conclusion"),
        "an expired conclusion must still reach the reader, marked: {rendered}"
    );
    assert!(
        rendered.contains("suppress recommendation"),
        "and must tell the reader not to act on it: {rendered}"
    );

    // NOT re-queued: expiry is surfaced where it is read, never turned into
    // work. No lane may serve the research task itself.
    for lane in Lane::LADDER {
        let items = loom::workitem::queue_items(&store, *lane).unwrap();
        assert!(
            !items.iter().any(|i| i.target.id == task.id),
            "expired research must not become queue work in lane {lane:?}"
        );
    }
}
