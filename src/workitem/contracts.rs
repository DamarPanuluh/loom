//! Role contracts — the PromptContract text for every work-item lane.
//!
//! Plane: judgment-plane routing (text assembly only). Each contract states
//! the role, mindset, allowed and forbidden actions, required evidence, and
//! the exact prefilled write-back command for one lane — the write-back must
//! target the store's gated paths, so a contract can never instruct a way
//! around INV-4/5/6. No store writes happen here.

use super::queues::prescreen_for;
use super::{q, EvidenceClause, PromptContract};
use crate::model::{Edge, EdgeKind, Node};
use crate::store::Store;
use crate::Result;

const FINDING_ADD_ACTION: &str = "loom finding add '<claim>' --source code_audit --kind code_audit --file <registered-codefile> --evidence '<file:line — observed fact>' --impact '<why it matters>' --confidence <0.0-1.0>";
const NON_BLOCKING_SMELL_RULE: &str = "silently skipping a material non-blocking smell; either capture it as a finding, reject it with evidence in triage, or leave it unmentioned because it is below capture threshold";

/// The elaboration contract: grow the surroundings the human forgot, decide
/// nothing that belongs to the human.
pub(super) fn elaborator_contract(
    intent: &Node,
    card: &crate::completeness::Scorecard,
) -> PromptContract {
    let name = q(&intent.name);
    PromptContract {
        role: "builder".into(),
        mindset: "The human gave the core idea; the surroundings are systematically \
                  forgotten — growing them is this item's whole job. For each OPEN axis \
                  on the scorecard: create the missing artifact, or waive the axis with a \
                  real reason, or raise ONE crisp question to the human when it is a \
                  product decision. Proposed scenarios are planned intents — the normal \
                  build loop ratifies them. Never decide product questions yourself."
            .into(),
        why_now: format!(
            "{} of {} completeness axes are open around this user-visible idea",
            card.open,
            card.axes.len()
        ),
        allowed_actions: vec![
            format!(
                "scenarios: loom intent add --name '<what goes wrong / degraded path / boundary case>' --description '<falsifiable criterion>' --aspect <sad|fallback|edge_case> --visibility user_visible; then loom edge relate scenario-of '<that scenario>' {name}"
            ),
            format!(
                "prerequisites: loom edge relate requires {name} '<intent that must exist first>'"
            ),
            "boundary/proof/journey: loom validation add … / loom journey coverage add … (or let the quality and validate queues drive them)".into(),
            format!(
                "questions: loom question add \"<one crisp product question>\" --intent {name}"
            ),
            format!("waive: loom intent waive {name} <axis> --reason '<why it deliberately does not apply>'"),
        ],
        forbidden_actions: vec![
            "deciding a product question yourself — raise it and move on".into(),
            "proposing scenarios that restate the happy path".into(),
            "waiving an axis just to close it (a waiver needs a real reason)".into(),
        ],
        evidence_clauses: Vec::new(),
        required_evidence: "every open axis closed by an artifact, a waiver, or a question — never by silence".into(),
        evidence_template: None,
        examples: None,
        pre_screened_hits: Vec::new(),
        write_back: "one command per open axis (see allowed actions), then loom status".into(),
        stop_condition: "after addressing every open axis, return to loom status".into(),
        human_gate: None,
    }
}

// ---- role contracts (see docs/llm-driver.md) -------------------------------

/// The exact re-record command that closes THIS edge's verdict, prefilled with
/// real endpoint names. Relates claims use the ergonomic name-resolving
/// `edge explore`; every other relationship kind is re-recorded by edge id —
/// `edge explore` would silently target a different (relates) edge.
fn verdict_write_back(edge: &Edge, from: &str, to: &str) -> String {
    match edge.kind {
        EdgeKind::Governs => format!(
            "loom rule verdict {} {} <passing|failing|independent> --criterion '…' --evidence '…' --confidence <0.0-1.0>",
            q(from),
            q(to)
        ),
        EdgeKind::Validates => format!(
            "loom validation verdict {} <passed|failed|blocked> --evidence '…'   (blocked: add --reason '…')",
            q(from)
        ),
        EdgeKind::Relates => format!(
            "loom edge explore {} {} <ground|issue|independent> --criterion '…' --evidence '…' --confidence <0.0-1.0>",
            q(from),
            q(to)
        ),
        _ => format!(
            "loom edge verdict {} <ground|issue|independent> --criterion '…' --evidence '…' --confidence <0.0-1.0>",
            edge.id
        ),
    }
}

pub(super) fn builder_contract(intent: &Node) -> PromptContract {
    let name = q(&intent.name);
    PromptContract {
        role: "builder".into(),
        mindset: "Use Loom first to understand why this work exists, which entities/files are \
                  likely relevant, and what prior evidence says; then inspect the relevant code \
                  before editing. Functions/symbols are locators, not intents. Do not self-certify \
                  quality or proofs."
            .into(),
        why_now: format!("intent '{}' is {} and not yet realized", intent.name, intent.status),
        allowed_actions: vec![
            "loom status".into(),
            "loom next --all".into(),
            format!("loom intent show {name}"),
            "loom codefile list".into(),
            "loom codefile show <file>".into(),
            "edit code".into(),
            format!("loom edge implement {name} <codefile> --locator <symbol>"),
            format!("loom intent update {name} --lifecycle implemented --reason '<what was built>'"),
            "loom sync".into(),
            FINDING_ADD_ACTION.into(),
        ],
        forbidden_actions: vec![
            "loom rule verdict passing (quality lane)".into(),
            "loom validation verdict passed (validator lane)".into(),
            NON_BLOCKING_SMELL_RULE.into(),
        ],
        evidence_clauses: vec![
            EvidenceClause::CitesSpans { n: 1 },
            EvidenceClause::VerificationAtLeast {
                level: "cited".into(),
            },
        ],
        required_evidence: "Loom context checked, relevant code inspected, code written, locator confirmed, sync clean".into(),
        evidence_template: None,
        examples: None,
        pre_screened_hits: Vec::new(),
        write_back: format!(
            "loom edge implement {name} <codefile> --locator <symbol>; loom intent update {name} --lifecycle implemented --reason '<what was built>'"
        ),
        stop_condition: "after grounding + sync, return to loom status".into(),
        human_gate: None,
    }
}

/// A registration pointing at a file that no longer exists on disk. There is
/// nothing to read — the honest moves are unregistering, or registering the
/// successor file and re-grounding the affected intents there.
pub(super) fn missing_codefile_contract(codefile: &Node) -> PromptContract {
    let file = q(&codefile.name);
    PromptContract {
        role: "builder".into(),
        mindset: "This registered file is GONE from disk (deleted or renamed). Do not try to \
                  read it. If it was renamed/split, register the successor file(s) and ground \
                  the affected intents there; then unregister this ghost. If the behavior it \
                  carried is genuinely gone, unregister it and let sync settle the residue."
            .into(),
        why_now: format!("codefile '{}' is registered but missing from disk", codefile.name),
        allowed_actions: vec![
            format!("loom codefile show {file} (see which intents grounded here)"),
            "loom codefile add <successor-path> (when the file was renamed/split)".into(),
            format!("loom edge implement <intent> <successor> --locator <symbol> (re-ground before removing)"),
            format!("loom codefile remove {file}"),
            "loom sync".into(),
        ],
        forbidden_actions: vec![
            "grounding an intent to the missing file".into(),
            "inventing an intent to keep a dead registration alive".into(),
        ],
        evidence_clauses: Vec::new(),
        required_evidence: "the successor registration + re-grounding, or the removal of the dead registration".into(),
        evidence_template: None,
        examples: None,
        pre_screened_hits: Vec::new(),
        write_back: format!(
            "loom codefile remove {file}  (after re-grounding any intents it carried)"
        ),
        stop_condition: "after unregistering (and any re-grounding) + sync, return to loom status".into(),
        human_gate: None,
    }
}

pub(super) fn coverage_contract(codefile: &Node) -> PromptContract {
    let file = q(&codefile.name);
    PromptContract {
        role: "builder".into(),
        mindset: "A registered file with no owning intent is a coverage gap. Make the judgment \
                  BEFORE grounding anything: does an intent's behavior LIVE in this file? If yes, \
                  ground it --role realizes (that is what owns coverage). If the file only CALLS \
                  behavior that lives elsewhere (an HTTP route, a topic, a config key, an SDK), it \
                  is a consumer surface: create the owning intent for THIS surface, ground that \
                  --role realizes, and record --role consumes edges to the intents it merely \
                  exercises. A consumes edge never owns coverage, so a consumer surface stays \
                  visibly unowned until its realizing intent exists. Read the file before deciding; \
                  do not invent an intent, and do not ground a mere caller as realizes, just to \
                  satisfy the gate."
            .into(),
        why_now: format!("codefile '{}' is registered but unowned", codefile.name),
        allowed_actions: vec![
            format!("loom codefile show {file}"),
            "loom intent list".into(),
            "read the file to see whether a behavior LIVES here or is merely called".into(),
            format!("loom edge implement <intent> {file} --role realizes --locator <symbol>"),
            format!("loom edge implement <consumed-intent> {file} --role consumes --locator <seam>"),
            format!("loom codefile remove {file} (if it should not be tracked)"),
            "loom ignore add '<glob>' --reason '…' (if outside the tracked surface)".into(),
            "loom sync".into(),
        ],
        forbidden_actions: vec![
            "grounding a mere caller as --role realizes just to satisfy coverage".into(),
            "inventing an intent with no behavioral description".into(),
            "loom rule verdict passing (quality lane)".into(),
        ],
        evidence_clauses: vec![EvidenceClause::CitesSpans { n: 1 }],
        required_evidence: "file read; a realizing owner chosen with a locator, OR a new realizing intent for this surface plus consumes edges to what it calls, OR a reason to unregister".into(),
        evidence_template: None,
        examples: Some(serde_json::json!([
            {
                "situation": "the behavior LIVES in this file",
                "do": "loom edge implement \"<that intent>\" <file> --role realizes --locator <symbol>"
            },
            {
                "situation": "the file only CALLS behavior that lives elsewhere (a page hitting a backend route)",
                "do": "create the owning intent for this surface (level feature; visibility user_visible if a person touches it), ground it --role realizes, then add --role consumes edges to the intents it exercises, naming the seam (route/topic/key) in the locator"
            }
        ])),
        pre_screened_hits: Vec::new(),
        write_back: format!(
            "loom edge implement <intent> {file} --role realizes --locator <symbol>   (or, if it only calls behavior elsewhere)   loom intent add --name '<surface behavior>' --visibility user_visible … ; loom edge implement '<surface behavior>' {file} --role realizes --locator <symbol> ; loom edge implement '<consumed intent>' {file} --role consumes --locator <seam>   (or)   loom codefile remove {file}"
        ),
        stop_condition: "after grounding (realizes), or creating the realizing surface intent + its consumes edges, or unregistering, + sync, return to loom status".into(),
        human_gate: None,
    }
}

pub(super) fn analyzer_contract(edge: &Edge, from_name: &str, to_name: &str) -> PromptContract {
    let write_back = verdict_write_back(edge, from_name, to_name);
    PromptContract {
        role: "analyzer".into(),
        mindset: "Read both sides. Form a hypothesis before inspecting code. Record exactly what \
                  the code shows. Do not fix code; do not preserve the old verdict by assumption."
            .into(),
        why_now: format!("{} edge is {}", edge.kind, edge.status),
        allowed_actions: vec![
            "read codefiles, notes, prior evidence".into(),
            write_back.clone(),
            FINDING_ADD_ACTION.into(),
        ],
        forbidden_actions: vec![
            "edit code".into(),
            "record a verdict from name similarity or assumption".into(),
            NON_BLOCKING_SMELL_RULE.into(),
        ],
        evidence_clauses: vec![EvidenceClause::CitesSpans { n: 1 }],
        required_evidence: "file/line locators, validation output, or runtime evidence".into(),
        evidence_template: None,
        examples: None,
        pre_screened_hits: Vec::new(),
        write_back,
        stop_condition: "after recording the verdict, return to loom status".into(),
        human_gate: None,
    }
}

pub(super) fn fixer_contract(edge: &Edge, from_name: &str, to_name: &str) -> PromptContract {
    PromptContract {
        role: "fixer".into(),
        mindset: "Use Loom first to understand the stale/failing criterion, linked entities, \
                  likely files, and prior evidence; then inspect the relevant code before editing. \
                  Repair the actual broken behavior at its root cause, not the symptom. Code moving \
                  is not behavior changing. If the product changed, route through intent update, not \
                  a silent code change. After the fix, sync re-opens this claim and routes \
                  re-measurement to its owning lane — do not record the verdict from the fixer hat."
            .into(),
        why_now: format!("{} edge is failing", edge.kind),
        allowed_actions: vec![
            "loom status".into(),
            "loom next --all".into(),
            format!("loom edge show {}", edge.id),
            format!("loom intent show {}", q(if edge.kind == EdgeKind::Governs { to_name } else { from_name })),
            "loom codefile show <file>".into(),
            "edit code".into(),
            "loom sync".into(),
            "loom edge implement (re-ground if the fix moved code)".into(),
            FINDING_ADD_ACTION.into(),
        ],
        forbidden_actions: vec![
            "recording the passing verdict yourself (the owning lane re-measures after sync)".into(),
            "suppress the symptom without a root-cause fix".into(),
            NON_BLOCKING_SMELL_RULE.into(),
        ],
        evidence_clauses: vec![EvidenceClause::CitesSpans { n: 1 }],
        required_evidence: "Loom context checked, relevant code inspected, code change, sync clean, the failing criterion now addressed at its cause".into(),
        evidence_template: None,
        examples: None,
        pre_screened_hits: Vec::new(),
        write_back: "fix the source at root cause, then loom sync — sync re-opens this claim as needs_reverification and its owning lane re-measures it".into(),
        stop_condition: "after the fix + sync, return to loom status".into(),
        human_gate: None,
    }
}

pub(super) fn quality_contract(
    store: &Store,
    edge: &Edge,
    rule_name: &str,
    intent_name: &str,
) -> Result<PromptContract> {
    // The rule (edge.from) carries the inspection protocol — embed it so verdicts
    // are consistent across sessions (see docs/llm-driver.md quality contract).
    let rule = store.get_node(&edge.from_id)?;
    let hits = prescreen_for(store, rule.as_ref(), &edge.to_id)?;
    Ok(quality_contract_body(
        rule.as_ref(),
        &format!("governs edge is {}", edge.status),
        rule_name,
        intent_name,
        hits,
    ))
}

/// The quality prompt contract, shared by the edge path (re-measure) and the
/// unmeasured-pair path (first measurement, where no edge exists yet).
pub(super) fn quality_contract_body(
    rule: Option<&Node>,
    why_now: &str,
    rule_name: &str,
    intent_name: &str,
    pre_screened_hits: Vec<crate::prescan::PreScreenHit>,
) -> PromptContract {
    let body = rule.map(|n| n.body.clone()).unwrap_or_default();
    let guide = body
        .get("inspection_guide")
        .and_then(|v| v.as_str())
        .unwrap_or("inspect the code against this rule")
        .to_string();
    let hints: Vec<String> = body
        .get("detection_hints")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let evidence_template = body
        .get("evidence_template")
        .cloned()
        .filter(|v| !v.is_null());
    let examples = match (body.get("passing_example"), body.get("failing_example")) {
        (None, None) => None,
        (p, f) => Some(serde_json::json!({ "passing": p, "failing": f })),
    };
    let write_back = format!(
        "loom rule verdict {} {} <passing|failing|independent> --criterion '…' --evidence '…' --confidence <0.0-1.0>",
        q(rule_name),
        q(intent_name)
    );
    let mut allowed = vec![
        "loom codefile show <file>".into(),
        "read the grounded code".into(),
        write_back.clone(),
        FINDING_ADD_ACTION.into(),
    ];
    allowed.extend(hints.into_iter().map(|h| format!("hint: {h}")));
    let template_note = if evidence_template.is_some() {
        " Phrase evidence with the rule's evidence_template so verdicts are comparable across sessions."
    } else {
        ""
    };
    let hits_note = if pre_screened_hits.is_empty() {
        ""
    } else {
        " Machine pre-screened hits are attached: confirm or refute EVERY hit before your verdict — they are candidates, not conclusions."
    };
    PromptContract {
        role: "quality".into(),
        mindset: format!(
            "Measure this rule at the highest honest altitude. Follow the rule's inspection guide; \
             do not invent your own protocol. independent requires evidence of non-applicability.\
             {template_note}{hits_note} Guide: {guide}"
        ),
        why_now: why_now.into(),
        allowed_actions: allowed,
        forbidden_actions: vec![
            "edit code".into(),
            "mark passing without inspecting".into(),
            "mark independent without evidence the rule does not apply".into(),
            NON_BLOCKING_SMELL_RULE.into(),
        ],
        evidence_clauses: Vec::new(),
        required_evidence: "file/line locators showing compliance, violation, or non-applicability"
            .into(),
        evidence_template,
        examples,
        pre_screened_hits,
        write_back,
        stop_condition: "after recording the verdict, return to loom status".into(),
        human_gate: None,
    }
}

pub(super) fn validator_contract(
    store: &Store,
    edge: &Edge,
    val_name: &str,
    intent_name: &str,
) -> Result<PromptContract> {
    let val = store.get_node(&edge.from_id)?;
    let command = val
        .as_ref()
        .and_then(|n| {
            n.body
                .get("command")
                .and_then(|c| c.as_str())
                .map(String::from)
        })
        .unwrap_or_default();
    let write_back = format!(
        "loom validation run {}  (or)  {}",
        q(intent_name),
        verdict_write_back(edge, val_name, intent_name)
    );
    Ok(PromptContract {
        role: "validator".into(),
        mindset:
            "Run it; do not guess. Record exactly what the command produced. Do not edit code \
                  to make a proof pass. A blocked proof is honest — record it with a reason."
                .into(),
        why_now: format!("validates edge is {}", edge.status),
        allowed_actions: vec![
            format!(
                "run: {}",
                if command.is_empty() {
                    "<no command — manual_check>".into()
                } else {
                    command
                }
            ),
            format!("loom validation run {}", q(intent_name)),
            verdict_write_back(edge, val_name, intent_name),
            FINDING_ADD_ACTION.into(),
        ],
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
        required_evidence:
            "command output, test count, failure message, or a concrete blocker reason".into(),
        evidence_template: None,
        examples: None,
        pre_screened_hits: Vec::new(),
        write_back,
        stop_condition: "after recording the result, return to loom status".into(),
        human_gate: None,
    })
}

/// An implemented intent carrying no passing proof. Distinct from
/// `validator_contract`, which re-runs an EXISTING proof: here the proof itself
/// is the missing form, so the packet's job is to get one registered and run.
pub(super) fn unproven_contract(intent: &Node, has_registered_proof: bool) -> PromptContract {
    let name = q(&intent.name);
    PromptContract {
        role: "validator".into(),
        mindset: "An implemented claim with no passing proof is a claim, not truth. Write a proof \
                  that would FAIL if this behavior broke — a check that only asserts the process \
                  exited 0 proves liveness, not behavior."
            .into(),
        why_now: if has_registered_proof {
            "implemented, proof registered, none passing".into()
        } else {
            "implemented with no registered proof at all".into()
        },
        allowed_actions: vec![
            format!("loom intent show {}", q(&intent.id)),
            "read the grounded files listed in this packet's read set".into(),
            format!(
                "loom validation add --name '<what it proves>' --type test --command '<cmd>' --intent {name}"
            ),
            format!("loom validation run {name}"),
            "for a user-visible flow: loom journey add <spec> then loom journey run <spec>".into(),
            FINDING_ADD_ACTION.into(),
        ],
        forbidden_actions: vec![
            "recording a passing result without running the command".into(),
            "asserting only an exit code when the behavior has observable output".into(),
            "editing code to make a proof pass".into(),
            NON_BLOCKING_SMELL_RULE.into(),
        ],
        evidence_clauses: vec![
            EvidenceClause::CitesRun,
            EvidenceClause::ProofStrengthAtLeast {
                grade: "S2".into(),
            },
        ],
        required_evidence:
            "the command loom ran, its exit status, and the assertion that would have caught a \
             regression"
                .into(),
        evidence_template: None,
        examples: None,
        pre_screened_hits: Vec::new(),
        write_back: format!(
            "loom validation add --name '<what it proves>' --type test --command '<cmd>' --intent {name}  then  loom validation run {name}"
        ),
        stop_condition: "stop once one proof for this intent has actually run".into(),
        human_gate: None,
    }
}

/// Independent re-inspection of a verdict recorded below the confidence floor.
/// The reviewer forms their own hypothesis BEFORE reading the recorded
/// evidence, then confirms or overturns with honest confidence.
pub(super) fn reviewer_contract(
    edge: &Edge,
    owner_role: &str,
    from_name: &str,
    to_name: &str,
    review_floor: f64,
) -> PromptContract {
    let write_back = verdict_write_back(edge, from_name, to_name);
    PromptContract {
        role: owner_role.into(),
        mindset: "This verdict was recorded honestly but with low confidence — it is not settled \
                  truth. Re-inspect INDEPENDENTLY: form your own hypothesis from the code before \
                  reading the recorded criterion/evidence, then compare. Confirm or overturn; \
                  either way, record your own evidence and your own honest confidence."
            .into(),
        why_now: format!(
            "{} verdict stands at confidence {:.2}, below the {} review floor",
            edge.kind, edge.confidence, review_floor
        ),
        allowed_actions: vec![
            "read both endpoints and the grounded code FIRST".into(),
            format!("loom edge show {} (recorded criterion/evidence — read AFTER forming your own view)", edge.id),
            write_back.clone(),
            FINDING_ADD_ACTION.into(),
        ],
        forbidden_actions: vec![
            "edit code".into(),
            "rubber-stamping the prior verdict without independent inspection".into(),
            "inheriting the prior confidence instead of stating your own".into(),
            NON_BLOCKING_SMELL_RULE.into(),
        ],
        evidence_clauses: Vec::new(),
        required_evidence: "fresh file/line or runtime evidence; state explicitly whether the prior verdict was confirmed or overturned".into(),
        evidence_template: None,
        examples: None,
        pre_screened_hits: Vec::new(),
        write_back,
        stop_condition: "after recording the verdict, return to loom status".into(),
        human_gate: None,
    }
}

pub(super) fn prove_contract(hyp: &Node) -> PromptContract {
    let name = q(&hyp.name);
    PromptContract {
        role: "analyzer".into(),
        mindset:
            "An idea is not work until its claim survives contact with the code. Form your own \
                  reading first, then prove or refute the claim. Unproven ideas die honestly."
                .into(),
        why_now: format!("hypothesis '{}' is unproven", hyp.name),
        allowed_actions: vec![
            "read the targeted code".into(),
            format!("loom hypothesis prove {name} supported|refuted --evidence '…'"),
            format!(
                "if SUPPORTED: loom hypothesis adopt {name} — promotes the proven idea to a planned build intent (nothing else re-queues it)"
            ),
        ],
        forbidden_actions: vec![
            "adopt the hypothesis before proving it".into(),
            "edit code".into(),
        ],
        evidence_clauses: Vec::new(),
        required_evidence: "code evidence that the claim holds or fails".into(),
        evidence_template: None,
        examples: None,
        pre_screened_hits: Vec::new(),
        write_back: format!("loom hypothesis prove {name} <supported|refuted> --evidence '…'"),
        stop_condition: "a SUPPORTED verdict is not work until adopted (loom hypothesis adopt) — adopt it to spawn build work; a REFUTED verdict stands as an honest record. Then return to loom status.".into(),
        human_gate: None,
    }
}

pub(super) fn triage_contract(id: &str) -> PromptContract {
    PromptContract {
        role: "analyzer".into(),
        mindset: "Look and decide; do not fix here. Every material finding must become needed, justified, rejected, deferred, blocked, duplicate, or resolved with a concrete reason. Use resolved only after observing the repair."
            .into(),
        why_now: "an evidence-backed finding is unjudged (or its prior judgment went stale when the file changed)".into(),
        allowed_actions: vec![
            format!("loom finding verdict {id} needed --reason <what to do>"),
            format!("loom finding verdict {id} justified --reason <why it is acceptable>"),
            format!("loom finding verdict {id} rejected --reason <why it is false or below threshold>"),
            format!("loom finding verdict {id} deferred --reason <why not scheduled now>"),
            format!("loom finding verdict {id} blocked --reason <what it waits on>"),
            format!("loom finding verdict {id} duplicate --reason <duplicate finding id or target>"),
            format!("loom finding verdict {id} resolved --reason <observed repair and proof>"),
        ],
        forbidden_actions: vec![
            "edit code here (mark it needed, then fix in build/fix)".into(),
            "justified without a concrete reason".into(),
        ],
        evidence_clauses: vec![EvidenceClause::CitesSpans { n: 1 }],
        required_evidence: "a concrete reason: what to do, why it is fine/false/deferred, what blocks it, what it duplicates, or the observed repair"
            .into(),
        evidence_template: None,
        examples: None,
        pre_screened_hits: Vec::new(),
        write_back: format!("loom finding verdict {id} <needed|justified|rejected|deferred|blocked|duplicate|resolved> --reason '…'"),
        stop_condition: "after recording the verdict, return to loom status".into(),
        human_gate: None,
    }
}

/// Structural size/complexity findings need cohesion judgment — not a mechanical
/// "length is intentional" closeout. Owner-count is a hint, not the verdict.
pub(super) fn structural_finding_triage_contract(id: &str) -> PromptContract {
    PromptContract {
        role: "analyzer".into(),
        mindset: "Judge cohesion, not line count. Read the flagged file's top-level modules/handlers. One concern → justified; a catch-all bag of unrelated commands/surfaces → needed (split). Do not fix here."
            .into(),
        why_now: "a structural detector flagged size or complexity; calibrate already set the gate — this packet is about whether the file is one concern".into(),
        allowed_actions: vec![
            format!("loom finding verdict {id} needed --reason <split plan: which concerns to separate>"),
            format!("loom finding verdict {id} justified --reason <the single cohesive concern>"),
            format!("loom finding verdict {id} rejected --reason <why the metric is a false positive>"),
            format!("loom finding verdict {id} deferred --reason <why not scheduled now>"),
            format!("loom finding verdict {id} blocked --reason <what it waits on>"),
            format!("loom finding verdict {id} duplicate --reason <duplicate finding id or target>"),
            format!("loom finding verdict {id} resolved --reason <observed repair and proof>"),
        ],
        forbidden_actions: vec![
            "edit code here (mark it needed, then fix in build/fix)".into(),
            "justified because 'length is intentional' or 'cohesive surface' without naming one concern".into(),
            "justified from owner-count alone without reading the file structure".into(),
            "batch-reaffirm / mechanical closeout of this packet".into(),
        ],
        evidence_clauses: Vec::new(),
        required_evidence: "name the concern(s) you saw: one → justified with that name; several unrelated → needed with a split plan; false gate → rejected"
            .into(),
        evidence_template: None,
        examples: None,
        pre_screened_hits: Vec::new(),
        write_back: format!("loom finding verdict {id} <needed|justified|rejected|deferred|blocked|duplicate|resolved> --reason '…'"),
        stop_condition: "after recording the verdict, return to loom status".into(),
        human_gate: None,
    }
}

/// Ratification: the one write denied to every llm:* lane (INV-8). The LLM's
/// job in this packet is presentation — compile the intent's criterion, origin,
/// grounding and proof state for the human — never the decision itself.
pub(super) fn ratify_contract(id: &str) -> PromptContract {
    PromptContract {
        role: "human".into(),
        mindset: "Product authority. Decide whether this behavior is wanted. An LLM presenting this packet summarizes the intent and stops — it must not ratify, and it must not answer for the human."
            .into(),
        why_now: "the intent's wantedness is unestablished: minted without ratification, or redefined after it".into(),
        allowed_actions: vec![
            format!("loom intent show {id}"),
            format!("loom intent ratify {id} --evidence <why this is wanted>"),
            format!("loom intent retire {id} --reason <why it is not wanted>"),
            format!("loom intent update {id} --description <corrected criterion> --reason <…>  (then re-ratify)"),
        ],
        forbidden_actions: vec![
            "ratifying from an llm:* lane (INV-8 — the write boundary rejects it; do not work around it)".into(),
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
        pre_screened_hits: Vec::new(),
        write_back: format!("loom intent ratify {id} --evidence '…'"),
        stop_condition: "if no human is present, stop — batch ratify packets for the next human session instead of draining them".into(),
        human_gate: Some(
            "ratification is human-only: present the packet, then wait for the human's decision".into(),
        ),
    }
}

pub(super) fn inbox_triage_contract(id: &str) -> PromptContract {
    PromptContract {
        role: "analyzer".into(),
        mindset: "Normalize raw human/external input. Route it to typed graph work or reject it with a reason; do not use inbox for code-audit findings or product questions.".into(),
        why_now: "a raw inbox item is still new".into(),
        allowed_actions: vec![
            format!("loom inbox show {id}"),
            "routing commands: loom intent add / loom hypothesis add / loom rule add / loom task add / loom note add".into(),
            format!("loom inbox mark {id} routed --reason <where it was routed>"),
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
        pre_screened_hits: Vec::new(),
        write_back: format!("loom inbox mark {id} <routed|rejected|duplicate|deferred> --reason '…'"),
        stop_condition: "after disposition, return to loom status".into(),
        human_gate: None,
    }
}

/// The deepen contract. Unlike every other lane, this one does not close a gap
/// — it raises a floor that is already met, so the stop condition is a single
/// move rather than an empty queue.
pub(super) fn deepen_contract(id: &str, next_move: &str) -> PromptContract {
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
                what: "a proof grade higher than the current one".into(),
            },
        ],
        required_evidence: "a proof loom ran, whose new grade is higher than the old one".into(),
        evidence_template: None,
        examples: None,
        pre_screened_hits: Vec::new(),
        write_back: format!("the move for this behavior is: {next_move}"),
        stop_condition: "stop after ONE move — this queue re-ranks after every change, \
                         and the next-most-important thing is probably no longer this one"
            .into(),
        human_gate: None,
    }
}

/// The audit contract: loom found something in its own record that does not
/// look earned.
pub(super) fn audit_contract(remedy: &str) -> PromptContract {
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
            "loom journal tail".into(),
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
        pre_screened_hits: Vec::new(),
        write_back: remedy.to_string(),
        stop_condition: "stop when the claim is either anchored or withdrawn — never when \
                         it merely stops being reported"
            .into(),
        human_gate: None,
    }
}
