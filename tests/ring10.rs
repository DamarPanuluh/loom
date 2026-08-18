//! Ring 10 tests — completeness scorecard, waivers, questions axis + pulse,
//! elaborate queue, prescreen, scan adapters, and config travel.
//!
//! Each test names the externally observable contract it defends. Failure
//! messages are prefixed with the numbered contract so a red run points at the
//! violated behavior.

use loom::completeness::{self, AXES};
use loom::lane::Lane;
use loom::model::{EdgeKind, NodeType, TargetKind, TruthClass};
use loom::packs;
use loom::scan;
use loom::store::Store;
use loom::travel::Export;
use loom::workitem::{self, graph_state};
mod common;
use common::*;

// ---- shared builders --------------------------------------------------------

/// A feature-level intent with the given visibility facet.
fn feature_intent(store: &Store, name: &str, visibility: Option<&str>) -> loom::model::Node {
    let n = store
        .add_node(
            NodeType::Intent,
            name,
            "one falsifiable behavior",
            "implemented",
            serde_json::json!({ "level": "feature" }),
        )
        .unwrap();
    store
        .set_facet(
            &n.id,
            TargetKind::Node,
            "level",
            "feature",
            TruthClass::Asserted,
        )
        .unwrap();
    if let Some(v) = visibility {
        store
            .set_facet(
                &n.id,
                TargetKind::Node,
                "visibility",
                v,
                TruthClass::Asserted,
            )
            .unwrap();
    }
    n
}

/// A scenario intent linked to its happy-path parent via a ScenarioOf edge,
/// carrying the given aspect facet.
fn scenario_of(
    store: &Store,
    name: &str,
    aspect: &str,
    parent: &loom::model::Node,
) -> loom::model::Node {
    let s = store
        .add_node(
            NodeType::Intent,
            name,
            "a falsifiable scenario criterion",
            "planned",
            serde_json::json!({ "level": "feature" }),
        )
        .unwrap();
    store
        .set_facet(
            &s.id,
            TargetKind::Node,
            "level",
            "feature",
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &s.id,
            TargetKind::Node,
            "aspect",
            aspect,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::ScenarioOf,
            &s.id,
            &parent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    s
}

/// Look up one axis state in a scorecard by axis name.
fn axis_state<'a>(
    card: &'a loom::completeness::Scorecard,
    name: &str,
) -> &'a loom::completeness::AxisState {
    card.axes
        .iter()
        .find(|a| a.axis == name)
        .unwrap_or_else(|| panic!("scorecard missing axis '{name}'"))
}

// ===========================================================================
// 1. COMPLETENESS SCORECARD
// ===========================================================================

#[test]
fn scorecard_user_visible_feature_with_no_surroundings_scores_axes_open() {
    // Contract 1: a user_visible feature intent with no scenarios/proof/journey
    // scores those axes `open`; prerequisites defaults to met (none declared).
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = feature_intent(&store, "user can log in", Some("user_visible"));

    let card = completeness::scorecard(&store, &intent).unwrap();
    assert_eq!(
        card.intent_id, intent.id,
        "contract 1: scorecard is keyed by the intent id"
    );
    assert_eq!(
        card.visibility.as_deref(),
        Some("user_visible"),
        "contract 1: scorecard echoes the visibility facet"
    );
    assert_eq!(
        card.axes.len(),
        AXES.len(),
        "contract 1: scorecard carries exactly the fixed axis set"
    );
    assert_eq!(
        axis_state(&card, "scenarios").state,
        "open",
        "contract 1: scenarios axis is open when no sad/fallback/edge_case scenarios exist"
    );
    assert_eq!(
        axis_state(&card, "proof").state,
        "open",
        "contract 1: proof axis is open when no validation is registered"
    );
    assert_eq!(
        axis_state(&card, "journey").state,
        "open",
        "contract 1: journey axis is open when no journey proof or coverage exists"
    );
    assert_eq!(
        axis_state(&card, "prerequisites").state,
        "met",
        "contract 1: prerequisites axis is met when none are declared"
    );
    assert_eq!(
        axis_state(&card, "questions").state,
        "met",
        "contract 1: questions axis is met when no open question inbox items exist"
    );
    assert_eq!(
        card.open, 3,
        "contract 1: open count matches the three open axes"
    );
}

#[test]
fn scorecard_aspect_tagged_scenarioof_intents_close_the_scenarios_axis() {
    // Contract 1: adding aspect-tagged (sad+fallback+edge_case) ScenarioOf-linked
    // intents closes the scenarios axis to `met`.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = feature_intent(&store, "user can log in", Some("user_visible"));

    // Before: open.
    let before = completeness::scorecard(&store, &intent).unwrap();
    assert_eq!(
        axis_state(&before, "scenarios").state,
        "open",
        "contract 1: scenarios open before surrounding scenarios are added"
    );

    scenario_of(&store, "login with wrong password", "sad", &intent);
    scenario_of(
        &store,
        "login when auth service is down",
        "fallback",
        &intent,
    );
    scenario_of(&store, "login with empty password", "edge_case", &intent);

    let after = completeness::scorecard(&store, &intent).unwrap();
    assert_eq!(
        axis_state(&after, "scenarios").state,
        "met",
        "contract 1: scenarios axis is met once sad+fallback+edge_case ScenarioOf children exist"
    );
}

#[test]
fn scorecard_one_sad_scenario_closes_the_scenarios_axis() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = feature_intent(&store, "user can log in", Some("user_visible"));
    scenario_of(&store, "login with wrong password", "sad", &intent);
    let card = completeness::scorecard(&store, &intent).unwrap();
    assert_eq!(
        axis_state(&card, "scenarios").state,
        "met",
        "contract 1: any one of sad/fallback/edge_case closes scenarios"
    );
}

#[test]
fn scorecard_journey_exemption_is_not_applicable() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = feature_intent(&store, "repository plumbing holds", Some("user_visible"));
    store
        .set_facet(
            &intent.id,
            TargetKind::Node,
            "journey_exemption",
            r#"{"human_decision_digest":"sha256:ring10-exemption","kind":"infrastructure","reason":"not independently user-reachable"}"#,
            TruthClass::Asserted,
        )
        .unwrap();
    let card = completeness::scorecard(&store, &intent).unwrap();
    let journey = axis_state(&card, "journey");
    assert_eq!(
        journey.state, "not_applicable",
        "contract 1: a canonical journey_exemption closes the journey axis"
    );
    assert!(
        journey.detail.contains("infrastructure"),
        "the axis should name the exemption kind: {}",
        journey.detail
    );
}

#[test]
fn scorecard_internal_intent_gets_not_applicable_for_scenarios_and_journey() {
    // Contract 1: an internal intent (no user_visible facet) gets
    // not_applicable for scenarios and journey; questions still applies.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    // No visibility facet → internal.
    let intent = feature_intent(&store, "persist session token", None);

    let card = completeness::scorecard(&store, &intent).unwrap();
    assert_eq!(
        card.visibility, None,
        "contract 1: internal intent has no visibility facet echoed"
    );
    assert_eq!(
        axis_state(&card, "scenarios").state,
        "not_applicable",
        "contract 1: internal intents are not_applicable for scenarios"
    );
    assert_eq!(
        axis_state(&card, "journey").state,
        "not_applicable",
        "contract 1: internal intents are not_applicable for journey"
    );
    assert_eq!(
        axis_state(&card, "questions").state,
        "met",
        "contract 1: questions axis still applies to internal intents"
    );
}

#[test]
fn scorecard_intent_carrying_aspect_sad_is_itself_a_scenario() {
    // Contract 1: an intent that itself carries aspect=sad gets scenarios
    // not_applicable — it IS a scenario, not a scenario-needing happy path.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let happy = feature_intent(&store, "user can log in", Some("user_visible"));
    let sad = store
        .add_node(
            NodeType::Intent,
            "login with wrong password",
            "a falsifiable sad-path criterion",
            "planned",
            serde_json::json!({ "level": "feature" }),
        )
        .unwrap();
    store
        .set_facet(
            &sad.id,
            TargetKind::Node,
            "level",
            "feature",
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &sad.id,
            TargetKind::Node,
            "aspect",
            "sad",
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &sad.id,
            TargetKind::Node,
            "visibility",
            "user_visible",
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::ScenarioOf,
            &sad.id,
            &happy.id,
            TruthClass::Asserted,
        )
        .unwrap();

    let card = completeness::scorecard(&store, &sad).unwrap();
    assert_eq!(
        axis_state(&card, "scenarios").state,
        "not_applicable",
        "contract 1: an intent that IS a sad scenario is not_applicable for scenarios"
    );
    let _ = happy; // suppress unused warning
}

#[test]
fn scenario_boundary_inherits_every_happy_path_leaf_verdict() {
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    packs::seed(&store, "iso5055").unwrap();
    let rule = store
        .resolve_node(
            "iso5055-rel-no-unchecked-failure",
            Some(NodeType::QualityRule),
        )
        .unwrap();

    let root = feature_intent(&store, "user completes checkout", Some("user_visible"));
    let card_leaf = feature_intent(&store, "card payment completes", Some("user_visible"));
    let cash_leaf = feature_intent(&store, "cash payment completes", Some("user_visible"));
    for leaf in [&card_leaf, &cash_leaf] {
        store
            .add_edge(
                EdgeKind::Hierarchy,
                &root.id,
                &leaf.id,
                TruthClass::Asserted,
            )
            .unwrap();
    }
    let sad = scenario_of(&store, "checkout payment fails", "sad", &root);
    let nested = scenario_of(
        &store,
        "failed payment provider disappears",
        "edge_case",
        &sad,
    );

    let card_rule = store
        .add_edge(
            EdgeKind::Governs,
            &rule.id,
            &card_leaf.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &card_rule.id,
            loom::model::InspectionStatus::Passing,
            "failures are handled",
            "src/payment.rs:10-20",
            0.9,
            "llm",
        )
        .unwrap();

    for intent in [&root, &sad, &nested] {
        let card = completeness::scorecard(&store, intent).unwrap();
        let boundary = axis_state(&card, "boundary");
        assert_eq!(
            boundary.state, "open",
            "an uncovered sibling quality surface must keep '{}' open",
            intent.name
        );
        assert!(
            boundary.detail.contains("1 code-bearing quality surface"),
            "the missing-surface count must be explicit: {}",
            boundary.detail
        );
    }

    let cash_rule = store
        .add_edge(
            EdgeKind::Governs,
            &rule.id,
            &cash_leaf.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &cash_rule.id,
            loom::model::InspectionStatus::Passing,
            "failures are handled",
            "src/payment.rs:30-40",
            0.9,
            "llm",
        )
        .unwrap();

    for intent in [&root, &sad, &nested] {
        let card = completeness::scorecard(&store, intent).unwrap();
        assert_eq!(
            axis_state(&card, "boundary").state,
            "met",
            "scenario '{}' must inherit complete quality coverage from every happy-path leaf",
            intent.name
        );
    }
}

#[test]
fn all_scorecards_lists_feature_intents_most_incomplete_first() {
    // Contract 1: all_scorecards returns feature-level intents, sorted
    // most-incomplete first; a non-feature intent is excluded. Scenario
    // intents are feature-level too, so they appear in the list.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let complete = feature_intent(&store, "complete behavior", Some("user_visible"));
    // Surround it so its scenarios axis is met (fewer open axes).
    scenario_of(&store, "complete sad", "sad", &complete);
    scenario_of(&store, "complete fallback", "fallback", &complete);
    scenario_of(&store, "complete edge", "edge_case", &complete);

    let _incomplete = feature_intent(&store, "incomplete behavior", Some("user_visible"));

    // A component-level intent must be excluded.
    let component = store
        .add_node(
            NodeType::Intent,
            "component detail",
            "a component detail",
            "implemented",
            serde_json::json!({ "level": "component" }),
        )
        .unwrap();
    store
        .set_facet(
            &component.id,
            TargetKind::Node,
            "level",
            "component",
            TruthClass::Asserted,
        )
        .unwrap();

    let cards = completeness::all_scorecards(&store).unwrap();
    let names: Vec<&str> = cards.iter().map(|c| c.intent_name.as_str()).collect();
    assert!(
        !names.contains(&"component detail"),
        "contract 1: all_scorecards excludes non-feature intents"
    );
    assert!(
        names.contains(&"incomplete behavior") && names.contains(&"complete behavior"),
        "contract 1: all_scorecards includes both happy-path feature intents"
    );
    assert_eq!(
        cards[0].intent_name, "incomplete behavior",
        "contract 1: most-incomplete intent sorts first"
    );
    assert!(
        cards[0].open > cards[1].open,
        "contract 1: ordering is by descending open count"
    );
}

#[test]
fn check_axis_rejects_unknown_axes_and_accepts_known() {
    // Contract 1: check_axis rejects unknown axis labels and accepts each
    // declared axis.
    for a in AXES {
        assert!(
            completeness::check_axis(a).is_ok(),
            "contract 1: check_axis accepts declared axis '{a}'"
        );
    }
    assert!(
        completeness::check_axis("bogus").is_err(),
        "contract 1: check_axis rejects an unknown axis"
    );
    assert!(
        completeness::check_axis("").is_err(),
        "contract 1: check_axis rejects an empty axis label"
    );
}

// ===========================================================================
// 2. WAIVERS
// ===========================================================================

#[test]
fn waiver_facility_turns_open_axis_into_waived_with_reason() {
    // Contract 2: a `waiver:<axis>` facet turns an open axis into `waived`
    // carrying the reason.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = feature_intent(&store, "user can log in", Some("user_visible"));

    // scenarios is open by default.
    let before = completeness::scorecard(&store, &intent).unwrap();
    assert_eq!(
        axis_state(&before, "scenarios").state,
        "open",
        "contract 2: scenarios axis is open before a waiver"
    );

    store
        .set_facet(
            &intent.id,
            TargetKind::Node,
            "waiver:scenarios",
            "single-user CLI — no failure scenarios apply",
            TruthClass::Asserted,
        )
        .unwrap();

    let after = completeness::scorecard(&store, &intent).unwrap();
    let s = axis_state(&after, "scenarios");
    assert_eq!(
        s.state, "waived",
        "contract 2: waiver:scenarios facet flips the axis to waived"
    );
    assert_eq!(
        s.waived_reason.as_deref(),
        Some("single-user CLI — no failure scenarios apply"),
        "contract 2: waived axis carries the recorded reason"
    );
    // The open count must drop since the axis is no longer open.
    assert!(
        after.open < before.open,
        "contract 2: waiving an open axis reduces the open count"
    );
}

#[test]
fn waiver_does_not_apply_to_a_met_axis() {
    // Contract 2: a waiver facet on an already-met axis does not flip it to
    // waived — waivers only apply to open axes.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = feature_intent(&store, "user can log in", Some("user_visible"));
    // prerequisites is met (none declared).
    store
        .set_facet(
            &intent.id,
            TargetKind::Node,
            "waiver:prerequisites",
            "n/a",
            TruthClass::Asserted,
        )
        .unwrap();
    let card = completeness::scorecard(&store, &intent).unwrap();
    assert_eq!(
        axis_state(&card, "prerequisites").state,
        "met",
        "contract 2: a waiver on a met axis does not flip it to waived"
    );
    assert!(
        axis_state(&card, "prerequisites").waived_reason.is_none(),
        "contract 2: a met axis carries no waived reason even if a waiver facet exists"
    );
}

#[test]
fn redefine_intent_clears_waiver_facets_and_records_decision_note() {
    // Contract 2: Store::redefine_intent clears waiver:* facets (axis back to
    // open) and records a decision note.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = feature_intent(&store, "user can log in", Some("user_visible"));
    store
        .set_facet(
            &intent.id,
            TargetKind::Node,
            "waiver:scenarios",
            "single-user CLI",
            TruthClass::Asserted,
        )
        .unwrap();
    let waived = completeness::scorecard(&store, &intent).unwrap();
    assert_eq!(
        axis_state(&waived, "scenarios").state,
        "waived",
        "contract 2: axis is waived before redefinition"
    );

    store
        .redefine_intent(&intent.id, "user can log in with a password")
        .unwrap();

    let reopened = completeness::scorecard(&store, &intent).unwrap();
    assert_eq!(
        axis_state(&reopened, "scenarios").state,
        "open",
        "contract 2: redefine_intent re-opens a previously waived axis"
    );
    assert!(
        axis_state(&reopened, "scenarios").waived_reason.is_none(),
        "contract 2: redefined axis carries no waived reason"
    );

    // A decision note recording the waiver re-opening was added.
    let notes: Vec<loom::model::Node> = store
        .list_nodes(Some(NodeType::Note), usize::MAX)
        .unwrap()
        .into_iter()
        .filter(|n| n.body.get("target_id").and_then(|v| v.as_str()) == Some(intent.id.as_str()))
        .collect();
    assert!(
        notes.iter().any(|n| n.description.contains("waiver")),
        "contract 2: redefine_intent records a decision note mentioning the cleared waivers"
    );
    assert!(
        notes.iter().any(|n| n.description.contains("redefined")),
        "contract 2: redefine_intent records a decision note preserving the old wording"
    );
}

#[test]
fn questions_axis_is_never_waivable() {
    // Contract 2: the questions axis is NEVER waivable — a waiver:questions
    // facet must not flip it. The questions axis skips apply_waiver.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = feature_intent(&store, "user can log in", Some("user_visible"));

    // No open questions → questions axis is `met`. Record a waiver:questions
    // facet anyway; it must not change the state nor attach a reason.
    store
        .set_facet(
            &intent.id,
            TargetKind::Node,
            "waiver:questions",
            "I will answer them later",
            TruthClass::Asserted,
        )
        .unwrap();
    let card = completeness::scorecard(&store, &intent).unwrap();
    let q = axis_state(&card, "questions");
    assert_ne!(
        q.state, "waived",
        "contract 2: questions axis is never waivable — waiver:questions does not flip it to waived"
    );
    assert!(
        q.waived_reason.is_none(),
        "contract 2: questions axis never carries a waived reason"
    );

    // And when questions ARE open (via a Question node), the waiver still does not apply.
    let question = store
        .add_node(
            NodeType::Question,
            "should login support SSO?",
            "should login support SSO?",
            "open",
            serde_json::json!({ "intent": intent.id.clone() }),
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Questions,
            &question.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    let card_open = completeness::scorecard(&store, &intent).unwrap();
    let q_open = axis_state(&card_open, "questions");
    assert_eq!(
        q_open.state, "open",
        "contract 2: questions axis is open when an open Question node exists"
    );
    assert!(
        q_open.waived_reason.is_none(),
        "contract 2: an open questions axis is not flipped to waived by a waiver:questions facet"
    );
}

// ===========================================================================
// 3. QUESTIONS AXIS + PULSE
// ===========================================================================

#[test]
fn question_node_opens_questions_axis_and_increments_open_questions() {
    // Contract 3 (updated): a first-class Question node linked to an intent by
    // a `questions` edge opens the questions axis and increments open_questions.
    // InboxItem{source:"question"} is no longer the backing mechanism.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = feature_intent(&store, "user can log in", Some("user_visible"));

    let before = graph_state(&store).unwrap();
    assert_eq!(
        before.open_questions, 0,
        "contract 3: open_questions starts at zero"
    );
    let card_before = completeness::scorecard(&store, &intent).unwrap();
    assert_eq!(
        axis_state(&card_before, "questions").state,
        "met",
        "contract 3: questions axis is met before any question is opened"
    );

    // Create a Question node linked to the intent via a `questions` edge.
    let question = store
        .add_node(
            NodeType::Question,
            "should login support SSO?",
            "should login support SSO?",
            "open",
            serde_json::json!({ "intent": intent.id.clone() }),
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Questions,
            &question.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();

    let after = graph_state(&store).unwrap();
    assert_eq!(
        after.open_questions, 1,
        "contract 3: an open Question node increments open_questions"
    );
    let card_after = completeness::scorecard(&store, &intent).unwrap();
    assert_eq!(
        axis_state(&card_after, "questions").state,
        "open",
        "contract 3: a linked open Question opens the questions axis"
    );
}

#[test]
fn answering_question_node_closes_questions_axis_and_decrements_open_questions() {
    // Contract 3: setting a linked Question to status "answered" closes the
    // questions axis and decrements open_questions back to zero.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = feature_intent(&store, "user can log in", Some("user_visible"));
    let question = store
        .add_node(
            NodeType::Question,
            "should login support SSO?",
            "should login support SSO?",
            "open",
            serde_json::json!({ "intent": intent.id.clone() }),
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Questions,
            &question.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();

    let open = graph_state(&store).unwrap();
    assert_eq!(
        open.open_questions, 1,
        "contract 3: open_questions is 1 before answer"
    );

    store.set_node_status(&question.id, "answered").unwrap();

    let closed = graph_state(&store).unwrap();
    assert_eq!(
        closed.open_questions, 0,
        "contract 3: answering the question decrements open_questions to zero"
    );
    let card = completeness::scorecard(&store, &intent).unwrap();
    assert_eq!(
        axis_state(&card, "questions").state,
        "met",
        "contract 3: an answered question closes the questions axis"
    );
}

#[test]
fn non_question_inbox_item_does_not_open_the_questions_axis() {
    // Contract 3: an InboxItem (any source) does not open the questions axis
    // nor count toward open_questions. Only first-class Question nodes linked
    // to an intent by a `questions` edge drive the questions axis.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = feature_intent(&store, "user can log in", Some("user_visible"));
    store
        .add_node(
            NodeType::InboxItem,
            "a human note",
            "remember to revisit login copy",
            "new",
            serde_json::json!({
                "source": "human",
                "link": format!("intent:{}", intent.id),
            }),
        )
        .unwrap();
    let pulse = graph_state(&store).unwrap();
    assert_eq!(
        pulse.open_questions, 0,
        "contract 3: a non-question inbox item does not count toward open_questions"
    );
    let card = completeness::scorecard(&store, &intent).unwrap();
    assert_eq!(
        axis_state(&card, "questions").state,
        "met",
        "contract 3: a non-question inbox item does not open the questions axis"
    );
}

#[test]
fn question_close_deferred_closes_questions_axis() {
    // Contract 3: closing a Question with status "deferred" (non-"open" status)
    // closes the questions axis and decrements open_questions.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = feature_intent(&store, "user can log in", Some("user_visible"));
    let question = store
        .add_node(
            NodeType::Question,
            "should login support SSO?",
            "should login support SSO?",
            "open",
            serde_json::json!({ "intent": intent.id.clone() }),
        )
        .unwrap();
    store
        .add_edge(
            EdgeKind::Questions,
            &question.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();

    assert_eq!(graph_state(&store).unwrap().open_questions, 1);

    store.set_node_status(&question.id, "deferred").unwrap();

    assert_eq!(
        graph_state(&store).unwrap().open_questions,
        0,
        "contract 3: a deferred question is no longer open"
    );
    let card = completeness::scorecard(&store, &intent).unwrap();
    assert_eq!(
        axis_state(&card, "questions").state,
        "met",
        "contract 3: deferred question closes the questions axis"
    );
}

// ===========================================================================
// 4. ELABORATE QUEUE
// ===========================================================================

#[test]
fn mode_parse_elaborate_yields_elaborate_mode() {
    // Contract 4: Lane::parse("elaborate") returns Some(Lane::Elaborate).
    assert_eq!(
        Lane::parse("elaborate"),
        Some(Lane::Elaborate),
        "contract 4: Lane::parse(\"elaborate\") yields Lane::Elaborate"
    );
}

#[test]
fn elaborate_serves_incomplete_user_visible_feature_intent() {
    // Contract 4: with one incomplete user_visible feature intent,
    // next(store, Some(Lane::Elaborate)) serves mode=elaborate, owner_role=
    // builder, scorecard Some (JSON contains axes), and prompt_contract.
    // write_back non-empty.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let _intent = feature_intent(&store, "user can log in", Some("user_visible"));

    let item = workitem::next(&store, Some(Lane::Elaborate))
        .unwrap()
        .expect("contract 4: elaborate serves an incomplete user-visible feature intent");
    assert_eq!(
        item.mode, "elaborate",
        "contract 4: elaborate item reports mode=elaborate"
    );
    assert_eq!(
        item.owner_role, "builder",
        "contract 4: elaborate item is owned by the builder role"
    );
    let card = item
        .scorecard
        .as_ref()
        .expect("contract 4: elaborate item carries a scorecard");
    let axes = card
        .get("axes")
        .expect("contract 4: scorecard JSON contains an axes array");
    assert!(
        axes.is_array(),
        "contract 4: scorecard JSON axes field is an array"
    );
    assert!(
        !item.prompt_contract.write_back.is_empty(),
        "contract 4: elaborate prompt_contract.write_back is non-empty"
    );
    assert!(
        item.prompt_contract.mindset.contains("FIRST tell them")
            && item.prompt_contract.mindset.contains("plain language")
            && item.prompt_contract.mindset.contains("may not know Loom"),
        "contract 4: the LLM must proactively introduce intent-completion help without assuming Loom knowledge"
    );
    assert!(
        item.prompt_contract.stop_condition.contains("ask ONE")
            && item.prompt_contract.stop_condition.contains("wait for the user")
            && item.prompt_contract.write_back.contains("loom question answer"),
        "contract 4: a product decision must become one conversational question followed by a recorded human answer"
    );
    assert!(
        item.prompt_contract.forbidden_actions.iter().any(|rule| {
            rule.contains("implementation details") && rule.contains("engineering judgment")
        }),
        "contract 4: elaboration must not burden the user with safely inferable technical choices"
    );
}

#[test]
fn elaborate_contract_offers_unnamed_wantedness_without_minting_first() {
    // Contract 4: unnamed wantedness is offered as Keep / Decline / Revise,
    // then the LLM waits. Minting an unratified intent is not the offer.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let _intent = feature_intent(&store, "user can log in", Some("user_visible"));

    let item = workitem::next(&store, Some(Lane::Elaborate))
        .unwrap()
        .expect("contract 4: elaborate serves an incomplete user-visible feature intent");
    let contract = &item.prompt_contract;
    assert!(
        contract.mindset.contains("Keep / Decline / Revise")
            && contract.mindset.contains("unnamed")
            && contract.mindset.contains("Silence is not wantedness"),
        "contract 4: the elaborate mindset must tell the LLM to offer unnamed wantedness and wait"
    );
    assert!(
        contract.allowed_actions.iter().any(|action| {
            action.contains("unnamed wantedness")
                && action.contains("Keep / Decline / Revise")
                && action.contains("WAIT")
                && action.contains("mint or ratify only after")
        }),
        "contract 4: unnamed wantedness is an allowed offer, not a silent mint"
    );
    assert!(
        contract.forbidden_actions.iter().any(|rule| {
            rule.contains("minting an unratified intent as a way to offer unnamed wantedness")
        }),
        "contract 4: minting first must not be the way to offer unnamed wantedness"
    );
    assert!(
        contract
            .forbidden_actions
            .iter()
            .any(|rule| rule.contains("finding") && rule.contains("brainstormed feature")),
        "contract 4: a finding or brainstorm is not wantedness"
    );
    assert!(
        contract.stop_condition.contains("Keep / Decline / Revise")
            && contract.stop_condition.contains("no answer means no mint"),
        "contract 4: silence after an unnamed-wantedness offer must not mint"
    );
}

#[test]
fn elaborate_returns_none_when_no_user_visible_feature_intents() {
    // Contract 4: with no user_visible feature intents, elaborate returns None.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    // An internal feature intent only — not user_visible.
    let _internal = feature_intent(&store, "persist session token", None);
    let item = workitem::next(&store, Some(Lane::Elaborate)).unwrap();
    assert!(
        item.is_none(),
        "contract 4: elaborate returns None when no user_visible feature intent is incomplete"
    );
}

#[test]
fn default_next_reaches_elaborate_only_after_other_queues_drain() {
    // Contract 4: default next(None) only reaches elaborate after other queues
    // are empty. Seed a failing edge (fix queue) and assert the fix item wins
    // over elaborate; then clear it and assert elaborate surfaces.
    //
    // No quality rules are seeded and no codefiles are registered, so once the
    // single failing edge is resolved every other queue is empty and elaborate
    // is the only remaining work.
    let tmp = Tmp::new();
    // The groundings below cite this file, so it has to exist: a citation into a
    // file that is not there is not evidence.
    tmp.write(
        "src/auth.rs",
        &(1..=60)
            .map(|n| format!("// auth line {n}\n"))
            .collect::<String>(),
    );
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let a = feature_intent(&store, "user can log in", Some("user_visible"));
    let b = feature_intent(&store, "user can log out", Some("user_visible"));
    let rel = store
        .add_edge(EdgeKind::Relates, &a.id, &b.id, TruthClass::Asserted)
        .unwrap();
    store
        .record_verdict(
            &rel.id,
            loom::model::InspectionStatus::Failing,
            "logout does not invalidate the login session",
            "src/auth.rs:40",
            0.9,
            "llm",
        )
        .unwrap();

    // With a failing verdict, the fix queue must win over elaborate.
    let item = workitem::next(&store, None)
        .unwrap()
        .expect("contract 4: default next serves the failing verdict before elaborate");
    assert_eq!(
        item.mode, "fix",
        "contract 4: default next serves the fix queue before elaborate when a failing verdict exists"
    );

    // Resolve the failure (passing verdict) so the fix queue drains.
    store
        .record_verdict(
            &rel.id,
            loom::model::InspectionStatus::Passing,
            "logout invalidates the session",
            "src/auth.rs:40",
            0.9,
            "llm",
        )
        .unwrap();

    // Both intents are `implemented` but ungrounded — that is legitimate BUILD
    // work (the build lane grounds implemented-but-unlinked intents so the
    // compass never routes `build` at an empty queue). Ground each and inspect
    // the grounding so the build AND analyze queues drain; only then is
    // elaborate genuinely the last remaining queue.
    let codefile = codefile(&store, "src/auth.rs");
    for intent in [&a, &b] {
        let impl_edge = store
            .add_edge(
                EdgeKind::Implements,
                &intent.id,
                &codefile.id,
                TruthClass::Asserted,
            )
            .unwrap();
        store
            .record_verdict(
                &impl_edge.id,
                loom::model::InspectionStatus::Passing,
                "grounded and inspected",
                "src/auth.rs:1",
                0.9,
                "llm",
            )
            .unwrap();
    }

    // Both intents are implemented with no passing proof, which is real
    // validate work — the `proven` rung counts them and the lane now serves
    // them. Give each a passing proof so the validate queue genuinely drains;
    // otherwise this test asserts "elaborate is last" from a graph where an
    // earlier lane still has work.
    for intent in [&a, &b] {
        // A REAL proof: loom runs the command and records what it observed.
        // Hand-recording a passing verdict here is refused now — which is the
        // point, since that is the same shortcut that made 54 of this graph's
        // own proofs green without loom ever running them.
        loom::commands::prove_intent(
            &store,
            &intent.id,
            &format!("{}-proof", intent.name),
            "true",
        )
        .unwrap();
    }

    // `proven` requires S2: use a real test-runner summary rather than the
    // retired executable-Journey metadata path.
    for (intent, slug) in [(&a, "flow-a"), (&b, "flow-b")] {
        let val = store
            .add_node(
                NodeType::Validation,
                &format!("{slug} proof"),
                "",
                "not_run",
                serde_json::json!({
                    "type": "test",
                    "command": "printf 'test result: ok. 1 passed; 0 failed\\n'",
                }),
            )
            .unwrap();
        store
            .ensure_edge(EdgeKind::Validates, &val.id, &intent.id)
            .unwrap();
        let fresh = store.get_node(&val.id).unwrap().unwrap();
        loom::commands::observe_validation(&store, &fresh).unwrap();
        // The Journey axis is a separate question from proof depth. This
        // ordering fixture is deliberately rootless infrastructure, expressed
        // through the dedicated canonical exemption rather than a legacy
        // generic waiver.
        store
            .set_facet(
                &intent.id,
                TargetKind::Node,
                "journey_exemption",
                r#"{"human_decision_digest":"sha256:ring10","kind":"test_fixture","reason":"exercises queue precedence, not end-to-end depth"}"#,
                TruthClass::Asserted,
            )
            .unwrap();
    }
    // No sync here on purpose: `observe_validation` grades the proof in place,
    // and a sync would materialize structural smells this ordering fixture
    // never intended to create — putting triage in front of elaborate and
    // testing something other than the precedence this test is about.

    // Seed and measure quality rules so the measured rung is non-vacuously met;
    // otherwise the default walk serves the unseeded-quality seed packet before
    // elaborate, which would test a different precedence than this contract.
    loom::packs::seed(&store, "iso5055").unwrap();
    let rules = store
        .list_nodes(Some(NodeType::QualityRule), usize::MAX)
        .unwrap();
    let implemented = store
        .list_nodes(Some(NodeType::Intent), usize::MAX)
        .unwrap()
        .into_iter()
        .filter(|n| n.status == "implemented")
        .collect::<Vec<_>>();
    for rule in &rules {
        for intent in &implemented {
            let ge = store
                .add_edge(
                    EdgeKind::Governs,
                    &rule.id,
                    &intent.id,
                    TruthClass::Asserted,
                )
                .unwrap();
            store
                .record_verdict(
                    &ge.id,
                    loom::model::InspectionStatus::Passing,
                    "quality fixture criterion",
                    "quality fixture evidence",
                    0.9,
                    "llm",
                )
                .unwrap();
        }
    }

    // Now elaborate should surface (both intents still have open scenarios
    // axes, and no other queue has work).
    let item = workitem::next(&store, None)
        .unwrap()
        .expect("contract 4: default next reaches elaborate once other queues are empty");
    assert_eq!(
        item.mode, "elaborate",
        "contract 4: default next reaches elaborate only after other queues drain"
    );
}

// ===========================================================================
// 5. PRESCREEN
// ===========================================================================

#[test]
fn prescreen_finds_secret_literal_with_iso5055_patterns_and_respects_cap() {
    // Contract 5: given a temp root with a file containing a secret-like
    // literal and the iso5055 secrets rule's patterns (read from
    // packs::pack("iso5055")), prescreen returns a hit with the right path+
    // line; cap is respected.
    let tmp = Tmp::new();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/auth.rs"),
        "fn main() {\n    let api_key = \"sk-live-abcdefghijklmnop\";\n}\n",
    )
    .unwrap();

    // Read the patterns from the iso5055 pack's secrets rule.
    let secrets_rule = packs::pack("iso5055")
        .iter()
        .find(|r| r.name == "iso5055-sec-no-hardcoded-secrets")
        .expect("contract 5: iso5055 pack contains the secrets rule");
    let patterns: Vec<String> = secrets_rule
        .patterns
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert!(
        !patterns.is_empty(),
        "contract 5: the iso5055 secrets rule carries non-empty patterns"
    );

    let hits = loom::prescan::prescreen(root, &["src/auth.rs".to_string()], &patterns, 20).unwrap();
    assert_eq!(
        hits.len(),
        1,
        "contract 5: prescreen returns one hit for the secret literal"
    );
    assert_eq!(
        hits[0].path, "src/auth.rs",
        "contract 5: hit path matches the scanned file"
    );
    assert_eq!(
        hits[0].line, 2,
        "contract 5: hit line number points at the secret literal"
    );
    assert!(
        !hits[0].pattern.is_empty(),
        "contract 5: hit records the matching pattern"
    );

    // Cap respected: with cap=0 nothing is returned; with two secret lines and
    // cap=1 exactly one hit is returned.
    std::fs::write(
        root.join("src/auth.rs"),
        "let api_key = \"sk-live-abcdefghijklmnop\";\nlet token = \"tok-live-abcdefghijklmnop\";\n",
    )
    .unwrap();
    let none = loom::prescan::prescreen(root, &["src/auth.rs".to_string()], &patterns, 0).unwrap();
    assert!(
        none.is_empty(),
        "contract 5: prescreen with cap=0 returns no hits"
    );
    let capped =
        loom::prescan::prescreen(root, &["src/auth.rs".to_string()], &patterns, 1).unwrap();
    assert_eq!(
        capped.len(),
        1,
        "contract 5: prescreen respects the cap when more hits exist"
    );
}

#[test]
fn quality_work_item_carries_pre_screened_hits_for_grounded_intent() {
    // Contract 5: a quality work item for that rule/intent pair carries
    // prompt_contract.pre_screened_hits non-empty (register the file as a
    // CodeFile + implements edge first).
    let tmp = Tmp::new();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/auth.rs"),
        "fn main() {\n    let api_key = \"sk-live-abcdefghijklmnop\";\n}\n",
    )
    .unwrap();

    let store = Store::init(root, Some("t"), false).unwrap();
    packs::seed(&store, "iso5055").unwrap();
    let rule = store
        .resolve_node(
            "iso5055-sec-no-hardcoded-secrets",
            Some(NodeType::QualityRule),
        )
        .unwrap();
    let intent = feature_intent(
        &store,
        "config loads secrets from env",
        Some("user_visible"),
    );
    // Register the CodeFile and ground the intent in it.
    let codefile = codefile(&store, "src/auth.rs");
    store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &codefile.id,
            TruthClass::Asserted,
        )
        .unwrap();
    // Create an uninspected governs edge from the secrets rule to this intent.
    // The quality queue serves uninspected governs edges before the fallback,
    // so the served item is for THIS rule and carries its pre_screened_hits.
    store
        .ensure_edge(EdgeKind::Governs, &rule.id, &intent.id)
        .unwrap();

    // The quality queue serves the uninspected governs edge, and the packet's
    // pre_screened_hits come from prescreen_for running the rule's patterns
    // over the intent's grounded file.
    let item = workitem::next(&store, Some(Lane::Quality))
        .unwrap()
        .expect("contract 5: quality queue serves the unmeasured rule×intent pair");
    assert_eq!(
        item.mode, "quality",
        "contract 5: quality item reports mode=quality"
    );
    assert!(
        !item.prompt_contract.pre_screened_hits.is_empty(),
        "contract 5: quality work item carries non-empty pre_screened_hits for a grounded intent whose file matches the rule patterns"
    );
    let hit = &item.prompt_contract.pre_screened_hits[0];
    assert_eq!(
        hit.path, "src/auth.rs",
        "contract 5: pre_screened_hit path is the grounded CodeFile"
    );
    assert_eq!(
        hit.line, 2,
        "contract 5: pre_screened_hit line points at the secret literal"
    );
}

#[test]
fn quality_packet_retains_twenty_one_matching_lines_under_canonical_cap() {
    let tmp = Tmp::new();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    let matching_lines = (0..21)
        .map(|_| "let api_key = \"sk-live-abcdefghijklmnop\";\n")
        .collect::<String>();
    std::fs::write(root.join("src/auth.rs"), matching_lines).unwrap();

    let store = Store::init(root, Some("t"), false).unwrap();
    packs::seed(&store, "iso5055").unwrap();
    let rule = store
        .resolve_node(
            "iso5055-sec-no-hardcoded-secrets",
            Some(NodeType::QualityRule),
        )
        .unwrap();
    let intent = feature_intent(&store, "many secret candidates", Some("user_visible"));
    let codefile = codefile(&store, "src/auth.rs");
    store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &codefile.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .ensure_edge(EdgeKind::Governs, &rule.id, &intent.id)
        .unwrap();

    let item = workitem::next(&store, Some(Lane::Quality))
        .unwrap()
        .expect("quality item");
    assert_eq!(
        item.prompt_contract.pre_screened_hits.len(),
        21,
        "quality packets retain all 21 matches below the canonical 200-hit cap"
    );
}

#[test]
fn quality_packet_reads_and_prescreens_every_grounded_file() {
    let tmp = Tmp::new();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    let store = Store::init(root, Some("t"), false).unwrap();
    packs::seed(&store, "iso5055").unwrap();
    let rule = store
        .resolve_node(
            "iso5055-sec-no-hardcoded-secrets",
            Some(NodeType::QualityRule),
        )
        .unwrap();
    let intent = feature_intent(&store, "nine-file behavior", Some("user_visible"));

    for index in 0..9 {
        let path = format!("src/file-{index}.rs");
        std::fs::write(root.join(&path), "fn safe() {}\n").unwrap();
        let codefile = store
            .add_node(NodeType::CodeFile, &path, "", "", serde_json::json!({}))
            .unwrap();
        store
            .add_edge(
                EdgeKind::Implements,
                &intent.id,
                &codefile.id,
                TruthClass::Asserted,
            )
            .unwrap();
    }
    let last_edge = store
        .realizing_groundings(&intent.id)
        .unwrap()
        .pop()
        .unwrap();
    let last_file = store.get_node(&last_edge.to_id).unwrap().unwrap().name;
    std::fs::write(
        root.join(&last_file),
        "let api_key = \"sk-live-abcdefghijklmnop\";\n",
    )
    .unwrap();
    store
        .ensure_edge(EdgeKind::Governs, &rule.id, &intent.id)
        .unwrap();

    let item = workitem::next(&store, Some(Lane::Quality))
        .unwrap()
        .expect("quality item");
    assert_eq!(item.context.read_set.len(), 9);
    assert!(
        item.prompt_contract
            .pre_screened_hits
            .iter()
            .any(|hit| hit.path == last_file),
        "the file beyond the old eight-file cap must be pre-screened"
    );
}

// ===========================================================================
// 6. SCAN ADAPTERS
// ===========================================================================

#[test]
fn add_adapter_rejects_duplicate_names_and_bad_map_regex() {
    // Contract 6: add_adapter rejects duplicate names and a map regex without
    // named file/line groups.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    scan::add_adapter(
        &store,
        "fake",
        "printf 'src/a.rs:1: boom\\n'",
        None,
        scan::ScanFormat::Lines,
    )
    .unwrap();

    let dup = scan::add_adapter(
        &store,
        "fake",
        "printf 'src/a.rs:1: boom\\n'",
        None,
        scan::ScanFormat::Lines,
    );
    assert!(
        dup.is_err(),
        "contract 6: add_adapter rejects a duplicate adapter name"
    );

    // A map regex missing the `file` named group.
    let bad_map = scan::add_adapter(
        &store,
        "other",
        "printf 'src/a.rs:1: boom\\n'",
        Some(r"^(?P<line>\d+):\s*(?P<msg>.+)$"),
        scan::ScanFormat::Lines,
    );
    assert!(
        bad_map.is_err(),
        "contract 6: add_adapter rejects a map regex missing the named 'file' group"
    );

    // A map regex missing the `line` named group.
    let bad_line = scan::add_adapter(
        &store,
        "other",
        "printf 'src/a.rs:1: boom\\n'",
        Some(r"^(?P<file>[^:]+):\s*(?P<msg>.+)$"),
        scan::ScanFormat::Lines,
    );
    assert!(
        bad_line.is_err(),
        "contract 6: add_adapter rejects a map regex missing the named 'line' group"
    );
}

#[test]
fn scan_run_with_fake_adapter_creates_visible_finding_and_resolves_on_empty_rerun() {
    // Contract 6: run with a fake printf-style adapter creates a finding visible
    // via findings_view; re-run with empty output resolves it.
    let tmp = Tmp::new();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn demo() {}\n").unwrap();
    let store = Store::init(root, Some("t"), false).unwrap();
    codefile(&store, "src/lib.rs");
    scan::add_adapter(
        &store,
        "fake",
        "printf 'src/lib.rs:1: boom\\n'",
        None,
        scan::ScanFormat::Lines,
    )
    .unwrap();

    let first = scan::run(&store, root, Some("fake")).unwrap();
    assert_eq!(
        first.diagnostics, 1,
        "contract 6: first scan run reports one diagnostic"
    );
    assert_eq!(
        first.new_findings, 1,
        "contract 6: first scan run creates one new finding"
    );
    let findings = loom::signal::findings_view(&store).unwrap();
    assert_eq!(
        findings.len(),
        1,
        "contract 6: the adapter finding is visible via findings_view"
    );
    assert_eq!(
        findings[0].state, "untriaged",
        "contract 6: a fresh adapter finding is untriaged"
    );
    assert!(
        findings[0].node.name.contains("src/lib.rs:1 boom"),
        "contract 6: adapter finding name carries the path, line, and message"
    );

    // Re-run with empty output: the finding is resolved (removed).
    scan::remove_adapter(&store, "fake").unwrap();
    scan::add_adapter(&store, "fake", "printf ''", None, scan::ScanFormat::Lines).unwrap();
    let second = scan::run(&store, root, Some("fake")).unwrap();
    assert_eq!(
        second.diagnostics, 0,
        "contract 6: empty-output re-run reports no diagnostics"
    );
    assert_eq!(
        second.resolved_findings, 1,
        "contract 6: empty-output re-run resolves the prior finding"
    );
    let after = loom::signal::findings_view(&store).unwrap();
    assert!(
        after.is_empty(),
        "contract 6: no findings remain after the empty-output re-run"
    );
}

#[test]
fn scan_adapter_config_appears_in_snapshot_and_survives_export_round_trip() {
    // Contract 6: adapter config appears in Store::snapshot().config under
    // scan_adapters, and survives an Export::from_snapshot → into_snapshot
    // round trip.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    scan::add_adapter(
        &store,
        "fake",
        "printf 'src/lib.rs:1: boom\\n'",
        None,
        scan::ScanFormat::Lines,
    )
    .unwrap();

    let snap = store.snapshot().unwrap();
    let cfg = snap
        .config
        .get("scan_adapters")
        .expect("contract 6: snapshot().config contains the scan_adapters key");
    assert!(
        cfg.contains("fake"),
        "contract 6: scan_adapters config records the registered adapter name"
    );

    // Round trip through Export.
    let export = Export::from_snapshot(snap);
    let json = export.to_json().unwrap();
    assert!(
        json.contains("scan_adapters"),
        "contract 6: export JSON carries the scan_adapters config"
    );
    let reparsed = Export::from_json(&json).unwrap();
    let restored_snap = reparsed.into_snapshot();
    let restored_cfg = restored_snap
        .config
        .get("scan_adapters")
        .expect("contract 6: scan_adapters config survives the export round trip");
    assert!(
        restored_cfg.contains("fake"),
        "contract 6: the round-tripped scan_adapters config still records the adapter"
    );
}

// ===========================================================================
// 7. CONFIG TRAVEL
// ===========================================================================

#[test]
fn layer_order_set_via_meta_appears_in_snapshot_and_survives_restore() {
    // Contract 7: layer_order set via meta appears in snapshot().config and,
    // after restore() into a fresh store, get_meta("layer_order") returns it.
    let tmp = Tmp::new();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let order = r#"["presentation","domain","storage"]"#;
    store.set_meta("layer_order", order).unwrap();

    let snap = store.snapshot().unwrap();
    let cfg = snap
        .config
        .get("layer_order")
        .expect("contract 7: snapshot().config contains layer_order");
    assert_eq!(
        cfg, order,
        "contract 7: snapshot().config layer_order matches the meta value"
    );

    // Restore into a fresh store and read it back.
    let tmp2 = Tmp::new();
    let mut store2 = Store::init(tmp2.path(), Some("t2"), false).unwrap();
    store2.restore(&snap).unwrap();
    let restored = store2
        .get_meta("layer_order")
        .unwrap()
        .expect("contract 7: layer_order is readable after restore into a fresh store");
    assert_eq!(
        restored, order,
        "contract 7: get_meta(\"layer_order\") returns the traveled value after restore"
    );
}

/// **The pre-screen reads code, not prose and not tests.**
///
/// A rule's patterns are regexes over raw lines, so without filtering the
/// scanner reports a module's own DOCUMENTATION as a violation of the rule that
/// documentation describes — loom's doc comment explaining that "a test SHOULD
/// `.unwrap()`" was itself reported as an unchecked failure, and doc comments
/// mentioning DELETE or UPDATE were reported as SQL injection.
///
/// `#[cfg(test)]` modules are the same mistake one level down from scanning
/// whole test files: `src/store/mod.rs` alone contributed 40 hits, every one
/// inside `#[cfg(test)]`.
#[test]
fn the_prescreen_skips_comments_and_cfg_test_modules() {
    let tmp = Tmp::new();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/sample.rs"),
        r#"//! A doc comment that mentions .unwrap() in prose.
/// Another one describing DELETE FROM {table} interpolation.
pub fn real() -> u8 {
    Some(1).expect("this one is real code")
}
// A line comment with .unwrap() in it.
#[cfg(test)]
mod tests {
    #[test]
    fn t() {
        let v: Option<u8> = Some(1);
        assert_eq!(v.unwrap(), 1);
    }
}
"#,
    )
    .unwrap();

    let patterns = vec![
        r#"\bunwrap\(\)"#.to_string(),
        r#"\bexpect\s*\("#.to_string(),
    ];
    let hits =
        loom::prescan::prescreen(root, &["src/sample.rs".to_string()], &patterns, 50).unwrap();

    assert_eq!(
        hits.len(),
        1,
        "only the one real production call is a hit: {hits:?}"
    );
    assert_eq!(hits[0].line, 4, "the expect() inside fn real: {hits:?}");
    assert!(
        hits[0].excerpt.contains("this one is real code"),
        "and it is the production line, not prose or a test: {hits:?}"
    );
}

/// A `}` closing the test module must hand scanning back to production code —
/// otherwise everything after the first test module goes unscanned, which
/// would turn a false-positive fix into a false-negative one.
#[test]
fn production_code_after_a_test_module_is_still_scanned() {
    let tmp = Tmp::new();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/after.rs"),
        r#"#[cfg(test)]
mod tests {
    #[test]
    fn t() {
        let v: Option<u8> = Some(1);
        assert_eq!(v.unwrap(), 1);
    }
}

pub fn after_the_tests() -> u8 {
    Some(7).expect("production again")
}
"#,
    )
    .unwrap();

    let patterns = vec![
        r#"\bunwrap\(\)"#.to_string(),
        r#"\bexpect\s*\("#.to_string(),
    ];
    let hits =
        loom::prescan::prescreen(root, &["src/after.rs".to_string()], &patterns, 50).unwrap();

    assert_eq!(
        hits.len(),
        1,
        "the post-test production line is a hit: {hits:?}"
    );
    assert!(
        hits[0].excerpt.contains("production again"),
        "scanning resumed after the test module closed: {hits:?}"
    );
}
