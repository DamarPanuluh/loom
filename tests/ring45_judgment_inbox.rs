//! Ring 45 — judgments get an LLM-proposal inbox.
//!
//! Ratify and reject are correctly human-gated (INV-8), but LLM-discovered
//! candidates for them used to have no queue: junk intents sat in work
//! queues indefinitely and ratification arrived as an undifferentiated pile
//! at a terminal. Now the LLM stages proposals with evidence and confirms
//! each through the same authority gate as the direct command: human authority
//! for ratify/reject, builder ownership for redefine. The inbox changes where
//! candidates wait, not who may apply each judgment kind.

use loom::model::NodeType;
use loom::store::Store;
mod common;
use common::*;

fn loom(tmp: &std::path::Path, args: &[&str], envs: &[(&str, &str)]) -> std::process::Output {
    let mut cmd = std::process::Command::new(std::path::PathBuf::from(env!("CARGO_BIN_EXE_loom")));
    cmd.arg("--graph").arg(tmp).args(args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output()
        .unwrap_or_else(|e| panic!("spawn loom {args:?}: {e}"))
}

fn loom_json(tmp: &std::path::Path, args: &[&str], envs: &[(&str, &str)]) -> serde_json::Value {
    let mut full: Vec<&str> = args.to_vec();
    full.push("--json");
    let out = loom(tmp, &full, envs);
    assert!(
        out.status.success(),
        "loom {args:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .unwrap_or_else(|e| panic!("no json: {e}"))
}

fn loom_fail(tmp: &std::path::Path, args: &[&str], envs: &[(&str, &str)]) -> String {
    let out = loom(tmp, args, envs);
    assert!(
        !out.status.success(),
        "loom {args:?} unexpectedly succeeded:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    String::from_utf8_lossy(&out.stderr).to_string()
}

const BUILDER: &[(&str, &str)] = &[("LOOM_AGENT", "llm:builder")];
/// A builder lane relaying the human's VERIFIABLE answer: the host confirmed
/// presence (debug seam) and the exact response is being recorded. Without
/// this, an llm lane cannot pass --human-decision at all (INV-8).
const RELAY: &[(&str, &str)] = &[
    ("LOOM_AGENT", "llm:builder"),
    ("LOOM_PRESENCE_PROBE", "human"),
];
const ANALYZER: &[(&str, &str)] = &[("LOOM_AGENT", "llm:analyzer")];

/// An unratified intent; returns its id. The store is scoped so the graph
/// lock is free before any CLI subprocess runs.
fn intent(tmp: &Tmp, name: &str, description: &str) -> String {
    Store::init(tmp.path(), Some("t"), false).ok(); // idempotent-ish: ignore "exists"
    let store = Store::open(tmp.path()).unwrap();
    let n = store
        .add_node(
            NodeType::Intent,
            name,
            description,
            "implemented",
            serde_json::json!({}),
        )
        .unwrap();
    n.id
}

fn staged(tmp: &std::path::Path) -> Vec<serde_json::Value> {
    loom_json(tmp, &["judgment", "digest"], &[])["proposals"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The typed judgment values keep the existing lowercase SQLite and JSON
/// spellings. The concrete enum types are inferred through the proposal's
/// public fields because the store module intentionally exposes proposals as
/// its API surface.
#[test]
fn judgment_enums_parse_serialize_and_roundtrip_through_the_db() {
    let tmp = Tmp::new();
    let id = intent(&tmp, "typed judgment", "exercise typed persistence");
    let out = loom_json(
        tmp.path(),
        &[
            "judgment",
            "propose",
            "redefine",
            &id,
            "--evidence",
            "the accepted behavior now has a narrower contract",
            "--description",
            "exercise typed persistence with the narrower contract",
        ],
        BUILDER,
    );
    assert_eq!(out["proposal"]["kind"], "redefine");
    assert_eq!(out["proposal"]["state"], "staged");

    let store = Store::open(tmp.path()).unwrap();
    let proposal = store
        .get_judgment(out["proposal"]["id"].as_str().unwrap())
        .unwrap()
        .unwrap();

    let parsed_kinds: std::result::Result<Vec<_>, anyhow::Error> = ["ratify", "reject", "redefine"]
        .into_iter()
        .map(str::parse)
        .collect();
    let parsed_states: std::result::Result<Vec<_>, anyhow::Error> =
        ["staged", "confirmed", "withdrawn"]
            .into_iter()
            .map(str::parse)
            .collect();
    let parsed_kinds = parsed_kinds.unwrap();
    let parsed_states = parsed_states.unwrap();
    assert_eq!(proposal.kind, parsed_kinds[2]);
    assert_eq!(proposal.state, parsed_states[0]);
    assert_eq!(
        parsed_kinds
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["ratify", "reject", "redefine"]
    );
    assert_eq!(
        parsed_states
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["staged", "confirmed", "withdrawn"]
    );
    assert_eq!(proposal.kind.as_str(), "redefine");
    assert_eq!(proposal.state.as_str(), "staged");
    assert_eq!(proposal.kind.to_string(), "redefine");
    assert_eq!(proposal.state.to_string(), "staged");
    assert_eq!(
        serde_json::to_value(&parsed_kinds).unwrap(),
        serde_json::json!(["ratify", "reject", "redefine"])
    );
    assert_eq!(
        serde_json::to_value(&parsed_states).unwrap(),
        serde_json::json!(["staged", "confirmed", "withdrawn"])
    );
    let serde_kinds: Vec<loom::store::JudgmentKind> =
        serde_json::from_value(serde_json::json!(["ratify", "reject", "redefine"])).unwrap();
    let serde_states: Vec<loom::store::JudgmentState> =
        serde_json::from_value(serde_json::json!(["staged", "confirmed", "withdrawn"])).unwrap();
    assert_eq!(parsed_kinds, serde_kinds);
    assert_eq!(parsed_states, serde_states);

    store
        .decide_judgment(&proposal.id, "confirmed".parse().unwrap())
        .unwrap();
    let confirmed = store.get_judgment(&proposal.id).unwrap().unwrap();
    assert_eq!(confirmed.state.as_str(), "confirmed");
    assert_eq!(
        serde_json::to_value(confirmed).unwrap()["state"],
        "confirmed"
    );

    assert!("Ratify".parse::<loom::store::JudgmentKind>().is_err());
    assert!("confirmed_now"
        .parse::<loom::store::JudgmentState>()
        .is_err());
}

/// The core loop: the LLM stages a reject with evidence; the human gate
/// still refuses the machine acting alone; with the human's exact answer
/// the SAME gated write lands — and the inbox empties.
#[test]
fn a_reject_proposal_confirms_only_through_the_human_gate() {
    let tmp = Tmp::new();
    let id = intent(&tmp, "legacy cache bypass", "skips the cache for speed");

    let staged_out = loom_json(
        tmp.path(),
        &[
            "judgment",
            "propose",
            "reject",
            &id,
            "--evidence",
            "bypasses every cache invariant; two incidents trace here",
        ],
        BUILDER,
    );
    let pid = staged_out["proposal"]["id"].as_str().unwrap().to_string();
    let pid8 = &pid[..8];

    // The machine alone cannot confirm: INV-8 unchanged by the inbox.
    let err = loom_fail(tmp.path(), &["judgment", "confirm", pid8], BUILDER);
    assert!(
        err.contains("INV-8"),
        "the gate is the direct command's: {err}"
    );

    // With the human's exact answer the SAME write lands: rejected, retired,
    // removal work minted.
    let out = loom_json(
        tmp.path(),
        &[
            "judgment",
            "confirm",
            pid8,
            "--human-decision",
            "yes, remove the bypass",
        ],
        RELAY,
    );
    assert_eq!(out["proposal"]["state"].as_str().unwrap(), "confirmed");
    assert!(
        out["outcome"]["rejected"]["id"].as_str().is_some(),
        "the reject write executed: {out}"
    );
    let store = Store::open(tmp.path()).unwrap();
    let n = store.get_node(&id).unwrap().unwrap();
    assert_eq!(n.status, "deprecated", "rejection retires the intent");
    drop(store);

    // The digest no longer serves it, and a double-confirm cannot replay
    // the gated write under a second decision.
    assert!(staged(tmp.path()).is_empty(), "inbox drained");
    let err = loom_fail(tmp.path(), &["judgment", "confirm", pid8], BUILDER);
    assert!(
        err.contains("not staged") || err.contains("confirmed"),
        "{err}"
    );
}

/// A ratify proposal confirms through the identical mediation path as
/// `intent ratify` — the verdict the inbox produces is indistinguishable
/// from the direct command's.
#[test]
fn a_ratify_proposal_lands_the_same_ratification_as_the_direct_command() {
    let tmp = Tmp::new();
    let id = intent(
        &tmp,
        "session tokens expire",
        "tokens age out after an hour",
    );

    loom_json(
        tmp.path(),
        &[
            "judgment",
            "propose",
            "ratify",
            &id,
            "--evidence",
            "product asked for bounded sessions in the security review",
        ],
        BUILDER,
    );
    let pid8 = staged(tmp.path())[0]["id8"].as_str().unwrap().to_string();

    loom_json(
        tmp.path(),
        &[
            "judgment",
            "confirm",
            &pid8,
            "--human-decision",
            "ratify it",
        ],
        RELAY,
    );
    let store = Store::open(tmp.path()).unwrap();
    assert_eq!(
        store.ratification(&id).unwrap(),
        "ratified",
        "the gated ratification fact landed"
    );
}

/// A redefine proposal carries its replacement statement; only the builder
/// owner may confirm it, and confirmation applies it WITH the ripple — a
/// ratified intent's wantedness rots with its meaning, exactly as a direct
/// `intent update --description` does.
#[test]
fn a_redefine_proposal_preserves_the_builder_gate_and_applies_the_ripple() {
    let tmp = Tmp::new();
    let id = intent(&tmp, "export reports", "users export CSV reports");
    // Ratified first, so the ripple has something to stale.
    loom_json(
        tmp.path(),
        &[
            "intent",
            "ratify",
            &id,
            "--evidence",
            "requested in Q1 review",
            "--human-decision",
            "wanted",
        ],
        RELAY,
    );

    let err = loom_fail(
        tmp.path(),
        &[
            "judgment",
            "propose",
            "redefine",
            &id,
            "--evidence",
            "scope changed",
        ],
        BUILDER,
    );
    assert!(
        err.contains("--description"),
        "a redefine without a replacement statement is refused: {err}"
    );

    loom_json(
        tmp.path(),
        &[
            "judgment",
            "propose",
            "redefine",
            &id,
            "--evidence",
            "PDF replaced CSV in the finalized spec",
            "--description",
            "users export PDF reports",
        ],
        BUILDER,
    );
    let pid8 = staged(tmp.path())[0]["id8"].as_str().unwrap().to_string();

    // Redefinition is not human-only, but it remains a builder-owned write.
    let err = loom_fail(tmp.path(), &["judgment", "confirm", &pid8], ANALYZER);
    assert!(err.contains("lane gate"), "analyzer must be refused: {err}");
    assert!(err.contains("builder"), "the owning lane is named: {err}");
    assert_eq!(
        staged(tmp.path()).len(),
        1,
        "a refused confirm leaves the proposal staged"
    );
    {
        let store = Store::open(tmp.path()).unwrap();
        assert_eq!(
            store.get_node(&id).unwrap().unwrap().description,
            "users export CSV reports",
            "the refused lane moved nothing"
        );
    }

    let out = loom(tmp.path(), &["judgment", "confirm", &pid8], BUILDER);
    assert!(
        out.status.success(),
        "builder confirmation failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("builder-owner gate"), "{text}");
    assert!(
        !text.contains("human gate"),
        "redefine output must not claim human authorization: {text}"
    );

    let store = Store::open(tmp.path()).unwrap();
    let n = store.get_node(&id).unwrap().unwrap();
    assert_eq!(n.description, "users export PDF reports");
    assert_eq!(
        store.ratification(&id).unwrap(),
        "needs_reconfirmation",
        "wantedness staled with meaning — the direct command's ripple"
    );
}

/// One live proposal per (kind, intent): the pile is what the inbox exists
/// to end, so a duplicate stage points at the existing entry instead.
#[test]
fn a_duplicate_stage_is_refused_with_the_existing_entry_named() {
    let tmp = Tmp::new();
    let id = intent(&tmp, "dup target", "d");
    loom_json(
        tmp.path(),
        &[
            "judgment",
            "propose",
            "ratify",
            &id,
            "--evidence",
            "wanted for reason one",
        ],
        BUILDER,
    );
    let pid8 = staged(tmp.path())[0]["id8"].as_str().unwrap().to_string();
    let err = loom_fail(
        tmp.path(),
        &[
            "judgment",
            "propose",
            "ratify",
            &id,
            "--evidence",
            "wanted for reason two",
        ],
        BUILDER,
    );
    assert!(err.contains("already staged"), "says why: {err}");
    assert!(err.contains(&pid8), "names the live entry: {err}");
    assert_eq!(staged(tmp.path()).len(), 1, "still one inbox entry");
}

/// Withdrawal is the exit for a wrong candidate — reasoned, journaled, and
/// gone from the digest.
#[test]
fn a_staged_proposal_withdraws_with_a_reason() {
    let tmp = Tmp::new();
    let id = intent(&tmp, "withdraw me", "d");
    loom_json(
        tmp.path(),
        &[
            "judgment",
            "propose",
            "reject",
            &id,
            "--evidence",
            "looked junk on first pass",
        ],
        BUILDER,
    );
    let pid8 = staged(tmp.path())[0]["id8"].as_str().unwrap().to_string();

    let err = loom_fail(
        tmp.path(),
        &["judgment", "withdraw", &pid8, "--reason", "n/a"],
        BUILDER,
    );
    assert!(err.contains("substantively"), "a shrug is no reason: {err}");

    loom_json(
        tmp.path(),
        &[
            "judgment",
            "withdraw",
            &pid8,
            "--reason",
            "second look: the behavior is load-bearing for imports",
        ],
        BUILDER,
    );
    assert!(staged(tmp.path()).is_empty());
    let err = loom_fail(tmp.path(), &["judgment", "confirm", &pid8], BUILDER);
    assert!(err.contains("withdrawn"), "{err}");
}

/// The digest is the review surface: each staged entry names its intent, its
/// evidence, its proposer, and the exact confirm command. Redefine's command
/// does not falsely request a human decision.
#[test]
fn the_digest_renders_each_proposal_with_its_confirm_command() {
    let tmp = Tmp::new();
    let a = intent(&tmp, "first candidate", "d");
    let b = intent(&tmp, "second candidate", "d");
    loom_json(
        tmp.path(),
        &[
            "judgment",
            "propose",
            "ratify",
            &a,
            "--evidence",
            "asked for in review",
        ],
        BUILDER,
    );
    loom_json(
        tmp.path(),
        &[
            "judgment",
            "propose",
            "reject",
            &b,
            "--evidence",
            "dead since the migration",
        ],
        BUILDER,
    );

    let c = intent(&tmp, "third candidate", "d");
    loom_json(
        tmp.path(),
        &[
            "judgment",
            "propose",
            "redefine",
            &c,
            "--evidence",
            "the implementation and accepted contract now use the new name",
            "--description",
            "renamed accepted behavior",
        ],
        BUILDER,
    );

    let out = loom(tmp.path(), &["judgment", "digest"], &[]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("ratify 'first candidate'"), "{text}");
    assert!(text.contains("reject 'second candidate'"), "{text}");
    assert!(
        text.contains("asked for in review"),
        "evidence shown: {text}"
    );
    assert!(text.contains("llm:builder"), "proposer shown: {text}");
    assert!(text.contains("redefine 'third candidate'"), "{text}");
    assert!(
        text.lines()
            .skip_while(|line| !line.contains("redefine 'third candidate'"))
            .take(6)
            .any(|line| line
                .trim_start()
                .starts_with("confirm: loom judgment confirm")
                && !line.contains("--human-decision")),
        "redefine confirmation must not advertise a human gate: {text}"
    );
    assert!(
        text.contains("loom judgment confirm"),
        "the exact confirm command is printed: {text}"
    );

    let json = loom_json(tmp.path(), &["judgment", "digest"], &[]);
    assert_eq!(json["staged"].as_u64().unwrap(), 3);
    // Proposing for an intent that no longer needs the judgment is refused.
    loom_json(
        tmp.path(),
        &[
            "intent",
            "ratify",
            &a,
            "--evidence",
            "direct",
            "--human-decision",
            "yes",
        ],
        RELAY,
    );
    let err = loom_fail(
        tmp.path(),
        &["judgment", "propose", "ratify", &a, "--evidence", "again"],
        BUILDER,
    );
    assert!(err.contains("already ratified"), "{err}");
}
