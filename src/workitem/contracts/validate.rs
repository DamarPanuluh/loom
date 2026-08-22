use super::super::{q, EvidenceClause, PromptContract};
use super::{verdict_write_back, FINDING_ADD_ACTION, NON_BLOCKING_SMELL_RULE};
use crate::model::{Edge, Node};
use crate::store::Store;
use crate::Result;

pub(in super::super) fn validator_contract(
    store: &Store,
    edge: &Edge,
    val_name: &str,
    intent_name: &str,
) -> Result<PromptContract> {
    let val = store.get_node(&edge.from_id)?;
    if let Some(validation) = &val {
        if let Some((journey, profile)) =
            crate::completeness::compiler_owned_journey_validation(store, validation)?
        {
            let mut contract = journey_proof_contract_for_profile(&journey, &profile);
            contract.why_now = format!(
                "compiler-owned {} edge is {}; settle it with `loom journey run {} --profile {}`",
                edge.kind, edge.status, journey.id, profile
            );
            return Ok(contract);
        }
    }
    let command = val
        .as_ref()
        .and_then(|n| {
            n.body
                .get("command")
                .and_then(|c| c.as_str())
                .map(String::from)
        })
        .unwrap_or_default();
    // A runnable proof cannot be verdicted by hand — the floor demands a Run,
    // and offering `validation verdict` here sends the worker at a wall the
    // write boundary will refuse. Offer it ONLY for a manual check, which is
    // the one shape that has no command for loom to execute.
    let validation_type = val
        .as_ref()
        .and_then(|n| n.body.get("type").and_then(|t| t.as_str()))
        .unwrap_or("test");
    let manual_check = validation_type == "manual_check";
    let runnable = !command.trim().is_empty() && !manual_check;
    let unconfigured = command.trim().is_empty() && !manual_check;
    let write_back = if runnable {
        format!(
            "loom observe --for {} -- {}   (or)   loom validation run {}",
            q(intent_name),
            if command.is_empty() {
                "<cmd>"
            } else {
                &command
            },
            q(intent_name)
        )
    } else if unconfigured {
        format!(
            "loom validation update {} --command '<runnable-command>'; loom validation run {}",
            q(val_name),
            q(val_name)
        )
    } else {
        verdict_write_back(edge, val_name, intent_name)
    };
    Ok(PromptContract {
        role: "validator".into(),
        mindset:
            "Run it; do not guess. Record exactly what the command produced. Do not edit code \
                  to make a proof pass. A blocked proof is honest — record it with a reason."
                .into(),
        why_now: format!("validates edge is {}", edge.status),
        allowed_actions: {
            let mut actions = Vec::new();
            if runnable {
                // A stored command containing shell operators cannot be pasted
                // after `--`: the calling shell splits it, so the wrapper sees
                // only the first clause, mints a proof for THAT, and leaves
                // this edge unrun. The queue then serves the same item forever
                // while every run appears to succeed.
                //
                // `loom validation run` executes the stored command exactly, so
                // it is the correct offer for those — and the wrapper leads for
                // the simple case, where pasting is safe and the worker gets to
                // keep a command they were going to type anyway.
                let shell_shaped = command.contains("&&")
                    || command.contains("||")
                    || command.contains(';')
                    || command.contains('|')
                    || command.contains('>');
                if shell_shaped {
                    actions.push(format!("loom validation run {}", q(intent_name)));
                    actions.push(format!(
                        "loom observe --for {} -- sh -c {}",
                        q(intent_name),
                        q(&command)
                    ));
                } else {
                    actions.push(format!(
                        "loom observe --for {} -- {}",
                        q(intent_name),
                        command
                    ));
                    actions.push(format!("loom validation run {}", q(intent_name)));
                }
            } else if manual_check {
                actions.push("<no command — this is a manual check>".into());
                actions.push(verdict_write_back(edge, val_name, intent_name));
            } else {
                actions.push(format!("loom validation show {}", q(val_name)));
                actions.push(format!(
                    "loom validation update {} --command '<runnable-command>'",
                    q(val_name)
                ));
                actions.push(format!("loom validation run {}", q(val_name)));
            }
            actions.push(FINDING_ADD_ACTION.into());
            actions
        },
        forbidden_actions: vec![
            "edit code to make the proof pass".into(),
            "mark passed without observed proof".into(),
            NON_BLOCKING_SMELL_RULE.into(),
        ],
        evidence_clauses: vec![
            EvidenceClause::CitesRun,
            EvidenceClause::VerificationAtLeast {
                level: "verified".into(),
            },
        ],
        required_evidence: if runnable || unconfigured {
            "a run loom performed — its exit code and output. A reported outcome is refused".into()
        } else {
            "what you observed in the manual check, citing file:line or a journal entry".into()
        },
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back,
        stop_condition: "after recording the result, return to loom status".into(),
        human_gate: None,
    })
}

/// Contract for an implemented intent whose proof floor is still open. Distinct
/// from `validator_contract`, which re-runs an EXISTING pending proof: here the
/// proof must be added or strengthened. Journey-root S3 is routed separately
/// through [`journey_proof_contract`]; an Intent proof closes only the S2
/// behavioral floor and never invents a legacy executable Journey spec.
pub(in super::super) fn unproven_contract(
    intent: &Node,
    proof: crate::proofstrength::ProofAssessment,
) -> PromptContract {
    let name = q(&intent.name);
    let weak_passing = proof.any_passing && !proof.meaningful_passing;
    let best = proof
        .best_passing_strength
        .unwrap_or(crate::proofstrength::Strength::S0)
        .as_str();
    PromptContract {
        role: "validator".into(),
        mindset: if weak_passing {
            format!(
                "This proof ran and passed, but its {best} grade proves only liveness. Strengthen \
                 the proof so it would FAIL if this behavior broke: add an output/content assertion, \
                 update or replace the registered command/spec, and rerun it."
            )
        } else {
            "An implemented claim with no passing proof is a claim, not truth. Write a proof \
             that would FAIL if this behavior broke — a check that only asserts the process \
             exited 0 proves liveness, not behavior."
                .into()
        },
        why_now: if weak_passing {
            format!("implemented, proof ran and passed at {best}, but meaningful proof requires S2")
        } else if proof.any_registered {
            "implemented, proof registered, none passing".into()
        } else {
            "implemented with no registered proof at all".into()
        },
        allowed_actions: {
            let mut actions = vec![
                format!("loom intent show {}", q(&intent.id)),
                "read the grounded files listed in this packet's read set".into(),
            ];
            if weak_passing {
                actions.extend([
                    "strengthen or replace the proof with an output/content assertion (for example stdout_contains/body/exists, or a runner command that reports passing assertions)".into(),
                    "loom validation update <validation> --command '<command whose output/content assertion establishes the behavior>'".into(),
                    format!("loom validation run {name}"),
                ]);
            } else {
                actions.extend([
                    format!(
                        "loom validation add --name '<what it proves>' --type test --command '<cmd>' --intent {name}"
                    ),
                    format!("loom validation run {name}"),
                ]);
            }
            actions.push(FINDING_ADD_ACTION.into());
            actions
        },
        forbidden_actions: vec![
            "recording a passing result without running the command".into(),
            "asserting only an exit code when the behavior has observable output".into(),
            "editing code to make a proof pass".into(),
            NON_BLOCKING_SMELL_RULE.into(),
        ],
        evidence_clauses: vec![
            EvidenceClause::CitesRun,
            EvidenceClause::ProofStrengthAtLeast { grade: "S2".into() },
        ],
        required_evidence:
            "the command loom ran, its exit status, and the assertion that would have caught a \
             regression"
                .into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: if weak_passing {
            format!(
                "loom validation update <validation> --command '<stronger command with an output/content assertion>'  then  loom validation run {name}  (or add a new S2+ proof)"
            )
        } else {
            format!(
                "loom validation add --name '<what it proves>' --type test --command '<cmd>' --intent {name}  then  loom validation run {name}"
            )
        },
        stop_condition: "stop when this intent has a passing meaningful proof at S2 or stronger; Journey-root S3 is a separate compiled Journey packet".into(),
        human_gate: None,
    }
}

/// A compiled Journey whose consumer-plane proof has not yet earned S3.
/// Compilation creates the validation-specific Proves/Validates/Calls/
/// Exercises closure; running records only what Loom actually observes.
pub(in super::super) fn journey_proof_contract(journey: &Node) -> PromptContract {
    journey_proof_contract_for_profile(journey, "proof")
}

pub(in super::super) fn journey_proof_contract_for_profile(
    journey: &Node,
    profile: &str,
) -> PromptContract {
    let id = q(&journey.id);
    let profile = q(profile);
    PromptContract {
        role: "validator".into(),
        mindset: "Compile the current authored Journey into its proof profile, then run that exact profile. Do not hand-author a sibling Validation or attach an intent-wide witness: Journey compile owns the validation-specific Proves/Validates/Calls/Exercises closure. Run it; do not guess, and do not edit code to make the proof pass.".into(),
        why_now: format!(
            "compiled Journey '{}' has no current passing S3 proof through its surfaced CLI",
            journey.name
        ),
        allowed_actions: vec![
            format!("loom journey compile {id} --profile {profile}"),
            format!("loom journey run {id} --profile {profile}"),
            "inspect the compiled validation's call evidence and exact failure output".into(),
            FINDING_ADD_ACTION.into(),
        ],
        forbidden_actions: vec![
            "loom validation add --journey (there is no alternate proof door)".into(),
            "editing source code to make the proof pass".into(),
            "recording a passing verdict without a Loom-observed run".into(),
            "relying on an intent-wide verifies grounding instead of the compiled validation-specific witness".into(),
            NON_BLOCKING_SMELL_RULE.into(),
        ],
        evidence_clauses: vec![
            EvidenceClause::CitesRun,
            EvidenceClause::ProofStrengthAtLeast { grade: "S3".into() },
        ],
        required_evidence: "the proof-profile run Loom performed, including exit status/output and the validation-specific call witness through the surfaced CLI".into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: format!(
            "loom journey compile {id} --profile {profile}; loom journey run {id} --profile {profile}"
        ),
        stop_condition: "stop when the current Journey has a passing S3 proof through its surfaced CLI, or when Loom records the honest failure/blocker; then return to loom status".into(),
        human_gate: None,
    }
}

/// The deepen contract. Unlike every other lane, this one does not close a gap
/// — it raises a floor that is already met, so the stop condition is a single
/// move rather than an empty queue.
/// The deepen contract.
///
/// Whether the move can raise a grade decides what this contract may demand.
/// `FreezeBaseline` is the move at S3, and S3 is the highest grade
/// `proofstrength` assigns (see the comment at src/proofstrength.rs:1653), so
/// asking for "a grade higher than the current one" as *acceptance criteria*
/// asks for something no correct move can produce. `required_evidence` and
/// `evidence_clauses` are the contract's acceptance criteria, not prose — a
/// worker satisfying them literally cannot, and anything consuming the clauses
/// programmatically inherits the impossible demand.
pub(in super::super) fn deepen_contract(
    id: &str,
    name: &str,
    next_move: crate::risk::Move,
) -> PromptContract {
    let name = q(name);
    let raises_grade = next_move != crate::risk::Move::FreezeBaseline;
    let next_move = next_move.as_str();
    PromptContract {
        role: "validator".into(),
        mindset: "This behavior is already green. You are not fixing it — you are making \
                  the graph harder to be wrong about, starting with the thing that would \
                  hurt most if it were."
            .into(),
        why_now: "every floor is met, so the question is no longer 'what is missing' but \
                  'what is weakest'"
            .into(),
        allowed_actions: vec![
            format!("loom intent show {id}"),
            format!("loom impact {id}"),
            "loom journey add <spec>".into(),
            "loom journey freeze <journey>".into(),
            "loom validation run <name>".into(),
        ],
        forbidden_actions: vec![
            "weakening an existing assertion to make a proof pass".into(),
            "recording a stronger grade — strength is derived from the proof's shape, \
             never asserted"
                .into(),
        ],
        evidence_clauses: vec![
            EvidenceClause::CitesRun,
            EvidenceClause::Produces {
                what: if raises_grade {
                    "a proof grade higher than the current one".into()
                } else {
                    "a frozen baseline that replays, so a change in the shape of the \
                     output is noticed"
                        .into()
                },
            },
        ],
        required_evidence: if raises_grade {
            "a proof loom ran, whose new grade is higher than the old one".into()
        } else {
            "a proof loom ran, and a baseline frozen from it — the grade will not move, \
             because S3 is the highest grade currently assigned"
                .to_string()
        },
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: format!(
            "the move for this behavior is: {next_move} — close it with one proof move: \
             loom validation add --intent {name} --name '<what it proves>' --type test \
             --command '<cmd>' then loom validation run <name>; or loom journey add <spec> \
             for the behavior; or loom journey freeze <journey> to pin the baseline. \
             {closing}",
            closing = if raises_grade {
                "loom sync then re-grades the proof and re-ranks this queue"
            } else {
                "loom sync then re-reads the proof, but the grade stays at S3 and this \
                 item ranks first again — that is the ceiling, not your move failing"
            }
        ),
        stop_condition: if raises_grade {
            "stop after ONE move — this queue re-ranks after every change, and the \
             next-most-important thing is probably no longer this one"
                .into()
        } else {
            "stop after ONE move — and expect this same item back: the baseline move \
             cannot change the ranking, because the grade it would raise is already at \
             its ceiling"
                .to_string()
        },
        human_gate: None,
    }
}

/// The audit contract: loom found something in its own record that does not
/// look earned.
pub(in super::super) fn audit_contract(remedy: &str) -> PromptContract {
    PromptContract {
        role: "analyzer".into(),
        mindset: "Something in this graph's record does not look like it was earned. \
                  Establish what actually happened before changing anything — a record \
                  that was wrong once can be wrong again in the fix."
            .into(),
        why_now: "a graph whose claim is falsifiability has to be able to fail its own check"
            .into(),
        allowed_actions: vec![
            "loom audit --json".into(),
            "read the append-only record at .loom/journal/events.jsonl".into(),
            "loom intent show <id>".into(),
        ],
        forbidden_actions: vec![
            "re-asserting the flagged claim to clear the finding".into(),
            "deleting the record instead of correcting it".into(),
        ],
        evidence_clauses: vec![EvidenceClause::Produces {
            what: "either the missing anchor, or a withdrawal of the claim".into(),
        }],
        required_evidence: "either the anchor that was missing, or a withdrawal of the claim"
            .into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        // Uniform adjudicability: every audit packet's write_back names a
        // runnable closeout. Remedies that already name one keep it; prose
        // remedies get the universal state-closure: fix, then re-read the
        // record — the finding must be absent.
        write_back: if remedy.contains("loom ") {
            format!("{remedy}; close: loom audit --json — the finding must be absent")
        } else {
            format!(
                "{remedy}; close: fix per the remedy, then loom audit --json — the finding must be absent"
            )
        },
        stop_condition: "stop when the claim is either anchored or withdrawn — never when \
                         it merely stops being reported"
            .into(),
        human_gate: None,
    }
}
