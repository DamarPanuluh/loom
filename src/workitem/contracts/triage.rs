use super::super::{q, EvidenceClause, HumanGate, HumanGateOption, PromptContract};
use crate::model::Node;

pub(in super::super) fn triage_contract(id: &str) -> PromptContract {
    PromptContract {
        role: "analyzer".into(),
        mindset: "Look and decide; do not fix here. Every material finding must become needed, justified, rejected, deferred, blocked, duplicate, or resolved with a concrete reason. Use resolved only after observing the repair."
            .into(),
        why_now: "an evidence-backed finding is unjudged (or its prior judgment went stale when the file changed)".into(),
        allowed_actions: vec![
            format!("loom finding verdict {id} needed --reason <what to do>"),
            format!("loom finding verdict {id} justified --reason <why it is acceptable> --evidence <file:line in the flagged code, or journal:ref>"),
            format!("loom finding verdict {id} rejected --reason <why it is false or below threshold> --evidence <file:line, or journal:ref>"),
            format!("loom finding verdict {id} deferred --reason <why not scheduled now> --evidence <file:line, or journal:ref>"),
            format!("loom finding verdict {id} blocked --reason <what it waits on>"),
            format!("loom finding verdict {id} duplicate --reason <duplicate finding id or target> --evidence <the duplicate's id, or a file:line>"),
            format!("loom finding verdict {id} resolved --reason <observed repair and proof> --evidence <file:line of the fix, or journal:ref>"),
        ],
        forbidden_actions: vec![
            "edit code here (mark it needed, then fix in build/fix)".into(),
            "justified without a concrete reason".into(),
        ],
        evidence_clauses: vec![EvidenceClause::CitesSpans { n: 1 }],
        required_evidence: "a concrete reason; and for a settling verdict (justified/rejected/deferred/duplicate/resolved) a cited --evidence (file:line in the flagged file, or a journal:ref) — the reason says WHAT you decided, the evidence says what you decided it FROM"
            .into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: format!("loom finding verdict {id} <needed|justified|rejected|deferred|blocked|duplicate|resolved> --reason '…' --evidence '<file:line for a settling verdict; omit for needed/blocked>'"),
        stop_condition: "after recording the verdict, return to loom status".into(),
        human_gate: None,
    }
}

/// Structural size/complexity findings need cohesion judgment — not a mechanical
/// "length is intentional" closeout. Owner-count is a hint, not the verdict.
pub(in super::super) fn structural_finding_triage_contract(id: &str) -> PromptContract {
    PromptContract {
        role: "analyzer".into(),
        mindset: "Judge cohesion, not line count. Read the flagged file's top-level modules/handlers. One concern → justified; a catch-all bag of unrelated commands/surfaces → needed (split). Do not fix here."
            .into(),
        why_now: "a structural detector flagged size or complexity; calibrate already set the gate — this packet is about whether the file is one concern".into(),
        allowed_actions: vec![
            format!("loom finding verdict {id} needed --reason <split plan: which concerns to separate>"),
            format!("loom finding verdict {id} justified --reason <the single cohesive concern> --evidence <file:line showing that concern>"),
            format!("loom finding verdict {id} rejected --reason <why the metric is a false positive> --evidence <file:line, or journal:ref>"),
            format!("loom finding verdict {id} deferred --reason <why not scheduled now> --evidence <file:line, or journal:ref>"),
            format!("loom finding verdict {id} blocked --reason <what it waits on>"),
            format!("loom finding verdict {id} duplicate --reason <duplicate finding id or target> --evidence <the duplicate's id, or a file:line>"),
            format!("loom finding verdict {id} resolved --reason <observed repair and proof> --evidence <file:line of the fix, or journal:ref>"),
        ],
        forbidden_actions: vec![
            "edit code here (mark it needed, then fix in build/fix)".into(),
            "justified because 'length is intentional' or 'cohesive surface' without naming one concern".into(),
            "justified from owner-count alone without reading the file structure".into(),
            "batch-reaffirm / mechanical closeout of this packet".into(),
        ],
        evidence_clauses: Vec::new(),
        required_evidence: "name the concern(s) you saw: one → justified with that name; several unrelated → needed with a split plan; false gate → rejected. A settling verdict (justified/rejected/deferred/duplicate/resolved) must also cite --evidence (file:line in the flagged file, or a journal:ref)"
            .into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: format!("loom finding verdict {id} <needed|justified|rejected|deferred|blocked|duplicate|resolved> --reason '…' --evidence '<file:line for a settling verdict; omit for needed/blocked>'"),
        stop_condition: "after recording the verdict, return to loom status".into(),
        human_gate: None,
    }
}

pub(in super::super) fn inbox_triage_contract(id: &str) -> PromptContract {
    PromptContract {
        role: "analyzer".into(),
        mindset: "Normalize raw human/external input. Route it to typed graph work or reject it with a reason; do not use inbox for code-audit findings or product questions.".into(),
        why_now: "a raw inbox item is still new".into(),
        allowed_actions: vec![
            format!("loom inbox show {id}"),
            "choose one supported landing: existing_journey | new_journey | existing_intent | hypothesis | spike | external_research; run its creation/lookup command and retain the returned stable node id".into(),
            "routing commands: loom journey add / loom intent add / loom hypothesis add / loom task add --kind spike|research".into(),
            format!("loom inbox mark {id} routed --reason '<supported-destination-kind>:<returned-stable-node-id>'"),
            format!("loom inbox mark {id} rejected --reason <why it is not actionable>"),
        ],
        forbidden_actions: vec![
            "leave the item new after using it".into(),
            "drop context without recording the disposition".into(),
        ],
        evidence_clauses: Vec::new(),
        required_evidence: "the durable destination or concrete rejection reason".into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: format!("loom inbox mark {id} routed --reason '<existing_journey|new_journey|existing_intent|hypothesis|spike|external_research>:<returned-stable-node-id>' (or use rejected|duplicate|deferred with a concrete prose reason)"),
        stop_condition: "after disposition, return to loom status".into(),
        human_gate: None,
    }
}

/// Ratification: the decision is human-only; presentation and recording may be
/// mediated by an LLM. The host-facing gate is structured so the LLM can offer
/// useful choices and a recommendation, wait, then perform the typed write.
pub(in super::super) fn ratify_contract(intent: &Node) -> PromptContract {
    let id = crate::model::short(&intent.id);
    PromptContract {
        role: "human".into(),
        mindset: "Product authority. The LLM summarizes the evidence, recommends one option with reasons, asks the human, waits, then records the human's answer. It may execute the write; it may not choose the answer."
            .into(),
        why_now: "the intent's wantedness is unestablished: minted without ratification, or redefined after it".into(),
        allowed_actions: vec![
            format!("loom intent show {id}"),
            "ask the human with the three options in human_gate; mark the evidence-backed recommendation and explain its tradeoff".into(),
            format!("loom intent ratify {id} --evidence <why this is wanted> --human-decision <exact human answer>"),
            format!("loom intent reject {id} --reason <why it is not wanted> --human-decision <exact human answer>"),
            format!("loom intent update {id} --description <corrected criterion> --reason <…>  (then re-ratify)"),
        ],
        forbidden_actions: vec![
            "supplying --human-decision before the human answers, paraphrasing silence as approval, or choosing on the human's behalf".into(),
            "using the direct ratification path from an llm:* lane (INV-8 rejects it)".into(),
            "treating silence or plausibility as ratification".into(),
        ],
        evidence_clauses: vec![
            EvidenceClause::Prose,
            EvidenceClause::VerificationAtLeast {
                level: "cited".into(),
            },
        ],
        required_evidence: "the human's reason this behavior is wanted: an utterance, a source doc, a decision"
            .into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: format!("after the answer: loom intent ratify {id} --evidence '…' --human-decision '<exact answer>'  (or the selected reject/update command)"),
        stop_condition: "wait for the human; after recording their selected option, return to loom status. If they defer or do not answer, write nothing".into(),
        human_gate: Some(HumanGate {
            question: format!("Should '{}' remain a wanted behavior?", intent.name),
            options: vec![
                HumanGateOption {
                    id: "ratify".into(),
                    label: "Keep behavior".into(),
                    description: "Ratify the current criterion as wanted and continue proving or maintaining it.".into(),
                    write_back: Some(format!("loom intent ratify {id} --evidence '<why wanted>' --human-decision '<exact human answer>'")),
                },
                HumanGateOption {
                    id: "reject".into(),
                    label: "Remove behavior".into(),
                    description: "Reject it as unwanted; Loom will track any live implementation as removal work.".into(),
                    write_back: Some(format!("loom intent reject {id} --reason '<why unwanted>' --human-decision '<exact human answer>'")),
                },
                HumanGateOption {
                    id: "revise".into(),
                    label: "Revise criterion".into(),
                    description: "Correct what the behavior should mean before deciding whether to keep it.".into(),
                    write_back: Some(format!("loom intent update {id} --description '<corrected criterion>' --reason '<human decision>'")),
                },
            ],
            recommendation: "The presenting LLM must recommend one option from the packet's current implementation, proof, usage, and divergence evidence; state uncertainty and never treat the recommendation as the decision.".into(),
            after_answer: "Wait for the human's selection. Record Keep/Remove with their exact answer in --human-decision; for Revise, obtain the corrected criterion before writing. No answer means no write.".into(),
        }),
    }
}

/// Prep lane: shrink the human ratify queue. Never invents wantedness.
pub(in super::super) fn rectify_contract(intent: &Node, kind: &str) -> PromptContract {
    let id = crate::model::short(&intent.id);
    let name = q(&intent.name);
    let mut allowed_actions = vec![
        format!("loom intent show {id}"),
        format!("loom intent update {id} --visibility internal --reason '<why this is not user-facing product surface>'"),
        format!("loom intent update {id} --rectify escalated --reason '<why a human must decide wantedness>'"),
        format!("loom edge relate scenario-of {name} <parent-intent>"),
        format!("loom edge relate relates {name} <sibling-intent>"),
        format!("loom edge explore {name} <peer> ground --criterion '…' --evidence 'file:line — …' --confidence <0.0-1.0>"),
        format!("loom intent retire {id} --reason '<duplicate of better-named intent>' --replaced-by <keeper>"),
        format!("loom intent update {id} --description '<sharper falsifiable criterion>' --reword --reason '<same meaning, clearer words>'"),
    ];
    if kind == "duplicate_intent" {
        allowed_actions.push(format!(
            "loom intent update {id} --rectify clear --reason '<description discriminator proving these intents are distinct>'"
        ));
    }
    PromptContract {
        role: "rectify".into(),
        mindset: "Clear NEEDLESS ratify friction. Structural fixes only — false duplicates, \
                  mis-marked visibility, missing scenario_of/relates. If the behavior is a \
                  real user-visible product call that an LLM cannot honestly decide, escalate \
                  it to the human ratify lane. Never invent a yes or no on wantedness."
            .into(),
        why_now: format!(
            "blocking divergence '{kind}' looks like prep work, not a product decision yet"
        ),
        allowed_actions,
        forbidden_actions: vec![
            "loom intent ratify (INV-8 — wantedness is human-only)".into(),
            "loom intent reject (that is a product decision; escalate instead)".into(),
            "supplying --human-decision or treating obviousness as ratification".into(),
            "editing production code to silence a divergence".into(),
            "loom edge implement (rectify does not ground new behavior)".into(),
        ],
        evidence_clauses: vec![
            EvidenceClause::Prose,
            EvidenceClause::VerificationAtLeast {
                level: "cited".into(),
            },
        ],
        required_evidence: "file:line (or graph structure) showing why the friction was false, \
                            or a concrete reason the human must decide"
            .into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: if kind == "duplicate_intent" {
            format!(
                "record the pair-specific discriminator with `loom intent update {id} \
                 --rectify clear --reason '…'`, or apply a structural fix (scenario-of / retire / reword)"
            )
        } else {
            format!(
                "structural fix (visibility / scenario-of / retire / reword) OR \
                 loom intent update {id} --rectify escalated --reason '…'"
            )
        },
        stop_condition: "after the write, return to loom status. If escalated, the item \
                         leaves rectify and appears on loom next --mode ratify"
            .into(),
        human_gate: None,
    }
}
