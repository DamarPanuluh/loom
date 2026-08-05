//! Ring 39 — hit-level adjudication keyed by matched-content hash.
//!
//! A pattern pre-screen re-surfaces the same false positive on every
//! rule×intent pair, every re-measure, with shifted line numbers. The judgment
//! "not what the rule means" is recorded once against the matched TEXT's
//! content hash: it answers the same text wherever it moves, and it stops
//! applying the moment the text changes.

use loom::model::{Claim, EdgeKind, NodeType, TargetKind, TruthClass};
use loom::store::{Assertion, Store, Subject};
mod common;
use common::*;

/// Intent grounded in a file with two `expect(` hits (lines 2 and 5), governed
/// by a rule carrying the no-unchecked-failure pattern.
fn scannable(tmp: &Tmp) -> (Store, String) {
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(
        tmp.path().join("src/thing.rs"),
        "pub fn a() -> u8 {\n    Some(1).expect(\"a is total\")\n}\npub fn b() -> u8 {\n    Some(2).expect(\"b is total\")\n}\n",
    )
    .unwrap();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "a behavior under a rule",
            "d",
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    let cf = codefile(&store, "src/thing.rs");
    let g = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &cf.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .set_facet(
            &g.id,
            TargetKind::Edge,
            "locator",
            "fn a",
            TruthClass::Asserted,
        )
        .unwrap();
    let rule = store
        .add_node(
            NodeType::QualityRule,
            "no-unchecked-failure",
            "every fallible operation's failure path is handled",
            "",
            serde_json::json!({"category":"reliability","patterns":[r#"\bexpect\s*\("#]}),
        )
        .unwrap();
    let gov = store
        .add_edge(
            EdgeKind::Governs,
            &rule.id,
            &intent.id,
            TruthClass::Asserted,
        )
        .unwrap();
    (store, gov.id)
}

fn passing(store: &Store, edge: &str, evidence: &str) -> loom::Result<()> {
    store
        .assert_fact(
            Assertion::new(
                Subject::Edge(edge.to_string()),
                Claim::Verdict,
                "passing",
                "llm",
            )
            .criterion("measured the rule against the realizing code")
            .confidence(0.9)
            .cited(loom::evidence::cite(store.root(), evidence)?),
        )
        .map(|_| ())
}

const HIT_A: &str = r#"Some(1).expect("a is total")"#;
const HIT_B: &str = r#"Some(2).expect("b is total")"#;

#[test]
fn a_suppressed_hit_answers_the_gate_on_every_future_verdict() {
    let tmp = Tmp::new();
    let (store, gov) = scannable(&tmp);

    let err = passing(&store, &gov, "nothing to worry about")
        .expect_err("unanswered hits refuse a passing verdict");
    assert!(err
        .to_string()
        .contains("2 hit(s) this verdict does not answer"));

    store
        .suppress_hit(
            "no-unchecked-failure",
            HIT_A,
            "expect on a literal Some; total by construction",
        )
        .unwrap();

    let err =
        passing(&store, &gov, "nothing to worry about").expect_err("one hit still unanswered");
    let msg = err.to_string();
    assert!(
        msg.contains("1 hit(s) this verdict does not answer"),
        "{msg}"
    );
    assert!(
        msg.contains("1 hit(s) already answered (1 suppressed)"),
        "{msg}"
    );

    store
        .suppress_hit(
            "no-unchecked-failure",
            HIT_B,
            "same: literal Some cannot be None",
        )
        .unwrap();
    passing(&store, &gov, "both hits adjudicated false positives")
        .expect("every hit answered by adjudication: the verdict stands");
}

#[test]
fn the_judgment_follows_the_text_when_the_line_moves() {
    let tmp = Tmp::new();
    let (store, gov) = scannable(&tmp);
    store
        .suppress_hit("no-unchecked-failure", HIT_A, "total by construction")
        .unwrap();

    // Insert ten lines above: the hit is now on line 12, same text.
    std::fs::write(
        tmp.path().join("src/thing.rs"),
        "// ten\n// nine\n// eight\n// seven\n// six\n// five\n// four\n// three\n// two\n// one\npub fn a() -> u8 {\n    Some(1).expect(\"a is total\")\n}\npub fn b() -> u8 {\n    Some(2).expect(\"b is total\")\n}\n",
    )
    .unwrap();

    let err = passing(&store, &gov, "moved, unchanged")
        .expect_err("only the still-unadjudicated hit may remain");
    let msg = err.to_string();
    assert!(
        msg.contains("1 hit(s) this verdict does not answer"),
        "{msg}"
    );
    assert!(
        msg.contains("src/thing.rs:15"),
        "the unsuppressed hit moved too: {msg}"
    );
    assert!(msg.contains("(1 suppressed)"), "{msg}");
}

#[test]
fn a_changed_text_is_a_new_hit_the_judgment_does_not_reach() {
    let tmp = Tmp::new();
    let (store, gov) = scannable(&tmp);
    store
        .suppress_hit("no-unchecked-failure", HIT_A, "total by construction")
        .unwrap();

    // The matched text itself changed: the suppression must stop applying.
    std::fs::write(
        tmp.path().join("src/thing.rs"),
        "pub fn a() -> u8 {\n    Some(1).expect(\"a is still total, promise\")\n}\npub fn b() -> u8 {\n    Some(2).expect(\"b is total\")\n}\n",
    )
    .unwrap();

    let err = passing(&store, &gov, "same code, honest")
        .expect_err("changed matched text invalidates the suppression");
    assert!(
        err.to_string()
            .contains("2 hit(s) this verdict does not answer"),
        "both hits are open again: {err}"
    );
}

#[test]
fn unsuppress_reopens_the_hit() {
    let tmp = Tmp::new();
    let (store, gov) = scannable(&tmp);
    let row = store
        .suppress_hit("no-unchecked-failure", HIT_A, "total by construction")
        .unwrap();
    assert!(store
        .is_hit_suppressed("no-unchecked-failure", HIT_A)
        .unwrap());

    let withdrawn = store
        .unsuppress_hit("no-unchecked-failure", &row.content_hash[..8])
        .expect("a hash prefix resolves the suppression");
    assert_eq!(withdrawn.content_hash, row.content_hash);
    assert!(!store
        .is_hit_suppressed("no-unchecked-failure", HIT_A)
        .unwrap());

    let err = passing(&store, &gov, "withdrawing must re-open")
        .expect_err("the re-opened hit refuses again");
    assert!(err
        .to_string()
        .contains("2 hit(s) this verdict does not answer"));
}

#[test]
fn suppressions_are_a_reviewable_ledger() {
    let tmp = Tmp::new();
    let (store, _) = scannable(&tmp);
    store
        .suppress_hit("no-unchecked-failure", HIT_A, "total by construction")
        .unwrap();

    let rows = store
        .hit_adjudications(Some("no-unchecked-failure"))
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].excerpt, HIT_A);
    assert_eq!(rows[0].reason, "total by construction");
    assert!(!rows[0].content_hash.is_empty());
    assert!(store
        .hit_adjudications(Some("another-rule"))
        .unwrap()
        .is_empty());

    // A re-judgment of the same text is an explicit withdraw-then-judge, never
    // a silent overwrite; and a reasonless suppression is refused.
    assert!(store
        .suppress_hit("no-unchecked-failure", HIT_A, "a different reason")
        .is_err());
    assert!(store
        .suppress_hit("no-unchecked-failure", HIT_B, "   ")
        .is_err());
}
