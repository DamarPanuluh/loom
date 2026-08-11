//! Ring 52 — host-mediated Journey gates are hash-bound, mutation-confined,
//! and one-shot without ever manufacturing a human answer.

mod common;

use clap::Parser;
use common::Tmp;
use loom::cli::{Cli, Command, JourneyCmd};
use loom::journey_gate::{
    digest_token, CapsuleStore, GateBinding, GateSubject, HumanOption, HumanPrompt, ResumeAnswer,
    AUTHORITY_RECEIPT_SCHEMA, CONTINUATION_CAPSULE_SCHEMA, PENDING_HUMAN_SCHEMA,
};
use loom::ratification::HumanDecision;
use std::sync::{Arc, Barrier};

fn prompt() -> HumanPrompt {
    HumanPrompt::new(
        "  Should the changed export behavior remain wanted?  ",
        "  Recommend one option from the concrete drift evidence; the recommendation is not the decision.  ",
        vec![
            HumanOption::new(
                "keep",
                "  Keep behavior  ",
                "Retain the current criterion as wanted.",
                false,
            ),
            HumanOption::new(
                "remove",
                "Remove behavior",
                "Reject the current criterion as unwanted.",
                false,
            ),
            HumanOption::new(
                "revise",
                "Revise criterion",
                "Supply the corrected behavior before recording the decision.",
                true,
            ),
        ],
    )
    .unwrap()
}

fn binding(prompt: &HumanPrompt) -> GateBinding {
    GateBinding {
        journey_id: "human-asked".into(),
        profile: "proof".into(),
        journey_hash: "3ce94e731a0791e0".into(),
        surface_hash: "0123456789abcdef".into(),
        step_id: "record-human-choice".into(),
        step_index: 1,
        subject: GateSubject {
            kind: "intent".into(),
            id: "28441d7d9c46d7759d7c34ec50136998".into(),
            hash: "e6a21ffe9dec5760".into(),
        },
        prompt_hash: prompt.digest().unwrap(),
    }
}

fn keep_answer() -> ResumeAnswer {
    ResumeAnswer {
        choice_id: "keep".into(),
        human_decision: "Keep behavior — the signed export remains required".into(),
        free_form: None,
    }
}

#[test]
fn pending_gate_is_opaque_normalized_and_contains_no_canned_answer() {
    let tmp = Tmp::new();
    let runtime_temp = tmp.path().join("runtime-temp");
    let store = CapsuleStore::new(&runtime_temp).unwrap();
    let prompt = prompt();
    let issued = store.issue(binding(&prompt), prompt).unwrap();

    assert_eq!(issued.pending.schema, PENDING_HUMAN_SCHEMA);
    assert_eq!(issued.pending.status, "pending_human");
    assert!(!issued.pending.human_terminal_required);
    assert_eq!(issued.pending.options.len(), 3);
    assert_eq!(issued.pending.options[0].label, "Keep behavior");
    assert!(issued.pending.options[2].free_form);

    let token = &issued.pending.resume_token;
    assert!(token.starts_with("jgt1_"));
    for binding_text in [
        issued.pending.binding.journey_id.as_str(),
        issued.pending.binding.journey_hash.as_str(),
        issued.pending.binding.surface_hash.as_str(),
        issued.pending.binding.step_id.as_str(),
        issued.pending.binding.subject.id.as_str(),
    ] {
        assert!(
            !token.contains(binding_text),
            "opaque token disclosed bound data: {token}"
        );
    }
    let digest = digest_token(token).unwrap();
    assert_eq!(digest.len(), 64);
    assert_ne!(digest, *token);
    assert_eq!(
        digest_token(&format!("jgt1_{}", "0".repeat(64))).unwrap(),
        "55079e00f819c82de1ee55fc099e2b434d78a94a5e9f63ad80044c8fcfc45630",
        "token digests use stable SHA-256"
    );

    let pending_json = serde_json::to_value(&issued.pending).unwrap();
    let pending_object = pending_json.as_object().unwrap();
    for forbidden in ["argv", "write_back", "default", "human_decision"] {
        assert!(
            !pending_object.contains_key(forbidden),
            "pending gate exposed forbidden field {forbidden}: {pending_json}"
        );
    }
    let serialized = serde_json::to_string(&pending_json).unwrap();
    assert!(!serialized.contains("<answer>"));
    assert!(!serialized.contains("--human-decision"));

    let capsule = std::fs::read_to_string(&issued.paths.capsule).unwrap();
    assert!(capsule.contains(CONTINUATION_CAPSULE_SCHEMA));
    assert!(capsule.contains(&digest));
    assert!(
        !capsule.contains(token),
        "raw token reached capsule storage"
    );
    assert!(capsule.contains(r#""workspace":"workspace""#));
    assert!(capsule.contains(r#""runtime_state":"runtime-state.json""#));
}

#[test]
fn invalid_or_placeholder_answers_do_not_consume_then_exact_answer_is_receipted() {
    let tmp = Tmp::new();
    let store = CapsuleStore::new(tmp.path().join("runtime-temp")).unwrap();
    let prompt = prompt();
    let current = binding(&prompt);
    let issued = store.issue(current.clone(), prompt).unwrap();
    let token = issued.pending.resume_token.clone();

    for answer in [
        ResumeAnswer {
            choice_id: "keep".into(),
            human_decision: "<answer>".into(),
            free_form: None,
        },
        ResumeAnswer {
            choice_id: "maybe".into(),
            human_decision: "Keep it".into(),
            free_form: None,
        },
        ResumeAnswer {
            choice_id: "revise".into(),
            human_decision: "Revise it".into(),
            free_form: None,
        },
        ResumeAnswer {
            choice_id: "revise".into(),
            human_decision: "Revise it".into(),
            free_form: Some("todo".into()),
        },
    ] {
        assert!(store
            .claim(&token, &current, answer, "llm:builder")
            .is_err());
        assert!(
            issued.paths.directory.is_dir(),
            "invalid answer consumed the pending continuation"
        );
    }

    let claimed = store
        .claim(&token, &current, keep_answer(), "llm:builder")
        .unwrap();
    assert_eq!(claimed.receipt.schema, AUTHORITY_RECEIPT_SCHEMA);
    assert_eq!(claimed.receipt.authority, "human");
    assert_eq!(claimed.receipt.executor, "llm:builder");
    assert_eq!(claimed.receipt.choice_id, "keep");
    assert_eq!(claimed.receipt.token_digest, digest_token(&token).unwrap());
    assert_eq!(
        claimed.receipt.human_decision,
        HumanDecision::Mediated {
            response: "Keep behavior — the signed export remains required".into()
        }
    );
    assert!(claimed.receipt.free_form.is_none());
    assert!(!issued.paths.directory.exists());
    assert!(claimed.paths.directory.is_dir());

    let receipt = serde_json::to_string(&claimed.receipt).unwrap();
    assert!(receipt.contains("Keep behavior — the signed export remains required"));
    assert!(!receipt.contains(&token), "receipt exposed the raw token");
}

#[test]
fn stale_binding_is_rejected_without_consuming_the_current_token() {
    let tmp = Tmp::new();
    let store = CapsuleStore::new(tmp.path().join("runtime-temp")).unwrap();
    let prompt = prompt();
    let current = binding(&prompt);
    let issued = store.issue(current.clone(), prompt).unwrap();
    let token = issued.pending.resume_token.clone();

    let mut stale = current.clone();
    stale.subject.hash = "ffffffffffffffff".into();
    let error = store
        .claim(&token, &stale, keep_answer(), "llm:builder")
        .unwrap_err()
        .to_string();
    assert!(error.contains("stale"), "{error}");
    assert!(issued.paths.directory.is_dir());

    store
        .claim(&token, &current, keep_answer(), "llm:builder")
        .expect("the unchanged binding can still consume after a stale attempt");
}

#[test]
fn concurrent_resume_has_exactly_one_winner_and_replay_is_rejected() {
    let tmp = Tmp::new();
    let store = Arc::new(CapsuleStore::new(tmp.path().join("runtime-temp")).unwrap());
    let prompt = prompt();
    let current = binding(&prompt);
    let issued = store.issue(current.clone(), prompt).unwrap();
    let token = issued.pending.resume_token;
    let barrier = Arc::new(Barrier::new(8));

    let joins: Vec<_> = (0..8)
        .map(|_| {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            let current = current.clone();
            let token = token.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store.claim(&token, &current, keep_answer(), "llm:builder")
            })
        })
        .collect();
    let results: Vec<_> = joins.into_iter().map(|join| join.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    for error in results.iter().filter_map(|result| result.as_ref().err()) {
        let message = error.to_string();
        assert!(
            message.contains("consumed") || message.contains("unavailable"),
            "unexpected loser error: {message}"
        );
    }

    let replay = store
        .claim(&token, &current, keep_answer(), "llm:builder")
        .unwrap_err()
        .to_string();
    assert!(replay.contains("consumed"), "{replay}");
}

#[test]
fn capsule_paths_are_confined_and_no_graph_or_sibling_state_is_mutated() {
    let tmp = Tmp::new();
    let sentinel = tmp.path().join("outside.txt");
    std::fs::write(&sentinel, "unchanged").unwrap();
    let runtime_temp = tmp.path().join("runtime-temp");
    let store = CapsuleStore::new(&runtime_temp).unwrap();
    let prompt = prompt();
    let current = binding(&prompt);
    let issued = store.issue(current.clone(), prompt).unwrap();

    for path in [
        &issued.paths.directory,
        &issued.paths.capsule,
        &issued.paths.workspace,
        &issued.paths.runtime_state,
    ] {
        assert!(path.starts_with(store.root()), "path escaped: {path:?}");
    }
    let traversal = store
        .claim("../../outside", &current, keep_answer(), "llm:builder")
        .unwrap_err()
        .to_string();
    assert!(traversal.contains("invalid Journey gate resume token"));
    assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "unchanged");
    assert!(
        !tmp.path().join(".loom").exists(),
        "gate policy must not open or mutate a Loom graph"
    );
}

#[cfg(unix)]
#[test]
fn symlinked_capsule_is_rejected_before_following_it_outside_runtime_temp() {
    use std::os::unix::fs::symlink;

    let tmp = Tmp::new();
    let runtime_temp = tmp.path().join("runtime-temp");
    let store = CapsuleStore::new(&runtime_temp).unwrap();
    let outside = tmp.path().join("outside-capsule");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("capsule.json"), "{}").unwrap();
    let token = format!("jgt1_{}", "a".repeat(64));
    let digest = digest_token(&token).unwrap();
    symlink(&outside, store.root().join("pending").join(digest)).unwrap();
    let prompt = prompt();
    let error = store
        .claim(&token, &binding(&prompt), keep_answer(), "llm:builder")
        .unwrap_err()
        .to_string();
    assert!(error.contains("confined directory"), "{error}");
    assert_eq!(
        std::fs::read_to_string(outside.join("capsule.json")).unwrap(),
        "{}"
    );
}

#[test]
fn resume_cli_requires_the_opaque_token_choice_and_exact_human_answer() {
    let token = format!("jgt1_{}", "a".repeat(64));
    let parsed = Cli::try_parse_from([
        "loom",
        "journey",
        "resume",
        &token,
        "--choice",
        "revise",
        "--human-decision",
        "Revise it to signed PDF exports",
        "--free-form",
        "Users export signed PDF reports",
        "--json",
    ])
    .unwrap();
    assert!(parsed.json);
    let Some(Command::Journey {
        cmd:
            JourneyCmd::Resume {
                token: observed_token,
                choice,
                human_decision,
                free_form,
            },
    }) = parsed.command
    else {
        panic!("expected parsed Journey resume command");
    };
    assert_eq!(observed_token, token);
    assert_eq!(choice, "revise");
    assert_eq!(human_decision, "Revise it to signed PDF exports");
    assert_eq!(
        free_form.as_deref(),
        Some("Users export signed PDF reports")
    );

    assert!(Cli::try_parse_from([
        "loom",
        "journey",
        "resume",
        &token,
        "--human-decision",
        "Keep it",
    ])
    .is_err());
    assert!(
        Cli::try_parse_from(["loom", "journey", "resume", &token, "--choice", "keep",]).is_err()
    );
}
