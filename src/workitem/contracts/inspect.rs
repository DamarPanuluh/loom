use super::super::{q, EvidenceClause, PromptContract};
use super::{verdict_write_back, FINDING_ADD_ACTION, NON_BLOCKING_SMELL_RULE};
use crate::model::{Edge, Node};

/// Analyze / re-verify contract for an asserted edge.
///
/// `owner_role` is the registry lane that may record the verdict (implements →
/// builder, relates → analyzer, …). It must appear as `prompt_contract.role`
/// so a driver following the contract selects an identity that can write —
/// hardcoding `"analyzer"` here is what made analyze packets advertise
/// `owner_role: builder` beside `prompt_contract.role: analyzer`. The
/// analyzer *mindset* (inspect, do not fix) stays regardless of which lane
/// owns the write.
pub(in super::super) fn analyzer_contract(
    edge: &Edge,
    owner_role: &str,
    from_name: &str,
    to_name: &str,
) -> PromptContract {
    let write_back = verdict_write_back(edge, from_name, to_name);
    PromptContract {
        role: owner_role.into(),
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
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back,
        stop_condition: "after recording the verdict, return to loom status".into(),
        human_gate: None,
    }
}

pub(in super::super) fn research_contract(task: &Node) -> PromptContract {
    let id = &task.id;
    PromptContract {
        role: "analyzer".into(),
        mindset: "Use the host's web search/browser to discover sources, then read actual pages. Prefer primary authoritative sources, record conflicts and the dates that make claims current, and preserve provenance. Search snippets are discovery only, never evidence. The outcome is advisory context: never convert it into a human preference, Fact verification, professional authority, certification, or code edits. It is valid to conclude expert review required, conflicting, or inconclusive.".into(),
        why_now: "a current external fact is explicitly missing and blocks informed work".into(),
        allowed_actions: vec![
            "use the host web search/browser; read actual pages, not snippets".into(),
            format!("loom task source-add {id} --url '<actual-http(s)-page>' --title '<page title>' --publisher '<publisher>' --source-kind <official_docs|standard|regulation|maintainer|primary|secondary> --quote '<substantive exact quote>' [--published-at <RFC3339>] [--fresh-until <RFC3339>]"),
            format!("loom task close {id} --result '<advisory result, conflicts/current dates, or expert-review-required/inconclusive conclusion>'"),
        ],
        forbidden_actions: vec!["use a search snippet as evidence or record a search-results URL".into(), "edit code".into(), "convert web claims into Fact evidence, verification, human preference, or professional certification".into()],
        evidence_clauses: Vec::new(),
        required_evidence: "at least one substantive quote from an actual page with complete URL/title/publisher/kind and Loom-stamped retrieval provenance; note conflicts and relevant dates".into(),
        evidence_template: None, examples: None, pre_screen: None, pre_screened_hits: Vec::new(),
        write_back: format!("loom task source-add {id} <all required source fields>  (repeat per actual page); loom task close {id} --result '<advisory synthesis>'"),
        stop_condition: "after recording sources and closing with an advisory result, return to loom status".into(),
        human_gate: None,
    }
}

pub(in super::super) fn exemplar_contract(
    edge: &Edge,
    pattern: &str,
    file: &str,
) -> PromptContract {
    let write_back = format!(
        "loom pattern exemplar verdict {} <ground|issue|independent> --criterion '…' --evidence '{file}:<line>' --confidence <0.0-1.0>",
        edge.id
    );
    PromptContract {
        role: "analyzer".into(),
        mindset: format!("Inspect the uniquely located symbol in {file} against Pattern '{pattern}'. Decide whether the code actually exemplifies the authored rationale and use boundaries; do not infer compliance from names."),
        why_now: format!("Exemplar edge is {} and cannot route guidance until reviewed", edge.status),
        allowed_actions: vec!["read the Pattern body and the located symbol".into(), write_back.clone(), FINDING_ADD_ACTION.into()],
        forbidden_actions: vec!["edit code".into(), "use generic relationship reasoning".into(), "record ground without inspecting the located source".into()],
        evidence_clauses: vec![EvidenceClause::CitesSpans { n: 1 }],
        required_evidence: "a file/line citation inside the uniquely located exemplar symbol".into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back,
        stop_condition: "after recording exactly one Exemplar verdict, return to loom status".into(),
        human_gate: None,
    }
}

pub(in super::super) fn prove_contract(hyp: &Node) -> PromptContract {
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
            "only when a current external fact blocks the verdict: loom task add '<bounded question>' --kind research --why-external '<why current external knowledge is needed>' --preferred-source '<authoritative source guidance>'".into(),
        ],
        forbidden_actions: vec![
            "adopt the hypothesis before proving it".into(),
            "edit code".into(),
        ],
        evidence_clauses: Vec::new(),
        required_evidence: "code evidence that the claim holds or fails".into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: format!("loom hypothesis prove {name} <supported|refuted> --evidence '…'"),
        stop_condition: "a SUPPORTED verdict is not work until adopted (loom hypothesis adopt) — adopt it to spawn build work; a REFUTED verdict stands as an honest record. Then return to loom status.".into(),
        human_gate: None,
    }
}

/// Independent re-inspection of a verdict recorded below the confidence floor.
/// The reviewer forms their own hypothesis BEFORE reading the recorded
/// evidence, then confirms or overturns with honest confidence.
pub(in super::super) fn reviewer_contract(
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
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back,
        stop_condition: "after recording the verdict, return to loom status".into(),
        human_gate: None,
    }
}

/// Adversarial review challenges one exact settled verdict revision without
/// granting the reviewer authority to rewrite it. A credible counterexample
/// becomes a Finding; Triage remains the adjudication seam.
pub(in super::super) fn adversarial_reviewer_contract(
    edge: &Edge,
    from_name: &str,
    to_name: &str,
    prior_profile: Option<&str>,
) -> PromptContract {
    let avoid = prior_profile
        .map(|profile| format!(" Prefer an executor profile other than '{profile}'."))
        .unwrap_or_else(|| {
            " The prior executor profile is unavailable; declare your own profile so independence is auditable.".into()
        });
    PromptContract {
        role: "analyzer".into(),
        mindset: format!(
            "Act as a refutation-biased reviewer. Form a concrete falsification hypothesis from the endpoints and code BEFORE reading the prior verdict evidence; then attack boundary cases, negative paths, and hidden assumptions. A failed attack records survived, not proof of perfection. A credible break records counterexample and lets Triage decide.{}",
            avoid
        ),
        why_now: format!(
            "the current {} claim '{} —{}→ {}' is in the bounded high-risk frontier and has no adversarial attempt against this verdict revision",
            edge.kind, from_name, edge.kind, to_name
        ),
        allowed_actions: vec![
            "read both endpoints and grounded code before reading the prior verdict evidence".into(),
            format!("loom edge show {} (read only after writing down your own falsification hypothesis)", edge.id),
            "run focused read-only checks or tests that exercise the hypothesis".into(),
            format!("loom challenge record {} <survived|counterexample|inconclusive> --hypothesis '<what would falsify this claim>' --evidence '<what you tried and observed, including file:line or journal:id>' [--impact '<consequence>'] --confidence <0.0-1.0>", edge.id),
        ],
        forbidden_actions: vec![
            "edit source code while reviewing".into(),
            "replace the challenged edge verdict directly".into(),
            "read the prior criterion/evidence before forming an independent hypothesis".into(),
            "treat an inconclusive attempt as either confirmation or refutation".into(),
            NON_BLOCKING_SMELL_RULE.into(),
        ],
        required_evidence: "a substantive falsification hypothesis, a substantive account of the attempt, and at least one live file:line or journal:id citation; counterexample also requires impact".into(),
        evidence_clauses: vec![
            EvidenceClause::CitesSpans { n: 1 },
            EvidenceClause::VerificationAtLeast { level: "cited".into() },
            EvidenceClause::Prose,
        ],
        evidence_template: None,
        examples: None,
        pre_screened_hits: Vec::new(),
        pre_screen: None,
        write_back: format!(
            "loom challenge record {} <survived|counterexample|inconclusive> --hypothesis '<falsifiable attack>' --evidence '<attempt + file:line or journal:id>' [--impact '<required for counterexample>'] --confidence <0.0-1.0>",
            edge.id
        ),
        stop_condition: "stop when one current Challenge fact exists for this exact Verdict revision; a counterexample must also have created its untriaged Finding atomically, then return to loom status".into(),
        human_gate: None,
    }
}
